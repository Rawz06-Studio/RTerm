//! The workspace: the window, its tabs, their panes and the command
//! palette.

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{
    Action, AnyElement, App, Axis, Context, Entity, FocusHandle, Focusable, Hsla, KeyDownEvent,
    MouseButton, MouseDownEvent, SharedString, Window, actions, div, prelude::*, px,
};

use crate::commands;
use crate::completion::Engine;
use crate::config::{self, Config, TabsLayout};
use crate::history::HistoryStore;
use crate::layout::Node;
use crate::palette::{Palette, hsla, mix};
use crate::themes;
use crate::update::{self, CheckForUpdates, InstallUpdate, RestartToUpdate, Status, Updater};
use crate::view::{
    ClearBlocks, Copy, DecreaseFontSize, IncreaseFontSize, PaneContext, PaneEvent, Paste,
    ResetFontSize, TerminalView,
};
use crate::workflows::{self, Workflow};

actions!(
    workspace,
    [
        Quit,
        About,
        NewWindow,
        NewTab,
        ClosePane,
        NextTab,
        PreviousTab,
        SplitRight,
        SplitDown,
        FocusNextPane,
        FocusPreviousPane,
        TogglePalette,
        OpenConfig,
        OpenWorkflows,
        ToggleTabsLayout,
        ActivateTab1,
        ActivateTab2,
        ActivateTab3,
        ActivateTab4,
        ActivateTab5,
        ActivateTab6,
        ActivateTab7,
        ActivateTab8,
        ActivateTab9
    ]
);

/// Height of the tab bar, which doubles as the title bar.
const TAB_BAR_HEIGHT: f32 = 38.0;
/// Room left for the macOS window buttons, to the left of the tabs.
const TRAFFIC_LIGHTS_WIDTH: f32 = 78.0;
/// Widths of a horizontal tab: they share the bar between these bounds.
const TAB_MIN_WIDTH: f32 = 72.0;
const TAB_MAX_WIDTH: f32 = 280.0;
/// Width of the vertical tab column.
const SIDEBAR_WIDTH: f32 = 220.0;
/// Number of commands visible at once in the palette.
const PALETTE_ROWS: usize = 9;

type Pane = Entity<TerminalView>;

struct Tab {
    root: Node<Pane>,
    /// The pane that has (or will get back) the keyboard in this tab.
    active_pane: Pane,
}

/// What a palette entry does.
enum Run {
    Action(Box<dyn Action>),
    Theme(String),
    Workflow(Workflow),
}

struct Command {
    label: String,
    hint: String,
    run: Run,
}

/// The command palette, when it is open.
struct PaletteUi {
    query: String,
    selected: usize,
    commands: Vec<Command>,
    focus_handle: FocusHandle,
}

impl PaletteUi {
    /// Indices of the commands matching the query.
    fn matches(&self) -> Vec<usize> {
        commands::matches(&self.query, self.commands.iter().map(|command| command.label.as_str()))
    }
}

pub struct Workspace {
    config: Rc<Config>,
    colors: Rc<Palette>,
    /// History and completion, shared by every pane.
    history: Rc<RefCell<HistoryStore>>,
    completion: Rc<Engine>,
    theme: String,
    tabs_layout: TabsLayout,
    tabs: Vec<Tab>,
    active: usize,
    palette: Option<PaletteUi>,
    /// The "About" window, when it is open.
    about: Option<FocusHandle>,
    window_title: String,
    /// The last error to show (unreadable config, invalid theme…).
    error: Option<SharedString>,
    updater: Entity<Updater>,
}

impl Workspace {
    pub fn new(
        config: Rc<Config>,
        config_error: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (theme, theme_error) =
            themes::resolve_or_default(&config.appearance.theme, &config.colors);
        let mut workspace = Workspace {
            colors: Rc::new(Palette::new(&theme)),
            history: Rc::new(RefCell::new(HistoryStore::load(config.history.record))),
            completion: Rc::new(Engine::load()),
            theme: config.appearance.theme.clone(),
            tabs_layout: config.appearance.tabs,
            config,
            tabs: Vec::new(),
            active: 0,
            palette: None,
            about: None,
            window_title: String::new(),
            updater: update::global(cx),
            error: config_error
                .map(|error| format!("Config ignored, using default values: {error}"))
                .or(theme_error)
                .map(Into::into),
        };

        // Redrawn when the update status changes; a failure is explained.
        cx.observe(&workspace.updater, |this, updater, cx| {
            if let Status::Failed(error) = &updater.read(cx).status {
                this.error = Some(format!("Update failed: {error}").into());
            }
            cx.notify();
        })
        .detach();

        // The test hooks below only drive the first window: a window opened
        // later (Dock icon, Cmd+N) starts clean.
        static FIRST_WINDOW: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);
        let first_window = FIRST_WINDOW.swap(false, std::sync::atomic::Ordering::Relaxed);

