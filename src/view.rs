//! The view: displays the block list and the input editor (or the emulator
//! in full screen), and forwards keyboard and mouse to the editor or the shell.

use std::cell::RefCell;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use alacritty_terminal::event::Event as TermEvent;
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point as GridPoint, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::TermMode;
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{CursorShape, NamedColor};
use futures::StreamExt;
use futures::channel::mpsc::UnboundedReceiver;
use gpui::{
    App, Bounds, ClipboardItem, Context, Entity, FocusHandle, Focusable, Font, FontFeatures,
    FontStyle, FontWeight, InputHandler, KeyDownEvent, Keystroke, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, ScrollDelta, ScrollWheelEvent,
    SharedString, UTF16Selection, Window, actions, canvas, div, fill, point, prelude::*, px, size,
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::block::{Block, LiveBlock, RowCol};
use crate::config::Config;
use crate::editor::Editor;
use crate::git;
use crate::completion::{Candidate, Completion, Engine, ShellState, resolve_path};
use crate::highlight::{self, Style};
use crate::history::{HistoryCursor, HistoryStore, Meta};
use crate::keys;
use crate::palette::{Palette, hsla, mix};
use crate::scene::{Label, Layer, Metrics, RowColors, Scene};
use crate::terminal::{GridSize, Mode, Session, Terminal};

actions!(
    terminal,
    [
        Copy,
        Cut,
        Paste,
        SelectAll,
        Undo,
        Redo,
        ClearBlocks,
        IncreaseFontSize,
        DecreaseFontSize,
        ResetFontSize
    ]
);

const DEFAULT_TITLE: &str = "RTerm";

/// What a pane reports to the workspace.
pub enum PaneEvent {
    /// The pane's shell has exited.
    Exited,
}

/// Margin above and below a block's content, normal and compact.
const BLOCK_PADDING: Pixels = px(10.);
const COMPACT_BLOCK_PADDING: Pixels = px(4.);
/// Space between a block's header and its output, normal and compact.
const HEADER_GAP: Pixels = px(4.);
const COMPACT_HEADER_GAP: Pixels = px(1.);
/// Margin above and below the input editor.
const EDITOR_PADDING: Pixels = px(10.);
/// Cells reserved for the prompt symbol, to the left of the input.
const PROMPT_CELLS: usize = 2;
const PROMPT_SYMBOL: &str = "❯";

/// Number of rows visible in a menu (completion, history).
const POPUP_ROWS: usize = 8;
/// Number of commands offered by the history search.
const HISTORY_RESULTS: usize = 200;

/// What a pane shares with the others: settings, colors, history and
/// completion engine.
#[derive(Clone)]
pub struct PaneContext {
    pub config: Rc<Config>,
    pub palette: Rc<Palette>,
    pub history: Rc<RefCell<HistoryStore>>,
    pub completion: Rc<Engine>,
}

/// A command found in the history, ready to display.
struct HistoryResult {
    command: String,
    detail: String,
    failed: bool,
}

/// A menu displayed above the input editor.
enum Popup {
    Completion { completion: Completion, selected: usize },
    History { query: String, results: Vec<HistoryResult>, selected: usize },
}

/// What the mouse is currently selecting.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Drag {
    Grid,
    Editor,
    Block,
}

/// A text selection in a block's output.
struct BlockSelection {
    block: u64,
    anchor: RowCol,
    head: RowCol,
    /// False until the mouse has moved: it is then a plain click.
    dragged: bool,
}

impl BlockSelection {
    fn range(&self) -> (RowCol, RowCol) {
        (self.anchor.min(self.head), self.anchor.max(self.head))
    }
}

/// Where a block sits on screen, to find what is under the mouse.
struct BlockHit {
    id: u64,
    top: Pixels,
    bottom: Pixels,
    rows_top: Pixels,
    rows: usize,
}

/// Geometry of the last frame drawn.
#[derive(Default)]
struct Layout {
    mode: Option<Mode>,
    /// Top-left corner of column 0.
    grid_origin: Point<Pixels>,
    blocks: Vec<BlockHit>,
    editor_top: Option<Pixels>,
    editor_text_origin: Point<Pixels>,
    editor_columns: usize,
    viewport_height: f32,
}

pub struct TerminalView {
    terminal: Terminal,
    config: Rc<Config>,
    palette: Rc<Palette>,
    focus_handle: FocusHandle,
    title: SharedString,
    font_size: f32,
    editor: Editor,
    history: Rc<RefCell<HistoryStore>>,
    history_cursor: HistoryCursor,
    completion: Rc<Engine>,
    popup: Option<Popup>,
    /// The input's highlighting, with what it was computed from: the text,
    /// the current directory and the number of commands the shell knows.
    highlighted: (String, Option<PathBuf>, usize, Vec<Style>),
    layout: Layout,
    /// Scroll of the block list, in pixels from the bottom.
    scroll: f32,
    max_scroll: f32,
    /// Total height of the blocks in the last frame.
    content_height: f32,
    drag: Option<Drag>,
    block_selection: Option<BlockSelection>,
    /// The block selected with a click.
    selected_block: Option<u64>,
    /// Wheel scrolling not yet converted into whole lines.
    scroll_remainder: f32,
    /// Text being composed (dead key, IME).
    marked_text: Option<String>,
    /// The current directory's git branch, with what it was computed from.
    git_branch: (Option<PathBuf>, usize, Option<String>),
    /// `RTERM_RUN` commands still to launch (demos and tests).
    script: Vec<String>,
}

impl TerminalView {
    /// Opens a pane: a shell launched in `cwd` (or the default directory).
    /// `script` is a list of commands to launch right away (`RTERM_RUN`).
    pub fn open(
        context: PaneContext,
        cwd: Option<PathBuf>,
        script: Vec<String>,
        window: &mut Window,
        cx: &mut App,
    ) -> anyhow::Result<Entity<Self>> {
        // Provisional size: the real one is computed on the first frame.
        let initial = GridSize { columns: 80, lines: 24, cell_width: 8.0, cell_height: 17.0 };
        let (terminal, events) = Terminal::spawn(&context.config, initial, cwd)?;
        Ok(cx.new(|cx| Self::new(context, terminal, events, script, window, cx)))
    }

    fn new(
        context: PaneContext,
        terminal: Terminal,
        events: UnboundedReceiver<TermEvent>,
        script: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle);

        cx.spawn_in(window, async move |this, cx| Self::pump_events(this, events, cx).await)
            .detach();
        cx.observe_window_activation(window, |_, _, cx| cx.notify()).detach();

