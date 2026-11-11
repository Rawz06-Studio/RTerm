//! The terminal color table, built from the theme.

use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{Color, NamedColor, Rgb};
use gpui::{Hsla, Rgba};

use crate::config::Theme;

/// The 269 colors the emulator can address: 16 ANSI colors, the 6×6×6 cube,
/// 24 grays, then the named colors (text, background, cursor, dimmed).
pub struct Palette {
    colors: Vec<Rgb>,
    pub selection: Rgb,
    pub muted: Rgb,
}

const DIM_FACTOR: f32 = 0.66;

impl Palette {
    pub fn new(theme: &Theme) -> Self {
        let normal = theme.normal.as_array();
        let bright = theme.bright.as_array();
        let foreground = theme.foreground.0;
        let background = theme.background.0;

        let mut colors = Vec::with_capacity(269);
        colors.extend(normal);
        colors.extend(bright);

        let level = |i: u8| if i == 0 { 0 } else { 55 + 40 * i };
        for r in 0..6 {
            for g in 0..6 {
                for b in 0..6 {
                    colors.push(Rgb { r: level(r), g: level(g), b: level(b) });
                }
            }
        }
        for i in 0..24 {
            let value = 8 + 10 * i;
            colors.push(Rgb { r: value, g: value, b: value });
        }

        colors.push(foreground); // 256 Foreground
        colors.push(background); // 257 Background
        colors.push(theme.cursor.0); // 258 Cursor
        colors.extend(normal.map(|color| dim(color, background))); // 259..=266 Dim*
        colors.push(foreground); // 267 BrightForeground
        colors.push(dim(foreground, background)); // 268 DimForeground

        Palette { colors, selection: theme.selection.0, muted: theme.muted.0 }
    }

    pub fn background(&self) -> Rgb {
        self.colors[NamedColor::Background as usize]
    }

    pub fn foreground(&self) -> Rgb {
        self.colors[NamedColor::Foreground as usize]
    }

    pub fn cursor(&self) -> Rgb {
        self.colors[NamedColor::Cursor as usize]
    }

    /// The color at an index, taking into account redefinitions made by
    /// programs (OSC 4/10/11… sequences), stored in `overrides`.
    pub fn indexed(&self, index: usize, overrides: &Colors) -> Rgb {
        overrides[index].unwrap_or(self.colors[index])
    }

    pub fn resolve(&self, color: Color, overrides: &Colors) -> Rgb {
        match color {
            Color::Spec(rgb) => rgb,
            Color::Named(named) => self.indexed(named as usize, overrides),
            Color::Indexed(index) => self.indexed(index as usize, overrides),
        }
    }
}

/// Dims a color by moving it toward the background.
pub fn dim(color: Rgb, background: Rgb) -> Rgb {
    mix(color, background, DIM_FACTOR)
}

/// Mixes two colors: `amount` is 1 for `color` alone, 0 for `other`.
pub fn mix(color: Rgb, other: Rgb, amount: f32) -> Rgb {
    let channel = |a: u8, b: u8| (a as f32 * amount + b as f32 * (1.0 - amount)) as u8;
    Rgb {
        r: channel(color.r, other.r),
        g: channel(color.g, other.g),
        b: channel(color.b, other.b),
    }
}

pub fn hsla(color: Rgb) -> Hsla {
    Rgba {
        r: color.r as f32 / 255.0,
        g: color.g as f32 / 255.0,
        b: color.b as f32 / 255.0,
        a: 1.0,
    }
    .into()
}