        // `RTERM_RUN`: commands launched in the first pane (demos, tests).
        let script = std::env::var("RTERM_RUN")
            .ok()
            .filter(|_| first_window)
            .map(|script| script.lines().rev().map(str::to_owned).collect())
            .unwrap_or_default();
        if let Some(pane) = workspace.open_pane(None, script, window, cx) {
            workspace.tabs.push(Tab { root: Node::Pane(pane.clone()), active_pane: pane });
        }

        // `RTERM_DEMO`: a comma-separated list of steps played at startup
        // (screenshots, tests). E.g. "split-right,theme:Nord".
        if let Some(demo) = std::env::var("RTERM_DEMO").ok().filter(|_| first_window) {
            cx.defer_in(window, move |this, window, cx| {
                for step in demo.split(',') {
                    match step.trim() {
                        "new-tab" => this.new_tab(&NewTab, window, cx),
                        "split-right" => this.split(Axis::Horizontal, window, cx),
                        "split-down" => this.split(Axis::Vertical, window, cx),
                        "previous-pane" => this.focus_pane(-1, window, cx),
                        "palette" => this.toggle_palette(&TogglePalette, window, cx),
                        "about" => this.toggle_about(&About, window, cx),
                        // Bypasses the action, so that nothing is written to the config.
                        "vertical-tabs" => this.tabs_layout = TabsLayout::Vertical,
                        other => {
                            if let Some(name) = other.strip_prefix("theme:") {
                                this.apply_theme(name, cx);
                            } else if other.starts_with("complete:")
                                || other.starts_with("history:")
                                || other.starts_with("input:") {
                                if let Some(pane) = this.active_tab().map(|tab| tab.active_pane.clone()) {
                                    pane.update(cx, |pane, cx| pane.demo(other, cx));
                                }
                            } else if let Some(name) = other.strip_prefix("workflow:") {
                                let found = workflows::load_all().0.into_iter().find(|w| w.name == name);
                                if let Some(workflow) = found {
                                    this.insert_workflow(&workflow, cx);
                                }
                            } else if let Some(query) = other.strip_prefix("type:")
                                && let Some(palette) = &mut this.palette
                            {
                                palette.query.push_str(query);
                            }
                        }
                    }
                }
            });
        }
        workspace
    }

    // --- Panes and tabs -----------------------------------------------------

    /// Launches a shell in a new pane. On failure, the error is displayed
    /// and the workspace is left as it is.
    fn open_pane(
        &mut self,
        cwd: Option<std::path::PathBuf>,
        script: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Pane> {
        let context = PaneContext {
            config: self.config.clone(),
            palette: self.colors.clone(),
            history: self.history.clone(),
            completion: self.completion.clone(),
        };
        let pane = TerminalView::open(context, cwd, script, window, cx);
        match pane {
            Ok(pane) => {
                cx.subscribe_in(&pane, window, |this, pane, event, window, cx| match event {
                    PaneEvent::Exited => this.close_pane(&pane.clone(), window, cx),
                })
                .detach();
                // Tab titles follow what happens in the panes.
                cx.observe(&pane, |_, _, cx| cx.notify()).detach();
                Some(pane)
            }
            Err(error) => {
                self.error = Some(format!("Failed to launch the shell: {error:#}").into());
                cx.notify();
                None
            }
        }
    }

    fn active_tab(&self) -> Option<&Tab> {
        self.tabs.get(self.active)
    }

    /// The active pane's directory, to open the next one in the same place.
    fn active_cwd(&self, cx: &App) -> Option<std::path::PathBuf> {
        self.active_tab()?.active_pane.read(cx).cwd()
    }

    fn all_panes(&self) -> Vec<Pane> {
        self.tabs.iter().flat_map(|tab| tab.root.panes()).collect()
    }

    /// Gives the keyboard to the active pane of the displayed tab.
    fn focus_active(&self, window: &mut Window, cx: &App) {
        if let Some(tab) = self.active_tab() {
            window.focus(&tab.active_pane.focus_handle(cx));
        }
    }

    fn new_tab(&mut self, _: &NewTab, window: &mut Window, cx: &mut Context<Self>) {
        let cwd = self.active_cwd(cx);
        if let Some(pane) = self.open_pane(cwd, Vec::new(), window, cx) {
            self.tabs.insert(
                (self.active + 1).min(self.tabs.len()),
                Tab { root: Node::Pane(pane.clone()), active_pane: pane },
            );
            self.active = (self.active + 1).min(self.tabs.len() - 1);
            self.focus_active(window, cx);
            cx.notify();
        }
    }

    fn activate_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index < self.tabs.len() {
            self.active = index;
            self.focus_active(window, cx);
            cx.notify();
        }
    }

    fn next_tab(&mut self, _: &NextTab, window: &mut Window, cx: &mut Context<Self>) {
        self.activate_tab((self.active + 1) % self.tabs.len().max(1), window, cx);
    }

    fn previous_tab(&mut self, _: &PreviousTab, window: &mut Window, cx: &mut Context<Self>) {
        let count = self.tabs.len().max(1);
        self.activate_tab((self.active + count - 1) % count, window, cx);
    }

    /// Closes a tab and all its panes. Closing the last tab closes the
    /// window; RTerm keeps running and the Dock icon opens a new one.
    fn close_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.tabs.len() {
            return;
        }
        self.tabs.remove(index);
        if self.tabs.is_empty() {
            window.remove_window();
            return;
        }
        if index < self.active || self.active >= self.tabs.len() {
            self.active = self.active.saturating_sub(1);
        }
        self.focus_active(window, cx);
        cx.notify();
    }

    fn close_pane(&mut self, pane: &Pane, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.tabs.iter().position(|tab| tab.root.panes().contains(pane)) else {
            return;
        };
        let tab = &mut self.tabs[index];
        if !tab.root.remove(pane) {
            // It was the tab's only pane.
            self.close_tab(index, window, cx);
            return;
        }
        if tab.active_pane == *pane {
            tab.active_pane = tab.root.panes().remove(0);
        }
        self.focus_active(window, cx);
        cx.notify();
    }

    fn close_active_pane(&mut self, _: &ClosePane, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(pane) = self.active_tab().map(|tab| tab.active_pane.clone()) {
            self.close_pane(&pane, window, cx);
        }
    }

    fn split(&mut self, axis: Axis, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.active_tab().map(|tab| tab.active_pane.clone()) else {
            return;
        };
        let cwd = self.active_cwd(cx);
        let Some(pane) = self.open_pane(cwd, Vec::new(), window, cx) else {
            return;
        };
        let tab = &mut self.tabs[self.active];
        tab.root.split(&target, axis, pane.clone());
        tab.active_pane = pane;
        self.focus_active(window, cx);
        cx.notify();
    }

    fn split_right(&mut self, _: &SplitRight, window: &mut Window, cx: &mut Context<Self>) {
        self.split(Axis::Horizontal, window, cx);
    }

    fn split_down(&mut self, _: &SplitDown, window: &mut Window, cx: &mut Context<Self>) {
        self.split(Axis::Vertical, window, cx);
    }

    /// Moves to the next (`step` = 1) or previous (-1) pane of the tab.
    fn focus_pane(&mut self, step: isize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        let panes = tab.root.panes();
        let current = panes.iter().position(|pane| *pane == tab.active_pane).unwrap_or(0);
        let next = (current as isize + step).rem_euclid(panes.len() as isize) as usize;
        tab.active_pane = panes[next].clone();
        self.focus_active(window, cx);
        cx.notify();
    }

    fn focus_next_pane(&mut self, _: &FocusNextPane, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_pane(1, window, cx);
    }

    fn focus_previous_pane(
        &mut self,
        _: &FocusPreviousPane,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_pane(-1, window, cx);
    }

    // --- Theme and configuration --------------------------------------------

    /// Applies a theme to every pane and remembers it in the config.
    fn set_theme(&mut self, name: &str, cx: &mut Context<Self>) {
        if self.apply_theme(name, cx) {
            self.error = config::save_theme(name)
                .err()
                .map(|error| format!("Theme applied but not saved: {error:#}").into());
        }
    }

    /// Applies a theme without touching the config. Returns `false` if it is
    /// missing or invalid (the error is then displayed).
    fn apply_theme(&mut self, name: &str, cx: &mut Context<Self>) -> bool {
        let theme = match themes::resolve(name, &self.config.colors) {
            Ok(theme) => theme,
            Err(error) => {
                self.error = Some(format!("{error:#}").into());
                cx.notify();
                return false;
            }
        };
        self.colors = Rc::new(Palette::new(&theme));
        self.theme = name.to_owned();
        for pane in self.all_panes() {
            let colors = self.colors.clone();
            pane.update(cx, |pane, cx| pane.set_palette(colors, cx));
        }
        cx.notify();
        true
    }

    fn open_config(&mut self, _: &OpenConfig, _: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = Config::path() else {
            return;
        };
        // `open -t` opens the file in the default text editor.
        if let Err(error) = std::process::Command::new("open").arg("-t").arg(&path).spawn() {
            self.error = Some(format!("Failed to open {}: {error}", path.display()).into());
            cx.notify();
        }
    }

    /// Opens or closes the "About" window.
    fn toggle_about(&mut self, _: &About, window: &mut Window, cx: &mut Context<Self>) {
        if self.about.take().is_some() {
            self.focus_active(window, cx);
        } else {
            let focus_handle = cx.focus_handle();
            window.focus(&focus_handle);
            self.about = Some(focus_handle);
        }
        cx.notify();
    }

    fn render_about(&self, focus_handle: &FocusHandle, cx: &mut Context<Self>) -> AnyElement {
        let background = self.colors.background();
        let foreground = hsla(self.colors.foreground());
        let muted = hsla(self.colors.muted);
        let accent = hsla(self.colors.cursor());
        let config_path = Config::path()
            .map(|path| crate::view::display_path(&path))
            .unwrap_or_default();
        let line = |label: &'static str, value: String| {
            div()
                .flex()
                .gap_3()
                .child(div().w(px(92.)).flex_none().text_color(muted).child(label))
                .child(div().min_w_0().child(value))
        };

        // A veil over the whole window: a click or a key press closes it.
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .occlude()
            .flex()
            .items_center()
            .justify_center()
            .bg(hsla(background).opacity(0.6))
            .track_focus(focus_handle)
            .on_key_down(cx.listener(|this, _, window, cx| {
                cx.stop_propagation();
                this.toggle_about(&About, window, cx);
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| this.toggle_about(&About, window, cx)),
            )
            .child(
                div()
                    .w(px(440.))
                    .p_6()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .bg(hsla(mix(self.colors.foreground(), background, 0.06)))
                    .border_1()
                    .border_color(hsla(mix(self.colors.foreground(), background, 0.16)))
                    .rounded_lg()
                    .shadow_lg()
                    .text_size(px(13.))
                    .text_color(foreground)
                    .child(
                        div()
                            .flex()
                            .items_baseline()
                            .gap_3()
                            .child(div().text_size(px(26.)).text_color(accent).child("❯"))
                            .child(div().text_size(px(26.)).child("RTerm"))
                            .child(div().text_color(muted).child(format!("version {}", crate::VERSION))),
                    )
                    .child(
                        "A modern, fully local terminal: no account, no cloud, no AI. \
                         Everything you type stays on your machine.",
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(line("Version", crate::VERSION.to_owned()))
                            .child(line("Config", config_path))
                            .child(line("Theme", self.theme.clone()))
                            .child(line("Updates", self.updater.read(cx).summary()))
                            .child(line("License", "MIT".to_owned())),
                    )
                    .child(div().text_color(muted).text_size(px(12.)).child("Click or press a key to close")),
            )
            .into_any_element()
    }

    fn open_workflows(&mut self, _: &OpenWorkflows, _: &mut Window, cx: &mut Context<Self>) {
        // Loading creates the directory and its examples if they do not exist.
        workflows::load_all();
        let Some(directory) = workflows::directory() else {
            return;
        };
        if let Err(error) = std::process::Command::new("open").arg(&directory).spawn() {
            self.error = Some(format!("Failed to open {}: {error}", directory.display()).into());
            cx.notify();
        }
    }

    /// Puts a workflow into the active pane's editor.
    fn insert_workflow(&mut self, workflow: &Workflow, cx: &mut Context<Self>) {
        if let Some(pane) = self.active_tab().map(|tab| tab.active_pane.clone()) {
            let (text, fields) = workflow.expand();
            pane.update(cx, |pane, cx| pane.insert_workflow(&text, fields, cx));
        }
    }

    // --- Command palette ----------------------------------------------------

    fn commands(&mut self, cx: &App) -> Vec<Command> {
        let action = |label: &str, hint: &str, action: Box<dyn Action>| Command {
            label: label.to_owned(),
            hint: hint.to_owned(),
            run: Run::Action(action),
        };
        let mut commands = vec![
            action("New Tab", "⌘T", Box::new(NewTab)),
            action("Split Right", "⌘D", Box::new(SplitRight)),
            action("Split Down", "⇧⌘D", Box::new(SplitDown)),
            action("Close Pane", "⌘W", Box::new(ClosePane)),
            action("Next Tab", "⌃⇥", Box::new(NextTab)),
            action("Previous Tab", "⌃⇧⇥", Box::new(PreviousTab)),
            action("Next Pane", "⌘]", Box::new(FocusNextPane)),
            action("Previous Pane", "⌘[", Box::new(FocusPreviousPane)),
            action("Clear Blocks", "⌘K", Box::new(ClearBlocks)),
            action("Copy", "⌘C", Box::new(Copy)),
            action("Paste", "⌘V", Box::new(Paste)),
            action("Increase Font Size", "⌘+", Box::new(IncreaseFontSize)),
            action("Decrease Font Size", "⌘-", Box::new(DecreaseFontSize)),
            action("Reset Font Size", "⌘0", Box::new(ResetFontSize)),
            action(
                match self.tabs_layout {
                    TabsLayout::Horizontal => "Use Vertical Tabs (left column)",
                    TabsLayout::Vertical => "Use Horizontal Tabs (top bar)",
                },
                "",
                Box::new(ToggleTabsLayout),
            ),
            action("Open Configuration", "⌘,", Box::new(OpenConfig)),
            action("Open Workflows Folder", "", Box::new(OpenWorkflows)),
            action("About RTerm", "", Box::new(About)),
            action("New Window", "⌘N", Box::new(NewWindow)),
            action("Check for Updates", "", Box::new(CheckForUpdates)),
            action("Quit RTerm", "⌘Q", Box::new(Quit)),
        ];
        match &self.updater.read(cx).status {
            Status::Available(release) => commands.push(action(
                &format!("Install Update ({})", release.version),
                "",
                Box::new(InstallUpdate),
            )),
            Status::Ready(_) => commands.push(action("Restart to Update", "", Box::new(RestartToUpdate))),
            _ => {}
        }
        commands.extend(themes::all().into_iter().map(|theme| Command {
            label: format!("Theme: {}", theme.name),
            hint: if theme.name == self.theme { "current".to_owned() } else { String::new() },
            run: Run::Theme(theme.name),
        }));

        // Reloaded on every open: a workflow you just wrote is there.
        let (found, errors) = workflows::load_all();
        if !errors.is_empty() {
            self.error = Some(format!("Unreadable workflow: {}", errors.join("; ")).into());
        }
        commands.extend(found.into_iter().map(|workflow| Command {
            label: format!("Workflow: {}", workflow.name),
            hint: workflow.description.clone(),
            run: Run::Workflow(workflow),
        }));
        commands
    }

    fn toggle_palette(&mut self, _: &TogglePalette, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.is_some() {
            self.close_palette(window, cx);
            return;
        }
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle);
        self.palette = Some(PaletteUi {
            query: String::new(),
            selected: 0,
            commands: self.commands(cx),
            focus_handle,
        });
        cx.notify();
    }

    fn close_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.palette = None;
        self.focus_active(window, cx);
        cx.notify();
    }

    /// Runs the command at `index` (in the full list).
    fn run_command(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(mut palette) = self.palette.take() else {
            return;
        };
        // The keyboard goes back to the pane first: it is the one receiving
        // actions such as Copy or Clear Blocks.
        self.focus_active(window, cx);
        cx.notify();
        if index >= palette.commands.len() {
            return;
        }
        match palette.commands.swap_remove(index).run {
            Run::Action(action) => window.dispatch_action(action, cx),
            Run::Theme(name) => self.set_theme(&name, cx),
            Run::Workflow(workflow) => self.insert_workflow(&workflow, cx),
        }
    }

    fn palette_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        let keystroke = &event.keystroke;
        let Some(palette) = &mut self.palette else {
            return;
        };
        let matches = palette.matches();
        match keystroke.key.as_str() {
            "escape" => {
                self.close_palette(window, cx);
                return;
            }
            "enter" => {
                if let Some(&index) = matches.get(palette.selected) {
                    self.run_command(index, window, cx);
                }
                return;
            }
            "up" => {
                palette.selected = palette.selected.checked_sub(1).unwrap_or(matches.len().saturating_sub(1));
            }
            "down" => {
                palette.selected = if palette.selected + 1 >= matches.len() { 0 } else { palette.selected + 1 };
            }
            "backspace" if keystroke.modifiers.platform => {
                palette.query.clear();
                palette.selected = 0;
            }
            "backspace" => {
                palette.query.pop();
                palette.selected = 0;
            }
            _ => {
                let typed = keystroke
                    .key_char
                    .as_ref()
                    .filter(|_| !keystroke.modifiers.platform && !keystroke.modifiers.control);
                if let Some(text) = typed {
                    palette.query.push_str(text);
                    palette.selected = 0;
                }
            }
        }
        cx.notify();
    }

    // --- Rendering ----------------------------------------------------------

    /// A tab. In the vertical layout it takes the full width of the column;
    /// in the horizontal one, tabs share the bar, up to a maximum width.
    fn render_tab(&self, index: usize, title: &str, cx: &mut Context<Self>) -> AnyElement {
        let background = self.colors.background();
        let foreground = hsla(self.colors.foreground());
        let muted = hsla(self.colors.muted);
        let active_background = hsla(mix(self.colors.foreground(), background, 0.10));
        let hover_background = hsla(mix(self.colors.foreground(), background, 0.05));
        let is_active = index == self.active;
        let vertical = self.tabs_layout == TabsLayout::Vertical;

        div()
            .id(("tab", index))
            .group("tab")
            .pl_3()
            .pr_2()
            .flex()
            .items_center()
            .justify_between()
            .gap_2()
            .rounded_md()
            .text_size(px(12.5))
            .when(vertical, |tab| tab.h(px(30.)).flex_none())
            .when(!vertical, |tab| {
                tab.h(px(26.)).flex_1().min_w(px(TAB_MIN_WIDTH)).max_w(px(TAB_MAX_WIDTH))
            })
            .when(is_active, |tab| tab.bg(active_background).text_color(foreground))
            .when(!is_active, |tab| tab.text_color(muted).hover(|style| style.bg(hover_background)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| this.activate_tab(index, window, cx)),
            )
            .child(div().flex_1().min_w_0().truncate().child(title.to_owned()))
            .child(
                div()
                    .id(("close-tab", index))
                    .flex_none()
                    .px_1()
                    .rounded_sm()
                    .text_color(muted)
                    .opacity(0.)
                    .group_hover("tab", |style| style.opacity(1.))
                    .hover(|style| style.text_color(foreground).bg(hover_background))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.close_tab(index, window, cx);
                        }),
                    )
                    .child("×"),
            )
            .into_any_element()
    }

    fn render_new_tab_button(&self, label: &'static str, cx: &mut Context<Self>) -> impl IntoElement {
        let foreground = hsla(self.colors.foreground());
        let hover_background = hsla(mix(self.colors.foreground(), self.colors.background(), 0.05));
        div()
            .id("new-tab")
            .h(px(26.))
            .px_3()
            .flex()
            .flex_none()
            .items_center()
            .rounded_md()
            .text_size(px(12.5))
            .text_color(hsla(self.colors.muted))
            .hover(|style| style.text_color(foreground).bg(hover_background))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| this.new_tab(&NewTab, window, cx)),
            )
            .child(label)
    }

    /// Tabs at the top of the window, next to the macOS buttons.
    fn render_tab_bar(&self, titles: &[String], cx: &mut Context<Self>) -> impl IntoElement {
        let tabs: Vec<_> =
            titles.iter().enumerate().map(|(index, title)| self.render_tab(index, title, cx)).collect();
        div()
            .h(px(TAB_BAR_HEIGHT))
            .flex_none()
            .flex()
            .items_center()
            .gap_1()
            .pl(px(TRAFFIC_LIGHTS_WIDTH))
            .pr_2()
            .children(tabs)
            .child(self.render_new_tab_button("+", cx))
            // The rest of the bar is used to move the window.
            .child(drag_region().flex_1().min_w(px(40.)).h_full())
            .children(self.render_update_badge(cx))
    }

    /// A pill that announces an update and installs it when clicked.
    fn render_update_badge(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let updater = self.updater.read(cx);
        let text = updater.badge()?;
        let background = self.colors.background();
        let tint = match updater.status {
            Status::Failed(_) => self.colors.indexed(1, &Default::default()),
            Status::Checking | Status::UpToDate | Status::Installing(_) => self.colors.muted,
            _ => self.colors.cursor(),
        };
        let updater = self.updater.clone();
        Some(
            div()
                .id("update")
                .flex_none()
                .h(px(24.))
                .px_3()
                .flex()
                .items_center()
                .rounded_full()
                .text_size(px(12.))
                .text_color(hsla(tint))
                .bg(hsla(mix(tint, background, 0.14)))
                .hover(|style| style.bg(hsla(mix(tint, background, 0.24))))
                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                    cx.stop_propagation();
                    updater.update(cx, |updater, cx| updater.activate(cx));
                })
                .child(text)
                .into_any_element(),
        )
    }

    /// Tabs stacked in a column on the left.
    fn render_sidebar(&self, titles: &[String], cx: &mut Context<Self>) -> impl IntoElement {
        let border = hsla(mix(self.colors.foreground(), self.colors.background(), 0.14));
        let tabs: Vec<_> =
            titles.iter().enumerate().map(|(index, title)| self.render_tab(index, title, cx)).collect();
        div()
            .w(px(SIDEBAR_WIDTH))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(border)
            // The macOS buttons take the top of the column.
            .child(drag_region().h(px(TAB_BAR_HEIGHT)).flex_none())
            .child(
                div()
                    .id("tabs")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_2()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .children(tabs)
                    .child(self.render_new_tab_button("+ New Tab", cx)),
            )
            .children(self.render_update_badge(cx).map(|badge| div().flex_none().p_2().child(badge)))
    }

    /// A strip above the content when the tabs are on the left: it shows
    /// the title and is used to move the window.
    fn render_title_strip(&self, title: &str) -> impl IntoElement {
        drag_region()
            .h(px(TAB_BAR_HEIGHT))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(12.5))
            .text_color(hsla(self.colors.muted))
            .child(title.to_owned())
    }

    fn toggle_tabs_layout(&mut self, _: &ToggleTabsLayout, _: &mut Window, cx: &mut Context<Self>) {
        self.tabs_layout = match self.tabs_layout {
            TabsLayout::Horizontal => TabsLayout::Vertical,
            TabsLayout::Vertical => TabsLayout::Horizontal,
        };
        self.error = config::save_tabs_layout(self.tabs_layout)
            .err()
            .map(|error| format!("Layout applied but not saved: {error:#}").into());
        cx.notify();
    }

    fn render_palette(&self, palette: &PaletteUi, cx: &mut Context<Self>) -> impl IntoElement {
        let background = self.colors.background();
        let foreground = hsla(self.colors.foreground());
        let muted = hsla(self.colors.muted);
        let accent = self.colors.cursor();
        let panel = hsla(mix(self.colors.foreground(), background, 0.06));
        let border = hsla(mix(self.colors.foreground(), background, 0.16));
        let selected_background = hsla(mix(accent, background, 0.22));

        let matches = palette.matches();
        let selected = palette.selected.min(matches.len().saturating_sub(1));
        // The list scrolls to keep the selection visible.
        let first = (selected + 1).saturating_sub(PALETTE_ROWS);
        let rows = matches.iter().enumerate().skip(first).take(PALETTE_ROWS).map(|(position, &index)| {
            let command = &palette.commands[index];
            div()
                .id(("command", index))
                .mx_1()
                .px_3()
                .h(px(30.))
                .flex()
                .items_center()
                .justify_between()
                .rounded_md()
                .when(position == selected, |row| row.bg(selected_background))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, window, cx| this.run_command(index, window, cx)),
                )
                .child(command.label.clone())
                .child(div().text_color(muted).text_size(px(12.)).child(command.hint.clone()))
        });

        let query = if palette.query.is_empty() {
            div().text_color(muted).child("Search commands…")
        } else {
            div().child(palette.query.clone())
        };

        // A veil over the whole window: a click outside closes the palette.
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| this.close_palette(window, cx)),
            )
            .child(
                div().absolute().top(px(64.)).left_0().w_full().flex().justify_center().child(
                    div()
                        .w(px(540.))
                        .pb_1()
                        .bg(panel)
                        .border_1()
                        .border_color(border)
                        .rounded_lg()
                        .shadow_lg()
                        .text_size(px(13.))
                        .text_color(foreground)
                        .track_focus(&palette.focus_handle)
                        .on_key_down(cx.listener(Self::palette_key))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .h(px(42.))
                                .px_4()
                                .mb_1()
                                .flex()
                                .items_center()
                                .gap_2()
                                .border_b_1()
                                .border_color(border)
                                .child(div().text_color(hsla(accent)).child("❯"))
                                .child(query),
                        )
                        .children(rows)
                        .when(matches.is_empty(), |panel| {
                            panel.child(
                                div().px_4().h(px(30.)).flex().items_center().text_color(muted).child("No matching command"),
                            )
                        }),
                ),
            )
    }
}