        let PaneContext { config, palette, history, completion } = context;
        TerminalView {
            terminal,
            palette,
            font_size: config.font.size,
            config,
            focus_handle,
            title: DEFAULT_TITLE.into(),
            editor: Editor::default(),
            history,
            history_cursor: HistoryCursor::default(),
            completion,
            popup: None,
            highlighted: Default::default(),
            layout: Layout::default(),
            scroll: 0.0,
            max_scroll: 0.0,
            content_height: 0.0,
            drag: None,
            block_selection: None,
            selected_block: None,
            scroll_remainder: 0.0,
            marked_text: None,
            git_branch: (None, 0, None),
            script,
        }
    }

    /// The shell's current directory, if it has already reported it.
    pub fn cwd(&self) -> Option<PathBuf> {
        self.terminal.session.lock().cwd.clone()
    }

    pub fn set_palette(&mut self, palette: Rc<Palette>, cx: &mut Context<Self>) {
        self.palette = palette;
        cx.notify();
    }

    /// Applies settings changed while RTerm runs (block style, font size).
    pub fn set_config(&mut self, config: Rc<Config>, cx: &mut Context<Self>) {
        if config.font.size != self.config.font.size {
            self.font_size = config.font.size;
        }
        self.config = config;
        cx.notify();
    }

    /// The tab title: the running command, otherwise the current directory.
    pub fn tab_title(&self) -> String {
        let session = self.terminal.session.lock();
        if let Some(live) = session.live.as_ref().filter(|live| !live.command.is_empty()) {
            return live.command.lines().next().unwrap_or_default().to_owned();
        }
        match &session.cwd {
            Some(cwd) => display_path(cwd),
            None => self.title.to_string(),
        }
    }

    async fn pump_events(
        this: gpui::WeakEntity<Self>,
        mut events: UnboundedReceiver<TermEvent>,
        cx: &mut gpui::AsyncWindowContext,
    ) {
        while let Some(event) = events.next().await {
            let alive = this.update_in(cx, |this, window, cx| this.on_term_event(event, window, cx));
            if alive.is_err() {
                break;
            }
        }
    }

    fn on_term_event(&mut self, event: TermEvent, _: &mut Window, cx: &mut Context<Self>) {
        match event {
            TermEvent::Wakeup => {
                self.record_finished();
                self.run_script(cx);
                cx.notify();
            }
            TermEvent::PtyWrite(text) => self.terminal.write(text.into_bytes()),
            TermEvent::Title(title) => self.set_title(title.into(), cx),
            TermEvent::ResetTitle => self.set_title(DEFAULT_TITLE.into(), cx),
            TermEvent::ClipboardStore(_, text) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
            TermEvent::ClipboardLoad(_, format) => {
                if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                    self.terminal.write(format(&text).into_bytes());
                }
            }
            TermEvent::ColorRequest(index, format) => {
                let color = {
                    let session = self.terminal.session.lock();
                    self.palette.indexed(index, session.active_term().colors())
                };
                self.terminal.write(format(color).into_bytes());
            }
            TermEvent::TextAreaSizeRequest(format) => {
                self.terminal.write(format(self.terminal.size().window_size()).into_bytes());
            }
            TermEvent::Exit => cx.emit(PaneEvent::Exited),
            TermEvent::ChildExit(_)
            | TermEvent::Bell
            | TermEvent::MouseCursorDirty
            | TermEvent::CursorBlinkingChange => {}
        }
    }

    fn set_title(&mut self, title: SharedString, cx: &mut Context<Self>) {
        self.title = title;
        cx.notify();
    }

    fn font(&self) -> Font {
        Font {
            family: if self.config.font.family.is_empty() {
                crate::platform::DEFAULT_FONT.into()
            } else {
                self.config.font.family.clone().into()
            },
            features: if self.config.font.ligatures {
                FontFeatures::default()
            } else {
                FontFeatures::disable_ligatures()
            },
            fallbacks: None,
            weight: FontWeight::NORMAL,
            style: FontStyle::Normal,
        }
    }

    fn mode(&self) -> Mode {
        self.terminal.session.lock().mode()
    }

    // --- Input ----------------------------------------------------------

    /// Sends input to the shell and goes back to the bottom of the display.
    fn send_input(&mut self, bytes: Vec<u8>, cx: &mut Context<Self>) {
        {
            let mut session = self.terminal.session.lock();
            let term = session.active_term_mut();
            term.scroll_display(Scroll::Bottom);
            term.selection = None;
        }
        self.scroll = 0.0;
        self.terminal.write(bytes);
        cx.notify();
    }

    /// Typed text: into the editor if it is displayed, otherwise to the shell.
    fn insert_text(&mut self, text: &str, cx: &mut Context<Self>) {
        if self.mode() == Mode::Editing {
            if let Some(Popup::History { query, .. }) = &mut self.popup {
                // The history search is open: we type into it.
                query.push_str(text);
                self.refresh_history();
            } else {
                self.editor.insert(text);
                self.history_cursor.reset();
                self.refresh_completion();
            }
            self.scroll = 0.0;
            cx.notify();
        } else {
            self.send_input(text.as_bytes().to_vec(), cx);
        }
    }

    /// Launches the next `RTERM_RUN` command as soon as the shell is ready.
    fn run_script(&mut self, cx: &mut Context<Self>) {
        if !self.script.is_empty()
            && self.mode() == Mode::Editing
            && let Some(command) = self.script.pop()
        {
            self.editor.insert(&command);
            self.submit(cx);
        }
    }

    /// Submits the editor's command: it goes to the shell in one piece.
    fn submit(&mut self, cx: &mut Context<Self>) {
        if self.editor.text().trim().is_empty() {
            return;
        }
        let command = self.editor.take();
        self.popup = None;
        self.history.borrow_mut().push(&command);
        self.history_cursor.reset();

        let bracketed = {
            let mut session = self.terminal.session.lock();
            session.submit(command.clone());
            session.prompt.mode().contains(TermMode::BRACKETED_PASTE)
        };
        // "Paste" mode makes zsh accept a multi-line command as a whole
        // instead of running it line by line.
        let mut bytes = if bracketed {
            format!("\x1b[200~{}\x1b[201~", command.replace('\x1b', "")).into_bytes()
        } else {
            command.into_bytes()
        };
        bytes.push(b'\r');

        self.block_selection = None;
        self.selected_block = None;
        self.send_input(bytes, cx);
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let (mode, term_mode) = {
            let session = self.terminal.session.lock();
            (session.mode(), *session.active_term().mode())
        };

        // Shift+Page Up/Down scrolls the display, except in vim, less, etc.
        if keystroke.modifiers.shift && !term_mode.contains(TermMode::ALT_SCREEN) {
            let page = self.layout.viewport_height * 0.9;
            let scrolled = match (mode, keystroke.key.as_str()) {
                (Mode::Fullscreen, key) => {
                    let scroll = match key {
                        "pageup" => Some(Scroll::PageUp),
                        "pagedown" => Some(Scroll::PageDown),
                        "home" => Some(Scroll::Top),
                        "end" => Some(Scroll::Bottom),
                        _ => None,
                    };
                    if let Some(scroll) = scroll {
                        self.terminal.session.lock().active_term_mut().scroll_display(scroll);
                    }
                    scroll.is_some()
                }
                (_, "pageup") => {
                    self.scroll = (self.scroll + page).min(self.max_scroll);
                    true
                }
                (_, "pagedown") => {
                    self.scroll = (self.scroll - page).max(0.0);
                    true
                }
                _ => false,
            };
            if scrolled {
                cx.stop_propagation();
                cx.notify();
                return;
            }
        }

        if mode == Mode::Editing {
            if self.editor_key(keystroke, cx) {
                cx.stop_propagation();
                cx.notify();
            }
            return;
        }

        if let Some(bytes) = keys::to_bytes(keystroke, term_mode, self.config.terminal.option_as_meta) {
            self.send_input(bytes, cx);
            cx.stop_propagation();
        }
    }

    /// Keys of the input editor. Returns `false` for ordinary text, which
    /// then arrives through the system input handler.
    fn editor_key(&mut self, keystroke: &Keystroke, cx: &mut Context<Self>) -> bool {
        if let Some(handled) = self.popup_key(keystroke) {
            return handled;
        }
        let handled = self.edit_key(keystroke, cx);
        if handled {
            self.refresh_completion();
        }
        handled
    }

    fn edit_key(&mut self, keystroke: &Keystroke, cx: &mut Context<Self>) -> bool {
        let mods = &keystroke.modifiers;
        let key = keystroke.key.as_str();
        let select = mods.shift;
        let columns = self.layout.editor_columns.max(2);

        // readline habits.
        if mods.control && !mods.alt && !mods.platform {
            match key {
                "a" => self.editor.move_line_start(false),
                "e" => self.editor.move_line_end(false),
                "k" => self.editor.delete_to_line_end(),
                "u" => self.editor.delete_to_line_start(),
                "w" => self.editor.delete_word_back(),
                "c" => {
                    self.editor.take();
                }
                // Ctrl+D on an empty line closes the shell, as usual.
                "d" if self.editor.is_empty() => self.terminal.write(vec![0x04]),
                "d" => self.editor.delete_forward(),
                "l" => self.terminal.session.lock().blocks.clear(),
                "r" => self.open_history_search(),
                _ => {}
            }
            self.history_cursor.reset();
            return true;
        }

        match key {
            "up" | "down" => {
                let moved = if key == "up" {
                    self.editor.move_up(select, columns)
                } else {
                    self.editor.move_down(select, columns)
                };
                // At the edge of the text, the arrows walk the history.
                if !moved && !select {
                    let history = self.history.borrow();
                    let recalled = if key == "up" {
                        self.history_cursor.previous(&history, self.editor.text()).map(str::to_owned)
                    } else {
                        self.history_cursor.next(&history)
                    };
                    drop(history);
                    if let Some(text) = recalled {
                        self.editor.set_text(&text);
                    }
                }
                self.scroll = 0.0;
                return true;
            }
            "enter" if mods.shift || mods.alt => self.editor.insert("\n"),
            "enter" => {
                self.submit(cx);
                return true;
            }
            "backspace" if mods.platform => self.editor.delete_to_line_start(),
            "backspace" if mods.alt => self.editor.delete_word_back(),
            "backspace" => self.editor.backspace(),
            "delete" => self.editor.delete_forward(),
            "left" if mods.platform => self.editor.move_line_start(select),
            "left" if mods.alt => self.editor.move_word_left(select),
            "left" => self.editor.move_left(select),
            "right" if mods.platform => self.editor.move_line_end(select),
            "right" if mods.alt => self.editor.move_word_right(select),
            "right" => self.editor.move_right(select),
            "home" => self.editor.move_line_start(select),
            "end" => self.editor.move_line_end(select),
            "tab" => {
                self.complete();
                return true;
            }
            "escape" => {
                self.editor.clear_selection();
                self.editor.clear_fields();
                self.block_selection = None;
                self.selected_block = None;
            }
            _ => return false,
        }
        self.history_cursor.reset();
        self.scroll = 0.0;
        true
    }

    // --- Completion, history, workflows -----------------------------------

    /// Candidates for the word under the cursor.
    fn shell_completion(&self) -> Option<Completion> {
        let (cwd, path, commands) = {
            let session = self.terminal.session.lock();
            (session.cwd.clone(), session.path.clone(), session.shell_commands.clone())
        };
        let shell = ShellState {
            cwd: cwd.as_deref(),
            path: path.as_deref(),
            commands: commands.as_deref(),
        };
        self.completion.complete(self.editor.text(), self.editor.cursor(), &shell)
    }

    fn accept(&mut self, range: Range<usize>, candidate: &Candidate) {
        let mut text = candidate.insert.clone();
        if candidate.append_space {
            text.push(' ');
        }
        self.editor.replace(range, &text);
    }

    /// Tab key: next workflow parameter, otherwise completion.
    fn complete(&mut self) {
        if self.editor.has_fields() && self.editor.next_field() {
            return;
        }
        let Some(mut completion) = self.shell_completion() else {
            return;
        };
        if let [only] = completion.candidates.as_slice() {
            self.accept(completion.range.clone(), &only.clone());
            return;
        }
        // Several possibilities: advance as far as they agree, then
        // let the user choose in the menu.
        if let Some(prefix) = completion.common_prefix().map(str::to_owned) {
            self.editor.replace(completion.range.clone(), &prefix);
            match self.shell_completion() {
                Some(narrowed) => completion = narrowed,
                None => return,
            }
        }
        self.popup = Some(Popup::Completion { completion, selected: 0 });
    }

    /// Updates the completion menu after the text was edited.
    fn refresh_completion(&mut self) {
        if matches!(self.popup, Some(Popup::Completion { .. })) {
            self.popup = self
                .shell_completion()
                // Nothing typed for this word anymore: the menu has no purpose left.
                .filter(|completion| !completion.range.is_empty())
                .map(|completion| Popup::Completion { completion, selected: 0 });
        }
    }

    fn open_history_search(&mut self) {
        self.popup = Some(Popup::History { query: String::new(), results: Vec::new(), selected: 0 });
        self.refresh_history();
    }

    fn refresh_history(&mut self) {
        let Some(Popup::History { query, results, selected }) = &mut self.popup else {
            return;
        };
        let history = self.history.borrow();
        *results = history
            .search(query, HISTORY_RESULTS)
            .into_iter()
            .map(|(command, meta)| HistoryResult {
                command: command.to_owned(),
                detail: meta.map(history_detail).unwrap_or_default(),
                failed: meta.is_some_and(|meta| meta.exit.is_some_and(|code| code != 0)),
            })
            .collect();
        *selected = 0;
    }

    /// Keys of an open menu. `None`: the key goes back to the editor.
    fn popup_key(&mut self, keystroke: &Keystroke) -> Option<bool> {
        let key = keystroke.key.as_str();
        let mods = &keystroke.modifiers;
        let step = |selected: &mut usize, count: usize, forward: bool| {
            if count > 0 {
                *selected = if forward { (*selected + 1) % count } else { (*selected + count - 1) % count };
            }
        };

        match self.popup.as_mut()? {
            Popup::Completion { completion, selected } => match key {
                "up" => step(selected, completion.candidates.len(), false),
                "down" => step(selected, completion.candidates.len(), true),
                "escape" => self.popup = None,
                "tab" | "enter" if !mods.shift && !mods.alt => {
                    let range = completion.range.clone();
                    let candidate = completion.candidates.get(*selected).cloned();
                    self.popup = None;
                    if let Some(candidate) = candidate {
                        self.accept(range, &candidate);
                        // A directory: carry on into its content.
                        if !candidate.append_space {
                            self.popup = self
                                .shell_completion()
                                .map(|completion| Popup::Completion { completion, selected: 0 });
                        }
                    }
                }
                _ => return None,
            },
            Popup::History { query, results, selected } => match key {
                "up" => step(selected, results.len(), false),
                "down" => step(selected, results.len(), true),
                "r" if mods.control => step(selected, results.len(), true),
                "escape" => self.popup = None,
                "enter" => {
                    let chosen = results.get(*selected).map(|result| result.command.clone());
                    self.popup = None;
                    if let Some(command) = chosen {
                        self.editor.set_text(&command);
                    }
                }
                "backspace" => {
                    if mods.platform {
                        query.clear();
                    } else {
                        query.pop();
                    }
                    self.refresh_history();
                }
                // Typed text arrives through the input handler; the other
                // keys make no sense here and are ignored.
                _ => return Some(mods.control || mods.platform || keystroke.key_char.is_none()),
            },
        }
        Some(true)
    }

    /// Writes the commands that just finished to the history.
    fn record_finished(&mut self) {
        let finished = std::mem::take(&mut self.terminal.session.lock().finished);
        let mut history = self.history.borrow_mut();
        for done in finished {
            history.record(&done.command, done.cwd.as_deref(), done.exit, done.duration);
        }
    }

    /// Puts a workflow into the editor, with its first parameter selected.
    pub fn insert_workflow(&mut self, text: &str, fields: Vec<Range<usize>>, cx: &mut Context<Self>) {
        self.popup = None;
        self.editor.set_template(text, fields);
        self.scroll = 0.0;
        cx.notify();
    }

    /// `RTERM_DEMO` steps specific to a pane (screenshots, tests).
    pub fn demo(&mut self, step: &str, cx: &mut Context<Self>) {
        if let Some(text) = step.strip_prefix("input:") {
            self.editor.insert(text);
        } else if let Some(text) = step.strip_prefix("complete:") {
            self.editor.insert(text);
            self.complete();
        } else if let Some(query) = step.strip_prefix("history:") {
            self.open_history_search();
            if let Some(Popup::History { query: current, .. }) = &mut self.popup {
                current.push_str(query);
            }
            self.refresh_history();
        }
        cx.notify();
    }

    // --- Clipboard and actions -------------------------------------------

    /// The text to copy: the editor's selection, a block's, or a whole block.
    fn selected_text(&self) -> Option<String> {
        let session = self.terminal.session.lock();
        let mode = session.mode();
        if mode == Mode::Fullscreen {
            return session.active_term().selection_to_string();
        }
        if mode == Mode::Editing
            && let Some(text) = self.editor.selected_text()
        {
            return Some(text.to_owned());
        }
        let (block, range) = match (&self.block_selection, self.selected_block) {
            (Some(selection), _) => (selection.block, Some(selection.range())),
            (None, Some(block)) => (block, None),
            (None, None) => return None,
        };
        if let Some(frozen) = session.blocks.iter().find(|candidate| candidate.id == block) {
            return Some(frozen.text(range));
        }
        session.live.as_ref().filter(|live| live.id == block).map(|live| live.text(range))
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.selected_text().filter(|text| !text.is_empty()) {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    fn cut(&mut self, _: &Cut, _: &mut Window, cx: &mut Context<Self>) {
        if self.mode() == Mode::Editing
            && let Some(text) = self.editor.cut()
        {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            cx.notify();
        }
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        let (mode, bracketed) = {
            let session = self.terminal.session.lock();
            (session.mode(), session.active_term().mode().contains(TermMode::BRACKETED_PASTE))
        };
        if mode == Mode::Editing {
            self.insert_text(&text.replace("\r\n", "\n"), cx);
            return;
        }
        let bytes = if bracketed {
            // We strip Escapes so that pasted text cannot close "paste"
            // mode by itself and get a command executed.
            format!("\x1b[200~{}\x1b[201~", text.replace('\x1b', "")).into_bytes()
        } else {
            text.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
        };
        self.send_input(bytes, cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        if self.mode() == Mode::Editing {
            self.editor.select_all();
            cx.notify();
        }
    }

    fn undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        if self.mode() == Mode::Editing {
            self.editor.undo();
            cx.notify();
        }
    }

    fn redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        if self.mode() == Mode::Editing {
            self.editor.redo();
            cx.notify();
        }
    }

    fn clear_blocks(&mut self, _: &ClearBlocks, _: &mut Window, cx: &mut Context<Self>) {
        self.terminal.session.lock().blocks.clear();
        self.block_selection = None;
        self.selected_block = None;
        self.scroll = 0.0;
        cx.notify();
    }

    fn increase_font_size(&mut self, _: &IncreaseFontSize, _: &mut Window, cx: &mut Context<Self>) {
        self.font_size = (self.font_size + 1.0).min(48.0);
        cx.notify();
    }

    fn decrease_font_size(&mut self, _: &DecreaseFontSize, _: &mut Window, cx: &mut Context<Self>) {
        self.font_size = (self.font_size - 1.0).max(6.0);
        cx.notify();
    }

    fn reset_font_size(&mut self, _: &ResetFontSize, _: &mut Window, cx: &mut Context<Self>) {
        self.font_size = self.config.font.size;
        cx.notify();
    }

    // --- Mouse -----------------------------------------------------------

    /// Column and row (unbounded) of a position, from an origin.
    fn cell_at(&self, position: Point<Pixels>, origin: Point<Pixels>) -> (f32, f32) {
        let grid = self.terminal.size();
        (
            f32::from(position.x - origin.x) / grid.cell_width,
            f32::from(position.y - origin.y) / grid.cell_height,
        )
    }

    /// The full-screen emulator cell under a window position.
    fn grid_point_at(&self, position: Point<Pixels>) -> (GridPoint, Side) {
        let grid = self.terminal.size();
        let (x, y) = self.cell_at(position, self.layout.grid_origin);
        let column = (x.max(0.0) as usize).min(grid.columns.saturating_sub(1));
        let row = (y.max(0.0) as usize).min(grid.lines.saturating_sub(1));
        let side = if x.fract() < 0.5 { Side::Left } else { Side::Right };

        let display_offset = self.terminal.session.lock().active_term().grid().display_offset();
        let line = Line(row as i32 - display_offset as i32);
        (GridPoint::new(line, Column(column)), side)
    }

    /// A position in a block's output. With `only`, the position is clamped
    /// to that block even if the mouse has left it.
    fn block_point_at(&self, position: Point<Pixels>, only: Option<u64>) -> Option<(u64, RowCol)> {
        let hit = match only {
            Some(id) => self.layout.blocks.iter().find(|hit| hit.id == id)?,
            None => self
                .layout
                .blocks
                .iter()
                .find(|hit| position.y >= hit.top && position.y < hit.bottom)?,
        };
        let origin = point(self.layout.grid_origin.x, hit.rows_top);
        let (x, y) = self.cell_at(position, origin);
        let columns = self.terminal.size().columns;
        let row = (y.max(0.0) as usize).min(hit.rows.saturating_sub(1));
        let column = (x.max(0.0) as usize).min(columns.saturating_sub(1));
        Some((hit.id, (row, column)))
    }

    /// The editor text position under the mouse.
    fn editor_offset_at(&self, position: Point<Pixels>) -> usize {
        let (x, y) = self.cell_at(position, self.layout.editor_text_origin);
        let columns = self.layout.editor_columns.max(2);
        self.editor.offset_at(y.max(0.0) as usize, x.round().max(0.0) as usize, columns)
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle);
        cx.notify();

        if self.layout.mode == Some(Mode::Fullscreen) {
            let (point, side) = self.grid_point_at(event.position);
            let mut session = self.terminal.session.lock();
            let term = session.active_term_mut();
            match &mut term.selection {
                Some(selection) if event.modifiers.shift => selection.update(point, side),
                _ => {
                    let kind = match event.click_count {
                        0 | 1 => SelectionType::Simple,
                        2 => SelectionType::Semantic,
                        _ => SelectionType::Lines,
                    };
                    term.selection = Some(Selection::new(kind, point, side));
                }
            }
            self.drag = Some(Drag::Grid);
            return;
        }

        if self.layout.editor_top.is_some_and(|top| event.position.y >= top) {
            let offset = self.editor_offset_at(event.position);
            self.editor.move_to(offset, event.modifiers.shift);
            self.block_selection = None;
            self.drag = Some(Drag::Editor);
            return;
        }

        self.editor.clear_selection();
        self.block_selection = self.block_point_at(event.position, None).map(|(block, at)| {
            BlockSelection { block, anchor: at, head: at, dragged: false }
        });
        self.drag = self.block_selection.is_some().then_some(Drag::Block);
        if self.block_selection.is_none() {
            self.selected_block = None;
        }
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if event.pressed_button != Some(MouseButton::Left) {
            return;
        }
        match self.drag {
            Some(Drag::Grid) => {
                let (point, side) = self.grid_point_at(event.position);
                let mut session = self.terminal.session.lock();
                if let Some(selection) = session.active_term_mut().selection.as_mut() {
                    selection.update(point, side);
                }
            }
            Some(Drag::Editor) => {
                let offset = self.editor_offset_at(event.position);
                self.editor.move_to(offset, true);
            }
            Some(Drag::Block) => {
                let block = self.block_selection.as_ref().map(|selection| selection.block);
                let head = self.block_point_at(event.position, block);
                if let (Some(selection), Some((_, head))) = (&mut self.block_selection, head) {
                    selection.dragged |= head != selection.anchor;
                    selection.head = head;
                }
            }
            None => return,
        }
        cx.notify();
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        match self.drag.take() {
            Some(Drag::Grid) => {
                // A plain click leaves an empty selection: remove it.
                let mut session = self.terminal.session.lock();
                let term = session.active_term_mut();
                if term.selection.as_ref().is_some_and(|selection| selection.is_empty()) {
                    term.selection = None;
                }
            }
            Some(Drag::Block) => {
                // A click without dragging: select the whole block (or
                // deselect it if it already was).
                if let Some(selection) = self.block_selection.take_if(|selection| !selection.dragged) {
                    let already = self.selected_block == Some(selection.block);
                    self.selected_block = (!already).then_some(selection.block);
                } else {
                    self.selected_block = None;
                }
            }
            Some(Drag::Editor) | None => {}
        }
        cx.notify();
    }

    fn on_scroll(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let cell_height = self.terminal.size().cell_height;
        let pixels = match event.delta {
            ScrollDelta::Pixels(pixels) => f32::from(pixels.y),
            ScrollDelta::Lines(lines) => lines.y * cell_height,
        };

        if self.layout.mode != Some(Mode::Fullscreen) {
            self.scroll = (self.scroll + pixels).clamp(0.0, self.max_scroll);
            cx.notify();
            return;
        }

        self.scroll_remainder += pixels / cell_height;
        let lines = self.scroll_remainder.trunc() as i32;
        if lines == 0 {
            return;
        }
        self.scroll_remainder -= lines as f32;

        let mut session = self.terminal.session.lock();
        let term = session.active_term_mut();
        if term.mode().contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) {
            // In less, man, etc. there is no scrollback: the wheel
            // sends arrows so that the program scrolls itself.
            drop(session);
            let arrow: &[u8] = if lines > 0 { b"\x1bOA" } else { b"\x1bOB" };
            self.terminal.write(arrow.repeat(lines.unsigned_abs() as usize));
        } else {
            term.scroll_display(Scroll::Delta(lines));
            drop(session);
            cx.notify();
        }
    }

    // --- Building the frame ----------------------------------------------

    /// Fills the scene and returns the rectangle of the input cursor.
    fn build_scene(
        &mut self,
        bounds: Bounds<Pixels>,
        origin_x: Pixels,
        metrics: &Metrics,
        focused: bool,
        scene: &mut Scene,
    ) -> Bounds<Pixels> {
        let handle = self.terminal.session.clone();
        let mut session = handle.lock();
        let mode = session.mode();
        self.layout = Layout {
            mode: Some(mode),
            grid_origin: point(origin_x, bounds.top()),
            viewport_height: bounds.size.height.into(),
            ..Layout::default()
        };
        scene.clip = bounds;

        let cursor = match mode {
            Mode::Fullscreen => self.build_fullscreen(&session, bounds, origin_x, metrics, focused, scene),
            Mode::Running | Mode::Editing => {
                self.build_blocks(&mut session, mode, bounds, origin_x, metrics, focused, scene)
            }
        };
        drop(session);

        let cursor = cursor.unwrap_or(Bounds::new(
            point(origin_x, bounds.top()),
            size(metrics.cell_width, metrics.cell_height),
        ));

        // Text being composed (e.g. "^" before "e"), on the cursor.
        if let Some(marked) = self.marked_text.as_ref().filter(|text| !text.is_empty()) {
            let width = metrics.cell_width * marked.width().max(1) as f32;
            scene.free.quads_below.push(fill(
                Bounds::new(cursor.origin, size(width, metrics.cell_height)),
                hsla(self.palette.background()),
            ));
            scene.free.labels.push(Label {
                origin: cursor.origin,
                text: marked.clone(),
                color: hsla(self.palette.foreground()),
                bold: false,
                underline: true,
                on_grid: false,
            });
        }
        cursor
    }

    /// The active emulator over the whole surface, like a classic terminal.
    fn build_fullscreen(
        &self,
        session: &Session,
        bounds: Bounds<Pixels>,
        origin_x: Pixels,
        metrics: &Metrics,
        focused: bool,
        scene: &mut Scene,
    ) -> Option<Bounds<Pixels>> {
        let term = session.active_term();
        let content = term.renderable_content();
        let colors = RowColors { palette: &self.palette, overrides: content.colors };
        let display_offset = content.display_offset as i32;
        let cursor_shape = match content.cursor.shape {
            CursorShape::Hidden => None,
            _ if !focused => Some(CursorShape::HollowBlock),
            shape => Some(shape),
        };

        for row in 0..term.screen_lines() {
            let line = Line(row as i32 - display_offset);
            let grid_row = &term.grid()[line];
            let cursor = cursor_shape
                .filter(|_| content.cursor.point.line == line)
                .map(|shape| (content.cursor.point.column.0, shape));
            scene.clipped.push_row(
                bounds.top() + metrics.cell_height * row as f32,
                (0..grid_row.len()).map(|column| &grid_row[Column(column)]),
                &colors,
                |column| {
                    content
                        .selection
                        .is_some_and(|selection| selection.contains(GridPoint::new(line, Column(column))))
                },
                cursor,
            );
        }
        scene.clipped.cursor_bounds(origin_x, metrics)
    }

    /// The block list, top-aligned until it fills the pane, and the input
    /// editor right below it.
    #[allow(clippy::too_many_arguments)]
    fn build_blocks(
        &mut self,
        session: &mut Session,
        mode: Mode,
        bounds: Bounds<Pixels>,
        origin_x: Pixels,
        metrics: &Metrics,
        focused: bool,
        scene: &mut Scene,
    ) -> Option<Bounds<Pixels>> {
        let palette = self.palette.clone();
        let (cell_width, cell_height) = (metrics.cell_width, metrics.cell_height);
        let columns = self.terminal.size().columns;
        let background = palette.background();
        let foreground = hsla(palette.foreground());
        let muted = hsla(palette.muted);
        let accent = palette.cursor();
        let no_overrides = Colors::default();
        let red = palette.indexed(NamedColor::Red as usize, &no_overrides);
        let green = palette.indexed(NamedColor::Green as usize, &no_overrides);
        let border = hsla(mix(palette.foreground(), background, 0.12));
        let blocks_config = self.config.blocks;
        let (block_padding, header_gap) = if blocks_config.compact {
            (COMPACT_BLOCK_PADDING, COMPACT_HEADER_GAP)
        } else {
            (BLOCK_PADDING, HEADER_GAP)
        };

        // The editor is only displayed when the shell is waiting for a command.
        let show_editor = mode == Mode::Editing;
        let editor_columns = columns.saturating_sub(PROMPT_CELLS).max(2);
        let editor_rows = if show_editor { self.editor.layout(editor_columns) } else { Vec::new() };
        let editor_height = if show_editor {
            EDITOR_PADDING * 2. + cell_height * (1 + editor_rows.len()) as f32
        } else {
            px(0.)
        };
        // Height of each block, the finished ones first, then the running one.
        struct Measure {
            id: u64,
            command_rows: usize,
            rows: usize,
            height: Pixels,
        }
        let measure = |id: u64, command: &str, rows: usize| {
            let command_rows = command_row_count(command, columns);
            let mut height = block_padding * 2. + cell_height * rows as f32;
            if command_rows > 0 {
                height += cell_height * (1 + command_rows) as f32;
                if rows > 0 {
                    height += header_gap;
                }
            }
            Measure { id, command_rows, rows, height }
        };
        let mut measures = Vec::with_capacity(session.blocks.len() + 1);
        for block in &mut session.blocks {
            block.wrap(columns);
            measures.push(measure(block.id, &block.command, block.row_count()));
        }
        if let Some(live) = &session.live {
            measures.push(measure(live.id, &live.command, live.row_count()));
        }

        // The blocks fill the pane from the top and the editor follows the
        // last one: it only sticks to the bottom once the blocks no longer fit.
        let content_height: f32 = measures.iter().map(|measure| f32::from(measure.height)).sum();
        let available = (bounds.size.height - editor_height).max(px(0.));
        let viewport = Bounds::new(bounds.origin, size(bounds.size.width, available.min(px(content_height))));
        scene.clip = viewport;
        self.layout.viewport_height = viewport.size.height.into();

        // When the list was scrolled up, output added at the bottom must
        // not move what is being read.
        if self.scroll > 0.0 && content_height > self.content_height {
            self.scroll += content_height - self.content_height;
        }
        self.content_height = content_height;
        self.max_scroll = (content_height - f32::from(viewport.size.height)).max(0.0);
        self.scroll = self.scroll.clamp(0.0, self.max_scroll);

        let frozen_count = session.blocks.len();
        let layer = &mut scene.clipped;
        let mut bottom = viewport.bottom() + px(self.scroll);
        for (index, measure) in measures.iter().enumerate().rev() {
            let top = bottom - measure.height;
            if bottom <= viewport.top() {
                break;
            }
            if top >= viewport.bottom() {
                bottom = top;
                continue;
            }

            let source = match session.blocks.get(index) {
                Some(block) => Source::Frozen(block),
                None => Source::Live(session.live.as_ref().expect("running block was measured")),
            };
            debug_assert!(index <= frozen_count);
            let failed = source.failed();
            let selected = self.selected_block == Some(measure.id);

            // Background and edge bar: red for a failure, accent for the chosen block.
            let block_bounds = Bounds::new(
                point(viewport.left(), top),
                size(viewport.size.width, measure.height),
            );
            let tint = if selected {
                Some(accent)
            } else if failed {
                Some(red)
            } else {
                None
            };
            if let Some(tint) = tint {
                layer.quads_below.push(fill(block_bounds, hsla(mix(tint, background, 0.08))));
                layer.quads_above.push(fill(
                    Bounds::new(block_bounds.origin, size(px(3.), measure.height)),
                    hsla(tint),
                ));
            }
            if !blocks_config.compact {
                layer.quads_below.push(fill(
                    Bounds::new(block_bounds.origin, size(viewport.size.width, px(1.))),
                    border,
                ));
            }

            let mut y = top + block_padding;
            if measure.command_rows > 0 {
                layer.labels.push(Label {
                    origin: point(origin_x, y),
                    text: source.meta(blocks_config.show_duration),
                    color: if failed { hsla(red) } else { muted },
                    bold: false,
                    underline: false,
                    on_grid: false,
                });
                y += cell_height;
                for row in command_rows(source.command(), columns) {
                    layer.labels.push(Label {
                        origin: point(origin_x, y),
                        on_grid: is_single_width(&row),
                        text: row,
                        color: foreground,
                        bold: true,
                        underline: false,
                    });
                    y += cell_height;
                }
                if measure.rows > 0 {
                    y += header_gap;
                }
            }

            // Only the visible rows are drawn.
            let rows_top = y;
            let first = ((viewport.top() - rows_top) / cell_height).floor().max(0.0) as usize;
            let last = (((viewport.bottom() - rows_top) / cell_height).ceil().max(0.0) as usize)
                .min(measure.rows);
            let selection = self
                .block_selection
                .as_ref()
                .filter(|selection| selection.block == measure.id && selection.dragged)
                .map(BlockSelection::range);
            for row in first..last {
                let row_y = rows_top + cell_height * row as f32;
                let is_selected = |column: usize| {
                    selection.is_some_and(|(start, end)| (row, column) >= start && (row, column) <= end)
                };
                match source {
                    Source::Frozen(block) => {
                        let colors = RowColors { palette: &palette, overrides: &no_overrides };
                        layer.push_row(row_y, block.row(row).0.iter(), &colors, is_selected, None);
                    }
                    Source::Live(live) => {
                        let colors = RowColors { palette: &palette, overrides: live.term.colors() };
                        let cursor = live_cursor(live, focused).filter(|(cursor_row, ..)| *cursor_row == row);
                        layer.push_row(
                            row_y,
                            live.cells(row),
                            &colors,
                            is_selected,
                            cursor.map(|(_, column, shape)| (column, shape)),
                        );
                    }
                }
            }

            self.layout.blocks.push(BlockHit {
                id: measure.id,
                top,
                bottom,
                rows_top,
                rows: measure.rows,
            });
            bottom = top;
        }

        if !show_editor {
            return scene.clipped.cursor_bounds(origin_x, metrics);
        }

        // --- Input editor ---
        let layer = &mut scene.free;
        let editor_top = viewport.bottom();
        if !measures.is_empty() {
            layer.quads_below.push(fill(
                Bounds::new(point(bounds.left(), editor_top), size(bounds.size.width, px(1.))),
                border,
            ));
        }

        // Info line: current directory and git branch.
        let mut y = editor_top + EDITOR_PADDING;
        let cwd = session.cwd.clone();
        if self.git_branch.0 != cwd || self.git_branch.1 != frozen_count {
            let branch = cwd.as_deref().and_then(git::current_branch);
            self.git_branch = (cwd.clone(), frozen_count, branch);
        }
        let cwd_text = cwd.as_deref().map(display_path).unwrap_or_default();
        let blue = palette.indexed(NamedColor::Blue as usize, &no_overrides);
        let magenta = palette.indexed(NamedColor::Magenta as usize, &no_overrides);
        if let Some(branch) = &self.git_branch.2 {
            layer.labels.push(Label {
                origin: point(origin_x + cell_width * (cwd_text.width() + 2) as f32, y),
                text: format!("git:{branch}"),
                color: hsla(magenta),
                bold: false,
                underline: false,
                on_grid: false,
            });
        }
        layer.labels.push(Label {
            origin: point(origin_x, y),
            text: cwd_text,
            color: hsla(blue),
            bold: false,
            underline: false,
            on_grid: false,
        });
        y += cell_height;

        // The prompt symbol recalls the outcome of the last command.
        let prompt_color = match session.last_exit {
            Some(code) if code != 0 => red,
            _ => green,
        };
        layer.labels.push(Label {
            origin: point(origin_x, y),
            text: PROMPT_SYMBOL.to_owned(),
            color: hsla(prompt_color),
            bold: true,
            underline: false,
            on_grid: false,
        });

        // Syntax highlighting, recomputed only when the input, the
        // directory or the commands the shell knows change.
        let known = session.shell_commands.clone();
        let known_count = known.as_ref().map_or(0, |commands| commands.len());
        if self.highlighted.0 != self.editor.text()
            || self.highlighted.1 != cwd
            || self.highlighted.2 != known_count
        {
            let shell = ShellState {
                cwd: cwd.as_deref(),
                path: session.path.as_deref(),
                commands: known.as_deref(),
            };
            let lookup = highlight::Lookup {
                is_command: &|word| self.completion.is_command(word, &shell),
                path_exists: &|path| resolve_path(path, shell.cwd).is_some_and(|path| path.exists()),
            };
            let styles = highlight::highlight(self.editor.text(), &lookup);
            self.highlighted = (self.editor.text().to_owned(), cwd.clone(), known_count, styles);
        }
        let cyan = palette.indexed(NamedColor::Cyan as usize, &no_overrides);
        let yellow = palette.indexed(NamedColor::Yellow as usize, &no_overrides);
        let style_color = |style: Style| match style {
            Style::Plain | Style::Path => foreground,
            Style::Command => hsla(green),
            Style::UnknownCommand => hsla(red),
            Style::Option => hsla(cyan),
            Style::String => hsla(yellow),
            Style::Variable => hsla(magenta),
            Style::Operator => hsla(blue),
            Style::Comment => muted,
        };

        let text_x = origin_x + cell_width * PROMPT_CELLS as f32;
        let text = self.editor.text();
        let selection = self.editor.selection();
        let width_of = |range: Range<usize>| text[range].width();
        for (index, row) in editor_rows.iter().enumerate() {
            let row_y = y + cell_height * index as f32;
            if let Some(selection) = &selection {
                let start = selection.start.max(row.range.start);
                let end = selection.end.min(row.range.end);
                if start < end || (start == end && selection.end > row.range.end && start >= row.range.start) {
                    // A line whose line break is selected overflows by one
                    // cell, so that the selection stays visible.
                    let newline = usize::from(selection.end > row.range.end);
                    let from = width_of(row.range.start..start);
                    let cells = width_of(start..end.max(start)) + newline;
                    layer.quads_below.push(fill(
                        Bounds::new(
                            point(text_x + cell_width * from as f32, row_y),
                            size(cell_width * cells as f32, cell_height),
                        ),
                        hsla(palette.selection),
                    ));
                }
            }
            // One text per piece of the same style, placed at its column.
            for (range, style) in highlight::runs(&self.highlighted.3, row.range.clone()) {
                let piece = &text[range.clone()];
                let column = width_of(row.range.start..range.start);
                layer.labels.push(Label {
                    origin: point(text_x + cell_width * column as f32, row_y),
                    text: piece.to_owned(),
                    color: style_color(style),
                    bold: false,
                    underline: style == Style::Path,
                    on_grid: is_single_width(piece),
                });
            }
        }

        let (cursor_row, cursor_column) = self.editor.cursor_position(editor_columns);
        let cursor_bounds = Bounds::new(
            point(text_x + cell_width * cursor_column as f32, y + cell_height * cursor_row as f32),
            size(cell_width, cell_height),
        );
        layer.quads_above.push(fill(
            Bounds::new(cursor_bounds.origin, size(px(2.), cell_height)),
            if focused { hsla(accent) } else { muted },
        ));

        // Completion or history menu, above the editor, or below it when
        // the editor is near the top of the pane.
        if let Some(popup) = &self.popup {
            let view = match popup {
                Popup::Completion { completion, selected } => PopupView {
                    rows: completion
                        .candidates
                        .iter()
                        .map(|candidate| (candidate.label.clone(), candidate.detail.clone(), false))
                        .collect(),
                    selected: *selected,
                    header: None,
                    anchor_column: PROMPT_CELLS
                        + self.editor.position_of(completion.range.start, editor_columns).1,
                },
                Popup::History { query, results, selected } => PopupView {
                    rows: results
                        .iter()
                        .map(|result| {
                            (result.command.replace('\n', " ⏎ "), result.detail.clone(), result.failed)
                        })
                        .collect(),
                    selected: *selected,
                    header: Some(format!("History: {query}▏")),
                    anchor_column: 0,
                },
            };
            let editor_bottom = editor_top + editor_height;
            push_popup(layer, &view, origin_x, (bounds.top(), editor_top, editor_bottom), columns, metrics, &palette);
        }

        self.layout.editor_top = Some(editor_top);
        self.layout.editor_text_origin = point(text_x, y);
        self.layout.editor_columns = editor_columns;
        Some(cursor_bounds)
    }
}

