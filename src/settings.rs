//! Settings shared by every window, and the Settings window that edits them.
//!
//! The configuration is loaded once at launch and kept in a global entity.
//! A change made in the Settings window (or from the palette) updates it,
//! is saved to `config.toml`, and every window observing it follows.

use std::rc::Rc;

use gpui::{
    App, AppContext as _, Bounds, Context, Entity, FocusHandle, Global, KeyBinding, KeyDownEvent,
    MouseButton, Pixels, Render, Size, Subscription, TitlebarOptions, Window, WindowAppearance,
    WindowBounds, WindowHandle, WindowKind, WindowOptions, actions, div, prelude::*, px, size,
};

use crate::config::{self, Config, TabsLayout, ThemeMode};
use crate::palette::{Palette, hsla, mix};
use crate::themes;

actions!(rterm, [ClosePanel]);

/// A change made in the Settings window or the palette.
#[derive(Clone, Copy)]
pub enum Setting {
    ThemeMode(ThemeMode),
    /// Moves a theme choice to the next (1) or previous (-1) theme.
    CycleTheme(ThemeSlot, isize),
    Tabs(TabsLayout),
    /// Font size change, in points.
    FontSize(f32),
    ShowDuration(bool),
    Compact(bool),
    CheckUpdates(bool),
}

/// Which theme choice a setting changes.
#[derive(Clone, Copy)]
pub enum ThemeSlot {
    Manual,
    Light,
    Dark,
}

pub struct Settings {
    pub config: Rc<Config>,
    /// The last save that failed, shown until a save succeeds.
    pub error: Option<String>,
}

struct GlobalSettings {
    settings: Entity<Settings>,
    /// The Settings window, while it is open: there is only ever one.
    window: Option<WindowHandle<SettingsWindow>>,
}

impl Global for GlobalSettings {}

pub fn init(config: Config, cx: &mut App) {
    let settings = cx.new(|_| Settings { config: Rc::new(config), error: None });
    cx.set_global(GlobalSettings { settings, window: None });
    // Cmd+W (Ctrl+Shift+W on Linux) closes the small windows.
    cx.bind_keys([KeyBinding::new(crate::platform::pick("cmd-w", "ctrl-shift-w"), ClosePanel, Some("Panel"))]);
}

pub fn global(cx: &App) -> Entity<Settings> {
    cx.global::<GlobalSettings>().settings.clone()
}

/// Applies a change from anywhere in the app.
pub fn change(setting: Setting, cx: &mut App) {
    global(cx).update(cx, |settings, cx| settings.change(setting, cx));
}

impl Settings {
    /// Changes the config in memory, saves `saved` to the config file and
    /// tells the windows.
    fn update(
        &mut self,
        change: impl FnOnce(&mut Config),
        saved: Vec<(&str, &str, toml_edit::Value)>,
        cx: &mut Context<Self>,
    ) {
        let mut config = (*self.config).clone();
        change(&mut config);
        self.config = Rc::new(config);
        self.error = config::save(&saved).err().map(|error| format!("Setting applied but not saved: {error:#}"));
        cx.notify();
    }

    /// A theme picked in the palette: it is used from now on, whatever the
    /// system's appearance ("manual" mode).
    pub fn set_manual_theme(&mut self, name: &str, cx: &mut Context<Self>) {
        let name = name.to_owned();
        self.update(
            |config| {
                config.appearance.theme_mode = ThemeMode::Manual;
                config.appearance.theme = name.clone();
            },
            vec![
                ("appearance", "theme_mode", ThemeMode::Manual.as_str().into()),
                ("appearance", "theme", name.as_str().into()),
            ],
            cx,
        );
    }

    pub fn change(&mut self, setting: Setting, cx: &mut Context<Self>) {
        match setting {
            Setting::ThemeMode(mode) => self.update(
                |config| config.appearance.theme_mode = mode,
                vec![("appearance", "theme_mode", mode.as_str().into())],
                cx,
            ),
            Setting::CycleTheme(slot, step) => {
                let names: Vec<String> = themes::all().into_iter().map(|theme| theme.name).collect();
                if names.is_empty() {
                    return;
                }
                let appearance = &self.config.appearance;
                let (key, current) = match slot {
                    ThemeSlot::Manual => ("theme", &appearance.theme),
                    ThemeSlot::Light => ("light_theme", &appearance.light_theme),
                    ThemeSlot::Dark => ("dark_theme", &appearance.dark_theme),
                };
                let index = names.iter().position(|name| name == current).unwrap_or(0) as isize;
                let next = names[(index + step).rem_euclid(names.len() as isize) as usize].clone();
                self.update(
                    |config| {
                        let appearance = &mut config.appearance;
                        match slot {
                            ThemeSlot::Manual => appearance.theme = next.clone(),
                            ThemeSlot::Light => appearance.light_theme = next.clone(),
                            ThemeSlot::Dark => appearance.dark_theme = next.clone(),
                        }
                    },
                    vec![("appearance", key, next.as_str().into())],
                    cx,
                );
            }
            Setting::Tabs(layout) => self.update(
                |config| config.appearance.tabs = layout,
                vec![("appearance", "tabs", layout.as_str().into())],
                cx,
            ),
            Setting::FontSize(step) => {
                let size = (self.config.font.size + step).clamp(6.0, 48.0);
                self.update(
                    |config| config.font.size = size,
                    vec![("font", "size", f64::from(size).into())],
                    cx,
                );
            }
            Setting::ShowDuration(show) => self.update(
                |config| config.blocks.show_duration = show,
                vec![("blocks", "show_duration", show.into())],
                cx,
            ),
            Setting::Compact(compact) => self.update(
                |config| config.blocks.compact = compact,
                vec![("blocks", "compact", compact.into())],
                cx,
            ),
            Setting::CheckUpdates(check) => self.update(
                |config| config.updates.check = check,
                vec![("updates", "check", check.into())],
                cx,
            ),
        }
    }
}

