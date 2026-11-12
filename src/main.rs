mod about;
mod block;
mod commands;
mod completion;
mod config;
mod editor;
mod git;
mod highlight;
mod history;
mod keys;
mod layout;
mod osc;
mod palette;
mod platform;
mod scene;
mod settings;
mod terminal;
mod themes;
mod update;
mod view;
mod workflows;
mod workspace;

use gpui::{
    App, Application, Bounds, KeyBinding, Menu, MenuItem, TitlebarOptions, WindowBounds,
    WindowOptions, point, prelude::*, px, size,
};

use crate::config::Config;
use crate::platform::pick;
use crate::view::{
    ClearBlocks, Copy, Cut, DecreaseFontSize, IncreaseFontSize, Paste, Redo, ResetFontSize,
    SelectAll, Undo,
};
use crate::workspace::{
    ActivateTab1, ActivateTab2, ActivateTab3, ActivateTab4, ActivateTab5, ActivateTab6,
    ActivateTab7, ActivateTab8, ActivateTab9, ClosePane, FocusNextPane, FocusPreviousPane,
    About, NewTab, NewWindow, NextTab, OpenConfig, OpenSettings, PreviousTab, Quit, SplitDown, SplitRight,
    TogglePalette, ToggleTabsLayout, Workspace,
};

/// The version RTerm displays. Published binaries carry the one of their
/// release (the `RTERM_VERSION` variable, set by CI); a local build keeps
/// the one from `Cargo.toml`, marked "dev".
pub const VERSION: &str = match option_env!("RTERM_VERSION") {
    Some(version) => version,
    None => concat!(env!("CARGO_PKG_VERSION"), "-dev"),
};