/// The content of a menu to draw: rows (text, detail, failed?).
struct PopupView {
    rows: Vec<(String, String, bool)>,
    selected: usize,
    /// Title row (the current query, for the history).
    header: Option<String>,
    /// The column the menu aligns under.
    anchor_column: usize,
}

/// Draws a menu right above the editor, or right below it when the space
/// between the top of the pane and the editor is too small.
/// `(pane_top, editor_top, editor_bottom)` locate the editor.
fn push_popup(
    layer: &mut Layer,
    view: &PopupView,
    origin_x: Pixels,
    (pane_top, editor_top, editor_bottom): (Pixels, Pixels, Pixels),
    columns: usize,
    metrics: &Metrics,
    palette: &Palette,
) {
    let (cell_width, cell_height) = (metrics.cell_width, metrics.cell_height);
    let background = palette.background();
    let foreground = hsla(palette.foreground());
    let muted = hsla(palette.muted);
    let red = hsla(palette.indexed(NamedColor::Red as usize, &Colors::default()));

    // The window of visible rows follows the selection.
    let visible = view.rows.len().min(POPUP_ROWS);
    let first = (view.selected + 1).saturating_sub(visible);
    let rows = &view.rows[first..first + visible];

    // Width: what the text and its detail need, within the pane's limits.
    let label_width = rows.iter().map(|(label, ..)| label.width()).max().unwrap_or(0);
    let detail_width = rows.iter().map(|(_, detail, _)| detail.width()).max().unwrap_or(0);
    let header_width = view.header.as_ref().map_or(0, |header| header.width());
    let wanted = (label_width + if detail_width > 0 { detail_width + 3 } else { 0 }).max(header_width);
    let inner = wanted.clamp(18, columns.saturating_sub(2).max(18));
    let label_width = label_width.min(inner.saturating_sub(detail_width.min(inner / 2) + 3).max(8));
    let width = inner + 2;
    let column = view.anchor_column.saturating_sub(1).min(columns.saturating_sub(width));

    let empty = view.rows.is_empty();
    let lines = visible + usize::from(view.header.is_some()) + usize::from(empty);
    let padding = px(5.);
    let height = cell_height * lines as f32 + padding * 2.;
    let left = origin_x + cell_width * column as f32;
    let above = editor_top - px(6.) - height;
    let top = if above >= pane_top { above } else { editor_bottom + px(6.) };
    let panel = Bounds::new(point(left, top), size(cell_width * width as f32, height));
    layer.quads_below.push(
        fill(panel, hsla(mix(palette.foreground(), background, 0.07)))
            .corner_radii(px(6.))
            .border_widths(px(1.))
            .border_color(hsla(mix(palette.foreground(), background, 0.2))),
    );

    let text_x = left + cell_width;
    let mut y = top + padding;
    let label = |layer: &mut Layer, x: Pixels, y: Pixels, text: String, color| {
        layer.labels.push(Label {
            origin: point(x, y),
            on_grid: is_single_width(&text),
            text,
            color,
            bold: false,
            underline: false,
        });
    };

    if let Some(header) = &view.header {
        label(layer, text_x, y, truncate(header, inner), muted);
        y += cell_height;
    }
    if empty {
        label(layer, text_x, y, "No results".to_owned(), muted);
    }
    for (index, (text, detail, failed)) in rows.iter().enumerate() {
        if first + index == view.selected {
            layer.quads_below.push(
                fill(
                    Bounds::new(
                        point(left + px(3.), y),
                        size(panel.size.width - px(6.), cell_height),
                    ),
                    hsla(mix(palette.cursor(), background, 0.25)),
                )
                .corner_radii(px(4.)),
            );
        }
        label(layer, text_x, y, truncate(text, label_width), if *failed { red } else { foreground });
        if !detail.is_empty() {
            let room = inner.saturating_sub(label_width + 3);
            let detail_x = text_x + cell_width * (label_width + 3) as f32;
            label(layer, detail_x, y, truncate(detail, room), muted);
        }
        y += cell_height;
    }
}