pub fn is_dark(appearance: WindowAppearance) -> bool {
    matches!(appearance, WindowAppearance::Dark | WindowAppearance::VibrantDark)
}

/// The theme to show, given the settings and the system's appearance.
pub fn wanted_theme(config: &Config, dark: bool) -> String {
    let appearance = &config.appearance;
    match (appearance.theme_mode, dark) {
        (ThemeMode::Manual, _) => appearance.theme.clone(),
        (ThemeMode::System, true) => appearance.dark_theme.clone(),
        (ThemeMode::System, false) => appearance.light_theme.clone(),
    }
}

/// Opens `config.toml` in the default text editor.
pub fn open_config_file() -> anyhow::Result<()> {
    use anyhow::Context as _;
    let path = Config::path().context("home directory not found")?;
    crate::platform::open(&path, true).with_context(|| format!("Failed to open {}", path.display()))
}

/// Opens a small window (Settings, About), centered on the screen.
pub fn open_panel<V: 'static + Render>(
    title: &str,
    content_size: Size<Pixels>,
    cx: &mut App,
    build: impl FnOnce(&mut Window, &mut Context<V>) -> V,
) -> Option<WindowHandle<V>> {
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, content_size, cx))),
        titlebar: Some(TitlebarOptions {
            title: Some(title.to_owned().into()),
            appears_transparent: false,
            traffic_light_position: None,
        }),
        kind: WindowKind::Floating,
        is_resizable: false,
        is_minimizable: false,
        ..Default::default()
    };
    let handle = cx.open_window(options, |window, cx| cx.new(|cx| build(window, cx))).ok()?;
    cx.activate(true);
    Some(handle)
}

/// The colors of a small window: the theme the settings call for, for this
/// window's appearance.
pub fn panel_palette(window: &Window, cx: &App) -> Palette {
    let config = global(cx).read(cx).config.clone();
    let wanted = wanted_theme(&config, is_dark(window.appearance()));
    Palette::new(&themes::resolve_or_default(&wanted, &config.colors).0)
}

/// Opens the Settings window, or brings it to the front if it is open.
pub fn open_window(cx: &mut App) {
    if let Some(handle) = cx.global::<GlobalSettings>().window
        && handle.update(cx, |_, window, _| window.activate_window()).is_ok()
    {
        cx.activate(true);
        return;
    }
    let handle = open_panel("Settings", size(px(460.), px(452.)), cx, SettingsWindow::new);
    cx.global_mut::<GlobalSettings>().window = handle;
}

pub struct SettingsWindow {
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl SettingsWindow {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle);
        let settings = global(cx);
        SettingsWindow {
            focus_handle,
            _subscriptions: vec![
                cx.observe(&settings, |_, _, cx| cx.notify()),
                cx.observe_window_appearance(window, |_, _, cx| cx.notify()),
            ],
        }
    }
}