fn main() {
    // `rterm --version` answers without opening a window.
    if std::env::args().any(|argument| argument == "--version" || argument == "-V") {
        println!("RTerm {VERSION}");
        return;
    }

    let application = Application::new();
    // Clicking the Dock icon after closing the window opens a new one.
    application.on_reopen(|cx| open_window(None, cx));
    application.run(move |cx: &mut App| {
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.on_action(|_: &NewWindow, cx| open_window(None, cx));
        cx.on_action(|_: &About, cx| about::open(cx));
        cx.on_action(|_: &OpenSettings, cx| settings::open_window(cx));

        // The configuration is read once; Settings keeps it up to date.
        let (config, config_error) = Config::load();
        if let Some(error) = &config_error {
            eprintln!("rterm: config ignored ({error})");
        }
        let check_updates = config.updates.check;
        settings::init(config, cx);
        update::init(check_updates, cx);
        let terminal = Some("Terminal");
        let workspace = Some("Workspace");
        // Each shortcut has a macOS and a Linux form (see `platform::pick`).
        cx.bind_keys([
            KeyBinding::new(pick("cmd-q", "ctrl-shift-q"), Quit, None),
            KeyBinding::new(pick("cmd-n", "ctrl-shift-n"), NewWindow, None),
            // Window: tabs, panes, palette.
            KeyBinding::new(pick("cmd-t", "ctrl-shift-t"), NewTab, workspace),
            KeyBinding::new(pick("cmd-w", "ctrl-shift-w"), ClosePane, workspace),
            KeyBinding::new(pick("ctrl-tab", "ctrl-tab"), NextTab, workspace),
            KeyBinding::new(pick("ctrl-shift-tab", "ctrl-shift-tab"), PreviousTab, workspace),
            KeyBinding::new(pick("cmd-shift-]", "ctrl-pagedown"), NextTab, workspace),
            KeyBinding::new(pick("cmd-shift-[", "ctrl-pageup"), PreviousTab, workspace),
            KeyBinding::new(pick("cmd-d", "ctrl-shift-d"), SplitRight, workspace),
            KeyBinding::new(pick("cmd-shift-d", "ctrl-shift-e"), SplitDown, workspace),
            KeyBinding::new(pick("cmd-]", "ctrl-shift-]"), FocusNextPane, workspace),
            KeyBinding::new(pick("cmd-[", "ctrl-shift-["), FocusPreviousPane, workspace),
            KeyBinding::new(pick("cmd-alt-right", "ctrl-alt-right"), FocusNextPane, workspace),
            KeyBinding::new(pick("cmd-alt-left", "ctrl-alt-left"), FocusPreviousPane, workspace),
            KeyBinding::new(pick("cmd-shift-p", "ctrl-shift-p"), TogglePalette, workspace),
            KeyBinding::new(pick("cmd-p", "ctrl-shift-p"), TogglePalette, workspace),
            KeyBinding::new(pick("cmd-,", "ctrl-,"), OpenSettings, None),
            KeyBinding::new(pick("cmd-1", "alt-1"), ActivateTab1, workspace),
            KeyBinding::new(pick("cmd-2", "alt-2"), ActivateTab2, workspace),
            KeyBinding::new(pick("cmd-3", "alt-3"), ActivateTab3, workspace),
            KeyBinding::new(pick("cmd-4", "alt-4"), ActivateTab4, workspace),
            KeyBinding::new(pick("cmd-5", "alt-5"), ActivateTab5, workspace),
            KeyBinding::new(pick("cmd-6", "alt-6"), ActivateTab6, workspace),
            KeyBinding::new(pick("cmd-7", "alt-7"), ActivateTab7, workspace),
            KeyBinding::new(pick("cmd-8", "alt-8"), ActivateTab8, workspace),
            KeyBinding::new(pick("cmd-9", "alt-9"), ActivateTab9, workspace),
            // Terminal pane.
            KeyBinding::new(pick("cmd-c", "ctrl-shift-c"), Copy, terminal),
            KeyBinding::new(pick("cmd-v", "ctrl-shift-v"), Paste, terminal),
            KeyBinding::new(pick("cmd-x", "ctrl-shift-x"), Cut, terminal),
            KeyBinding::new(pick("cmd-a", "ctrl-shift-a"), SelectAll, terminal),
            KeyBinding::new(pick("cmd-z", "ctrl-shift-z"), Undo, terminal),
            KeyBinding::new(pick("cmd-shift-z", "ctrl-shift-y"), Redo, terminal),
            KeyBinding::new(pick("cmd-k", "ctrl-shift-k"), ClearBlocks, terminal),
            KeyBinding::new(pick("cmd-=", "ctrl-="), IncreaseFontSize, terminal),
            KeyBinding::new(pick("cmd-+", "ctrl-+"), IncreaseFontSize, terminal),
            KeyBinding::new(pick("cmd--", "ctrl--"), DecreaseFontSize, terminal),
            KeyBinding::new(pick("cmd-0", "ctrl-0"), ResetFontSize, terminal),
        ]);
        cx.set_menus(vec![
            Menu {
                name: "RTerm".into(),
                items: vec![
                    MenuItem::action("About RTerm", About),
                    MenuItem::action("Check for Updates…", update::CheckForUpdates),
                    MenuItem::separator(),
                    MenuItem::action("Settings…", OpenSettings),
                    MenuItem::action("Open Settings File", OpenConfig),
                    MenuItem::separator(),
                    MenuItem::action("Quit RTerm", Quit),
                ],
            },
            Menu {
                name: "Shell".into(),
                items: vec![
                    MenuItem::action("New Window", NewWindow),
                    MenuItem::action("New Tab", NewTab),
                    MenuItem::action("Split Right", SplitRight),
                    MenuItem::action("Split Down", SplitDown),
                    MenuItem::separator(),
                    MenuItem::action("Close Pane", ClosePane),
                ],
            },
            Menu {
                name: "Edit".into(),
                items: vec![
                    MenuItem::action("Undo", Undo),
                    MenuItem::action("Redo", Redo),
                    MenuItem::separator(),
                    MenuItem::action("Cut", Cut),
                    MenuItem::action("Copy", Copy),
                    MenuItem::action("Paste", Paste),
                    MenuItem::action("Select All", SelectAll),
                ],
            },
            Menu {
                name: "View".into(),
                items: vec![
                    MenuItem::action("Command Palette", TogglePalette),
                    MenuItem::action("Toggle Vertical Tabs", ToggleTabsLayout),
                    MenuItem::separator(),
                    MenuItem::action("Increase Font Size", IncreaseFontSize),
                    MenuItem::action("Decrease Font Size", DecreaseFontSize),
                    MenuItem::action("Reset Font Size", ResetFontSize),
                    MenuItem::separator(),
                    MenuItem::action("Clear Blocks", ClearBlocks),
                ],
            },
            Menu {
                name: "Window".into(),
                items: vec![
                    MenuItem::action("Next Tab", NextTab),
                    MenuItem::action("Previous Tab", PreviousTab),
                    MenuItem::separator(),
                    MenuItem::action("Next Pane", FocusNextPane),
                    MenuItem::action("Previous Pane", FocusPreviousPane),
                ],
            },
        ]);

        open_window(config_error, cx);
    });
}

/// Opens a window with one tab. `config_error` is shown in it: only the
/// first window gets it.
fn open_window(config_error: Option<String>, cx: &mut App) {
    let config = settings::global(cx).read(cx).config.clone();
    let bounds = Bounds::centered(
        None,
        size(px(config.window.width), px(config.window.height)),
        cx,
    );
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        // On macOS, the tabs share the title bar with the window buttons. On
        // Linux, the system draws its own title bar above them.
        titlebar: Some(TitlebarOptions {
            title: Some("RTerm".into()),
            appears_transparent: platform::IS_MAC,
            traffic_light_position: platform::IS_MAC.then(|| point(px(13.), px(12.))),
        }),
        window_min_size: Some(size(px(320.), px(200.))),
        ..Default::default()
    };

    cx.open_window(options, move |window, cx| {
        cx.new(|cx| Workspace::new(config_error, window, cx))
    })
    .expect("failed to open the window");
    cx.activate(true);
}