/// Cuts a text to `cells` cells, with an ellipsis.
fn truncate(text: &str, cells: usize) -> String {
    if text.width() <= cells {
        return text.to_owned();
    }
    let mut width = 0;
    let mut cut = String::new();
    for ch in text.chars() {
        width += ch.width().unwrap_or(0);
        if width + 1 > cells {
            break;
        }
        cut.push(ch);
    }
    cut.push('…');
    cut
}

/// Detail of a history command: directory, age, failure.
fn history_detail(meta: &Meta) -> String {
    let mut parts = Vec::new();
    if let Some(cwd) = &meta.cwd {
        parts.push(display_path(Path::new(cwd)));
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    parts.push(format_ago(now.saturating_sub(meta.at)));
    if let Some(code) = meta.exit.filter(|code| *code != 0) {
        parts.push(format!("code {code}"));
    }
    parts.join(" · ")
}

fn format_ago(seconds: u64) -> String {
    match seconds {
        0..60 => "just now".to_owned(),
        60..3600 => format!("{} min ago", seconds / 60),
        3600..86_400 => format!("{} h ago", seconds / 3600),
        _ => format!("{} d ago", seconds / 86_400),
    }
}

/// A block to draw, finished or running.
#[derive(Clone, Copy)]
enum Source<'a> {
    Frozen(&'a Block),
    Live(&'a LiveBlock),
}

impl Source<'_> {
    fn command(&self) -> &str {
        match self {
            Source::Frozen(block) => &block.command,
            Source::Live(live) => &live.command,
        }
    }

    fn failed(&self) -> bool {
        matches!(self, Source::Frozen(block) if block.exit.is_some_and(|code| code != 0))
    }

    /// The header's info line: directory, duration (if `show_duration`),
    /// exit code.
    fn meta(&self, show_duration: bool) -> String {
        let (cwd, status) = match self {
            Source::Frozen(block) => {
                let mut status =
                    block.duration.filter(|_| show_duration).map(format_duration).unwrap_or_default();
                if let Some(code) = block.exit.filter(|code| *code != 0) {
                    if !status.is_empty() {
                        status.push_str("  ·  ");
                    }
                    status.push_str(&format!("code {code}"));
                }
                (&block.cwd, status)
            }
            Source::Live(live) => {
                (&live.cwd, if live.executing { "running…".to_owned() } else { String::new() })
            }
        };
        let cwd = cwd.as_deref().map(display_path).unwrap_or_default();
        match (cwd.is_empty(), status.is_empty()) {
            (false, false) => format!("{cwd}  ·  {status}"),
            (false, true) => cwd,
            (true, _) => status,
        }
    }
}

/// Row, column and shape of a running block's cursor, if it is visible.
fn live_cursor(live: &LiveBlock, focused: bool) -> Option<(usize, usize, CursorShape)> {
    let cursor = live.term.renderable_content().cursor;
    let shape = match cursor.shape {
        CursorShape::Hidden => return None,
        _ if !focused => CursorShape::HollowBlock,
        shape => shape,
    };
    let row = live.term.grid().history_size() as i32 + cursor.point.line.0;
    (row >= 0).then_some((row as usize, cursor.point.column.0, shape))
}

/// Number of rows a command takes in its block's header.
fn command_row_count(command: &str, columns: usize) -> usize {
    let columns = columns.max(1);
    command.lines().map(|line| line.chars().count().div_ceil(columns).max(1)).sum()
}

/// The command split into rows of at most `columns` characters.
fn command_rows(command: &str, columns: usize) -> Vec<String> {
    let columns = columns.max(1);
    let mut rows = Vec::new();
    for line in command.lines() {
        let chars: Vec<char> = line.chars().collect();
        if chars.is_empty() {
            rows.push(String::new());
        }
        rows.extend(chars.chunks(columns).map(|chunk| chunk.iter().collect::<String>()));
    }
    rows
}

fn is_single_width(text: &str) -> bool {
    text.chars().all(|ch| ch.width() == Some(1))
}

/// A path with the home directory shortened to `~`.
pub fn display_path(path: &Path) -> String {
    if let Some(home) = dirs::home_dir()
        && let Ok(relative) = path.strip_prefix(&home)
    {
        return if relative.as_os_str().is_empty() {
            "~".to_owned()
        } else {
            format!("~/{}", relative.display())
        };
    }
    path.display().to_string()
}

fn format_duration(duration: Duration) -> String {
    let seconds = duration.as_secs_f64();
    if seconds < 1.0 {
        format!("{} ms", duration.as_millis())
    } else if seconds < 60.0 {
        format!("{seconds:.1} s")
    } else {
        let total = duration.as_secs();
        format!("{} min {:02} s", total / 60, total % 60)
    }
}

impl gpui::EventEmitter<PaneEvent> for TerminalView {}

impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for TerminalView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let background = hsla(self.palette.background());
        let view = cx.entity();

        let surface = canvas(
            |_, _, _| (),
            move |bounds, _, window, cx| paint_terminal(&view, bounds, window, cx),
        )
        .size_full();

        div()
            .size_full()
            .bg(background)
            .key_context("Terminal")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_action(cx.listener(Self::clear_blocks))
            .on_action(cx.listener(Self::increase_font_size))
            .on_action(cx.listener(Self::decrease_font_size))
            .on_action(cx.listener(Self::reset_font_size))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            .child(surface)
    }
}