impl Render for SettingsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = panel_palette(window, cx);
        let background = colors.background();
        let foreground = hsla(colors.foreground());
        let muted = hsla(colors.muted);
        let accent = colors.cursor();
        let red = hsla(colors.indexed(1, &Default::default()));
        let surface = |amount: f32| hsla(mix(colors.foreground(), background, amount));
        let settings = global(cx);
        let (config, error) = {
            let settings = settings.read(cx);
            (settings.config.clone(), settings.error.clone())
        };

        // A clickable piece of text; `id` must be unique in the window.
        let button = |id: &'static str, label: &str, selected: bool, setting: Setting| {
            div()
                .id(id)
                .px_2()
                .py(px(3.))
                .rounded_md()
                .text_color(if selected { hsla(background) } else { foreground })
                .when(selected, |button| button.bg(hsla(accent)))
                .when(!selected, |button| button.hover(|style| style.bg(surface(0.12))))
                .on_mouse_down(MouseButton::Left, move |_, _, cx| change(setting, cx))
                .child(label.to_owned())
        };
        // Exclusive choices side by side.
        let segmented = |buttons: Vec<gpui::Stateful<gpui::Div>>| {
            div().flex().gap_1().p(px(2.)).rounded_lg().bg(surface(0.08)).children(buttons)
        };
        let toggle = |id: &'static str, on: bool, setting: Setting| {
            div()
                .id(id)
                .w(px(34.))
                .h(px(20.))
                .p(px(2.))
                .flex()
                .rounded_full()
                .bg(if on { hsla(accent) } else { surface(0.22) })
                .when(on, |toggle| toggle.justify_end())
                .on_mouse_down(MouseButton::Left, move |_, _, cx| change(setting, cx))
                .child(div().size(px(16.)).rounded_full().bg(gpui::white()))
        };
        // "‹ Nord ›": the arrows go through the themes.
        let theme_picker = |ids: (&'static str, &'static str), name: &str, slot: ThemeSlot| {
            div()
                .flex()
                .items_center()
                .gap_1()
                .child(button(ids.0, "‹", false, Setting::CycleTheme(slot, -1)))
                .child(div().w(px(130.)).flex().justify_center().child(name.to_owned()))
                .child(button(ids.1, "›", false, Setting::CycleTheme(slot, 1)))
        };
        let row = |label: &'static str, control: gpui::AnyElement| {
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_4()
                .min_h(px(30.))
                .child(div().child(label))
                .child(control)
        };
        let section =
            |title: &'static str| div().pt_2().text_size(px(11.)).text_color(muted).child(title.to_uppercase());

        let appearance = &config.appearance;
        let system = appearance.theme_mode == ThemeMode::System;
        let mut theme_rows = vec![row(
            "Theme",
            segmented(vec![
                button("mode-system", "Match System", system, Setting::ThemeMode(ThemeMode::System)),
                button("mode-manual", "Manual", !system, Setting::ThemeMode(ThemeMode::Manual)),
            ])
            .into_any_element(),
        )];
        if system {
            theme_rows.push(row(
                "Light mode",
                theme_picker(("light-prev", "light-next"), &appearance.light_theme, ThemeSlot::Light)
                    .into_any_element(),
            ));
            theme_rows.push(row(
                "Dark mode",
                theme_picker(("dark-prev", "dark-next"), &appearance.dark_theme, ThemeSlot::Dark).into_any_element(),
            ));
        } else {
            theme_rows.push(row(
                "Color theme",
                theme_picker(("theme-prev", "theme-next"), &appearance.theme, ThemeSlot::Manual).into_any_element(),
            ));
        }
        let vertical = appearance.tabs == TabsLayout::Vertical;
        let blocks = config.blocks;

        div()
            .size_full()
            .px_6()
            .py_4()
            .flex()
            .flex_col()
            .gap_1()
            .bg(hsla(background))
            .text_size(px(13.))
            .text_color(foreground)
            .key_context("Panel")
            .track_focus(&self.focus_handle)
            .on_action(|_: &ClosePanel, window, _| window.remove_window())
            .on_key_down(|event: &KeyDownEvent, window, _| {
                if event.keystroke.key == "escape" {
                    window.remove_window();
                }
            })
            .child(section("Appearance"))
            .children(theme_rows)
            .child(row(
                "Tabs",
                segmented(vec![
                    button("tabs-top", "Top", !vertical, Setting::Tabs(TabsLayout::Horizontal)),
                    button("tabs-left", "Left", vertical, Setting::Tabs(TabsLayout::Vertical)),
                ])
                .into_any_element(),
            ))
            .child(row(
                "Font size",
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(button("font-smaller", "−", false, Setting::FontSize(-1.0)))
                    .child(div().w(px(40.)).flex().justify_center().child(format!("{}", config.font.size)))
                    .child(button("font-larger", "+", false, Setting::FontSize(1.0)))
                    .into_any_element(),
            ))
            .child(section("Blocks"))
            .child(row(
                "Show command duration",
                toggle("show-duration", blocks.show_duration, Setting::ShowDuration(!blocks.show_duration))
                    .into_any_element(),
            ))
            .child(row(
                "Compact blocks, no separators",
                toggle("compact", blocks.compact, Setting::Compact(!blocks.compact)).into_any_element(),
            ))
            .child(section("Updates"))
            .child(row(
                "Check for updates automatically",
                toggle("check-updates", config.updates.check, Setting::CheckUpdates(!config.updates.check))
                    .into_any_element(),
            ))
            .children(error.map(|error| div().pt_2().text_size(px(12.)).text_color(red).child(error)))
            .child(div().flex_1())
            .child(
                div()
                    .pt_3()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .id("open-settings-file")
                            .px_3()
                            .py_1()
                            .rounded_md()
                            .bg(surface(0.10))
                            .hover(|style| style.bg(surface(0.16)))
                            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                if let Err(error) = open_config_file() {
                                    settings.update(cx, |settings, cx| {
                                        settings.error = Some(format!("{error:#}"));
                                        cx.notify();
                                    });
                                }
                            })
                            .child("Open Settings File…"),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(muted)
                            .child("Changes are saved right away"),
                    ),
            )
    }
}
