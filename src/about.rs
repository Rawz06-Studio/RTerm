//! The About window: version, configuration file, theme and update status.

use gpui::{
    App, Context, FocusHandle, Global, KeyDownEvent, Render, Subscription, Window, WindowHandle, div,
    prelude::*, px, size,
};

use crate::config::Config;
use crate::palette::hsla;
use crate::settings::{self, ClosePanel};
use crate::update;

/// The About window, while it is open: there is only ever one.
#[derive(Default)]
struct OpenAbout(Option<WindowHandle<AboutWindow>>);

impl Global for OpenAbout {}

/// Opens the About window, or brings it to the front if it is open.
pub fn open(cx: &mut App) {
    if let Some(handle) = cx.try_global::<OpenAbout>().and_then(|open| open.0)
        && handle.update(cx, |_, window, _| window.activate_window()).is_ok()
    {
        cx.activate(true);
        return;
    }
    let handle = settings::open_panel("About RTerm", size(px(420.), px(250.)), cx, AboutWindow::new);
    cx.set_global(OpenAbout(handle));
}

pub struct AboutWindow {
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl AboutWindow {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle);
        let settings = settings::global(cx);
        let updater = update::global(cx);
        AboutWindow {
            focus_handle,
            _subscriptions: vec![
                cx.observe(&settings, |_, _, cx| cx.notify()),
                cx.observe(&updater, |_, _, cx| cx.notify()),
                cx.observe_window_appearance(window, |_, _, cx| cx.notify()),
            ],
        }
    }
}

impl Render for AboutWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = settings::panel_palette(window, cx);
        let foreground = hsla(colors.foreground());
        let muted = hsla(colors.muted);
        let accent = hsla(colors.cursor());
        let config = settings::global(cx).read(cx).config.clone();
        let theme = settings::wanted_theme(&config, settings::is_dark(window.appearance()));
        let config_path = Config::path().map(|path| crate::view::display_path(&path)).unwrap_or_default();
        let line = |label: &'static str, value: String| {
            div()
                .flex()
                .gap_3()
                .child(div().w(px(80.)).flex_none().text_color(muted).child(label))
                .child(div().min_w_0().child(value))
        };

        div()
            .size_full()
            .px_6()
            .py_5()
            .flex()
            .flex_col()
            .gap_4()
            .bg(hsla(colors.background()))
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
                    .child(line("Config", config_path))
                    .child(line("Theme", theme))
                    .child(line("Updates", update::global(cx).read(cx).summary()))
                    .child(line("License", "MIT".to_owned())),
            )
    }
}