fn paint_terminal(view: &Entity<TerminalView>, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
    let (font, font_size, line_height, padding, palette, focus_handle) = {
        let this = view.read(cx);
        (
            this.font(),
            px(this.font_size),
            this.config.font.line_height,
            px(this.config.window.padding),
            this.palette.clone(),
            this.focus_handle.clone(),
        )
    };

    // Dimensions of a cell, then of the grid that fits in the area.
    let text_system = window.text_system().clone();
    let font_id = text_system.resolve_font(&font);
    let cell_width = text_system.em_advance(font_id, font_size).unwrap_or(font_size * 0.6);
    let cell_height = (font_size * line_height).ceil();
    let metrics = Metrics { font, font_size, cell_width, cell_height };

    let origin_x = bounds.left() + padding;
    let grid_size = GridSize {
        columns: (((bounds.size.width - padding * 2.) / cell_width).floor() as usize).max(2),
        lines: (((bounds.size.height - padding) / cell_height).floor() as usize).max(1),
        cell_width: cell_width.into(),
        cell_height: cell_height.into(),
    };
    let focused = focus_handle.is_focused(window) && window.is_window_active();

    let mut scene = Scene::default();
    let cursor_bounds = view.update(cx, |this, _| {
        // Selections in blocks are tracked by displayed row: they do
        // not survive a width change.
        if this.terminal.size().columns != grid_size.columns {
            this.block_selection = None;
        }
        this.terminal.resize(grid_size);
        this.build_scene(bounds, origin_x, &metrics, focused, &mut scene)
    });
    scene.paint(origin_x, &metrics, &palette, window, cx);

    window.handle_input(&focus_handle, TerminalInput { view: view.clone(), cursor_bounds }, cx);
}