/// An empty area that moves the window when dragged and zooms it on a
/// double click, like a title bar.
fn drag_region() -> gpui::Div {
    div().on_mouse_down(MouseButton::Left, |event: &MouseDownEvent, window, _| {
        if event.click_count == 2 {
            window.titlebar_double_click();
        } else {
            window.start_window_move();
        }
    })
}

/// Draws a tree of panes. The inactive pane is dimmed as soon as there are
/// several, to see at a glance which one receives the keyboard.
fn render_node(node: &Node<Pane>, active: &Pane, multiple: bool, border: Hsla) -> AnyElement {
    match node {
        Node::Pane(pane) => div()
            .size_full()
            .when(multiple && pane != active, |container| container.opacity(0.55))
            .child(pane.clone())
            .into_any_element(),
        Node::Split { axis, children } => {
            let container = match axis {
                Axis::Horizontal => div().flex().flex_row(),
                Axis::Vertical => div().flex().flex_col(),
            };
            container
                .size_full()
                .children(children.iter().enumerate().map(|(index, child)| {
                    div()
                        .flex_1()
                        .min_w_0()
                        .min_h_0()
                        .border_color(border)
                        .when(index > 0, |zone| match axis {
                            Axis::Horizontal => zone.border_l_1(),
                            Axis::Vertical => zone.border_t_1(),
                        })
                        .child(render_node(child, active, multiple, border))
                }))
                .into_any_element()
        }
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // A click in a pane gives it the keyboard: it becomes the active one.
        if let Some(tab) = self.tabs.get_mut(self.active)
            && let Some(focused) = tab
                .root
                .panes()
                .into_iter()
                .find(|pane| pane.focus_handle(cx).is_focused(window))
        {
            tab.active_pane = focused;
        }

        let titles: Vec<String> =
            self.tabs.iter().map(|tab| tab.active_pane.read(cx).tab_title()).collect();
        let title = titles.get(self.active).cloned().unwrap_or_default();
        if title != self.window_title {
            window.set_window_title(&title);
            self.window_title = title;
        }

        let background = self.colors.background();
        let border = hsla(mix(self.colors.foreground(), background, 0.14));
        let red = self.colors.indexed(1, &Default::default());

        let content = self.active_tab().map(|tab| {
            let multiple = matches!(tab.root, Node::Split { .. });
            render_node(&tab.root, &tab.active_pane, multiple, border)
        });

        macro_rules! activate_tab {
            ($root:expr, $($action:ident => $index:expr),+) => {
                $root$(.on_action(cx.listener(|this, _: &$action, window, cx| {
                    this.activate_tab($index, window, cx)
                })))+
            };
        }

        let root = div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(hsla(background))
            .text_color(hsla(self.colors.foreground()))
            .key_context("Workspace")
            .on_action(cx.listener(Self::new_tab))
            .on_action(cx.listener(Self::close_active_pane))
            .on_action(cx.listener(Self::next_tab))
            .on_action(cx.listener(Self::previous_tab))
            .on_action(cx.listener(Self::split_right))
            .on_action(cx.listener(Self::split_down))
            .on_action(cx.listener(Self::focus_next_pane))
            .on_action(cx.listener(Self::focus_previous_pane))
            .on_action(cx.listener(Self::toggle_palette))
            .on_action(cx.listener(Self::open_config))
            .on_action(cx.listener(Self::open_workflows))
            .on_action(cx.listener(Self::toggle_tabs_layout))
            .on_action(cx.listener(Self::toggle_about));
        let root = activate_tab!(
            root,
            ActivateTab1 => 0, ActivateTab2 => 1, ActivateTab3 => 2,
            ActivateTab4 => 3, ActivateTab5 => 4, ActivateTab6 => 5,
            ActivateTab7 => 6, ActivateTab8 => 7, ActivateTab9 => 8
        );

        let vertical = self.tabs_layout == TabsLayout::Vertical;
        let main = div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .when(vertical, |main| {
                main.child(self.render_title_strip(titles.get(self.active).map_or("", String::as_str)))
            })

            .children(self.error.clone().map(|error| {
                div()
                    .id("error")
                    .flex_none()
                    .mx_3()
                    .mb_2()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .bg(hsla(mix(red, background, 0.2)))
                    .text_size(px(12.))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.error = None;
                            cx.notify();
                        }),
                    )
                    .child(format!("{error}  (click to dismiss)"))
            }))
            .child(div().flex_1().min_h_0().border_t_1().border_color(border).children(content));

        let root = if vertical {
            root.flex_row().child(self.render_sidebar(&titles, cx)).child(main)
        } else {
            root.child(self.render_tab_bar(&titles, cx)).child(main)
        };
        root.children(self.palette.as_ref().map(|palette| self.render_palette(palette, cx)))
            .children(self.about.clone().map(|focus_handle| self.render_about(&focus_handle, cx)))
    }
}