/// Receives the typed text (accents, dead keys, IME, emoji) from the system.
struct TerminalInput {
    view: Entity<TerminalView>,
    cursor_bounds: Bounds<Pixels>,
}

impl InputHandler for TerminalInput {
    fn selected_text_range(&mut self, _: bool, _: &mut Window, _: &mut App) -> Option<UTF16Selection> {
        Some(UTF16Selection { range: 0..0, reversed: false })
    }

    fn marked_text_range(&mut self, _: &mut Window, cx: &mut App) -> Option<Range<usize>> {
        let marked = self.view.read(cx).marked_text.as_ref()?;
        Some(0..marked.encode_utf16().count())
    }

    fn text_for_range(
        &mut self,
        _: Range<usize>,
        _: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut App,
    ) -> Option<String> {
        None
    }

    fn replace_text_in_range(&mut self, _: Option<Range<usize>>, text: &str, _: &mut Window, cx: &mut App) {
        self.view.update(cx, |this, cx| {
            this.marked_text = None;
            this.insert_text(text, cx);
        });
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        new_text: &str,
        _: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut App,
    ) {
        self.view.update(cx, |this, cx| {
            this.marked_text = (!new_text.is_empty()).then(|| new_text.to_owned());
            cx.notify();
        });
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut App) {
        self.view.update(cx, |this, cx| {
            this.marked_text = None;
            cx.notify();
        });
    }

    fn bounds_for_range(&mut self, _: Range<usize>, _: &mut Window, _: &mut App) -> Option<Bounds<Pixels>> {
        Some(self.cursor_bounds)
    }

    fn character_index_for_point(&mut self, _: Point<Pixels>, _: &mut Window, _: &mut App) -> Option<usize> {
        None
    }

    /// Holding a key repeats it, instead of opening the macOS accent menu.
    fn apple_press_and_hold_enabled(&mut self) -> bool {
        false
    }
}
