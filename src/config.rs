//! TOML configuration, stored in `~/.config/rterm/config.toml`.
//!
//! Default values live in `default_config.toml`: that file is both the
//! template written on first launch and the source of the defaults. The
//! user's config is merged on top of it, key by key.

use std::fmt;
use std::path::PathBuf;

use alacritty_terminal::vte::ansi::Rgb;
use anyhow::{Context as _, Result};
use serde::Deserialize;
use serde::de::{self, Deserializer};

const DEFAULT_CONFIG: &str = include_str!("default_config.toml");

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub font: FontConfig,
    pub window: WindowConfig,
    pub shell: ShellConfig,
    pub terminal: TerminalConfig,
    pub history: HistoryConfig,
    pub updates: UpdatesConfig,
    pub blocks: BlocksConfig,
    pub appearance: Appearance,
    /// Tweaks to the chosen theme (`[colors]` section), empty by default.
    #[serde(default)]
    pub colors: toml::Table,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HistoryConfig {
    pub record: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpdatesConfig {
    pub check: bool,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
pub struct BlocksConfig {
    /// Shows how long each command took in its header.
    pub show_duration: bool,
    /// Tighter blocks, without the line between them.
    pub compact: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Appearance {
    /// Follow the system's light or dark mode, or always use `theme`.
    pub theme_mode: ThemeMode,
    /// The theme used in manual mode.
    pub theme: String,
    /// The themes used in system mode.
    pub light_theme: String,
    pub dark_theme: String,
    pub tabs: TabsLayout,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    System,
    Manual,
}

impl ThemeMode {
    pub fn as_str(self) -> &'static str {
        match self {
            ThemeMode::System => "system",
            ThemeMode::Manual => "manual",
        }
    }
}

impl TabsLayout {
    pub fn as_str(self) -> &'static str {
        match self {
            TabsLayout::Horizontal => "horizontal",
            TabsLayout::Vertical => "vertical",
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TabsLayout {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FontConfig {
    pub family: String,
    pub size: f32,
    pub line_height: f32,
    pub ligatures: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WindowConfig {
    pub width: f32,
    pub height: f32,
    pub padding: f32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ShellConfig {
    pub program: String,
    pub args: Vec<String>,
    pub working_directory: String,
    pub integration: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TerminalConfig {
    pub scrollback: usize,
    pub cursor_shape: CursorShapeConfig,
    pub option_as_meta: bool,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CursorShapeConfig {
    Block,
    Beam,
    Underline,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Theme {
    pub background: Color,
    pub foreground: Color,
    pub cursor: Color,
    pub selection: Color,
    pub muted: Color,
    pub normal: AnsiColors,
    pub bright: AnsiColors,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AnsiColors {
    pub black: Color,
    pub red: Color,
    pub green: Color,
    pub yellow: Color,
    pub blue: Color,
    pub magenta: Color,
    pub cyan: Color,
    pub white: Color,
}

impl AnsiColors {
    pub fn as_array(&self) -> [Rgb; 8] {
        [
            self.black.0,
            self.red.0,
            self.green.0,
            self.yellow.0,
            self.blue.0,
            self.magenta.0,
            self.cyan.0,
            self.white.0,
        ]
    }
}

/// A color written as `"#rrggbb"` in the TOML file.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Color(pub Rgb);

impl fmt::Debug for Color {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{:02x}{:02x}{:02x}", self.0.r, self.0.g, self.0.b)
    }
}

impl<'de> Deserialize<'de> for Color {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        parse_hex(&text)
            .map(Color)
            .ok_or_else(|| de::Error::custom(format!("invalid color \"{text}\", expected \"#rrggbb\"")))
    }
}

fn parse_hex(text: &str) -> Option<Rgb> {
    let hex = text.strip_prefix('#')?;
    if hex.len() != 6 || !hex.is_ascii() {
        return None;
    }
    let channel = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    Some(Rgb { r: channel(0)?, g: channel(2)?, b: channel(4)? })
}

impl Config {
    pub fn path() -> Option<PathBuf> {
        Some(dirs::home_dir()?.join(".config").join("rterm").join("config.toml"))
    }

    /// Loads the config. An unreadable config never blocks startup: we fall
    /// back to the defaults and return the error so it can be displayed.
    pub fn load() -> (Config, Option<String>) {
        match Self::try_load() {
            Ok(config) => (config, None),
            Err(error) => (Self::defaults(), Some(format!("{error:#}"))),
        }
    }

    pub fn defaults() -> Config {
        toml::from_str(DEFAULT_CONFIG).expect("the embedded default config is valid")
    }

    fn try_load() -> Result<Config> {
        let Some(path) = Self::path() else {
            return Ok(Self::defaults());
        };

        if !path.exists() {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("creating {}", parent.display()))?;
            }
            std::fs::write(&path, DEFAULT_CONFIG)
                .with_context(|| format!("writing {}", path.display()))?;
            return Ok(Self::defaults());
        }

        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        Self::from_user_toml(&text).with_context(|| format!("in {}", path.display()))
    }

    fn from_user_toml(text: &str) -> Result<Config> {
        let mut merged: toml::Table = toml::from_str(DEFAULT_CONFIG)?;
        let user: toml::Table = toml::from_str(text)?;
        merge(&mut merged, user);
        Ok(toml::Value::Table(merged).try_into()?)
    }
}

/// Writes settings to the config file, as `(section, key, value)`.
/// Comments and the rest of the file are kept as they are.
pub fn save(settings: &[(&str, &str, toml_edit::Value)]) -> Result<()> {
    let path = Config::path().context("home directory not found")?;
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    std::fs::write(&path, with_settings(&text, settings)?)
        .with_context(|| format!("writing {}", path.display()))
}

/// The config text with the given settings replaced.
fn with_settings(text: &str, settings: &[(&str, &str, toml_edit::Value)]) -> Result<String> {
    let mut document: toml_edit::DocumentMut = text.parse()?;
    for (section, key, value) in settings {
        let mut value = value.clone();
        // Keep a comment written after the old value on the same line.
        if let Some(old) = document.get(section).and_then(|table| table.get(key)).and_then(|item| item.as_value()) {
            *value.decor_mut() = old.decor().clone();
        }
        document[section][key] = toml_edit::Item::Value(value);
    }
    Ok(document.to_string())
}

/// Merges `over` into `base`: tables are merged recursively, any other
/// value replaces the one in `base`.
pub fn merge(base: &mut toml::Table, over: toml::Table) {
    for (key, value) in over {
        match (base.get_mut(&key), value) {
            (Some(toml::Value::Table(base_table)), toml::Value::Table(over_table)) => {
                merge(base_table, over_table);
            }
            (_, value) => {
                base.insert(key, value);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_parses() {
        let config = Config::defaults();
        assert_eq!(config.font.family, "");
        assert_eq!(config.appearance.theme, crate::themes::DEFAULT_THEME);
        assert!(config.colors.is_empty());
    }

    #[test]
    fn user_config_overrides_only_what_it_sets() {
        let config = Config::from_user_toml(
            "[font]\nsize = 16.0\n\n[colors.normal]\nred = \"#ff0000\"\n",
        )
        .unwrap();
        assert_eq!(config.font.size, 16.0);
        assert_eq!(config.font.family, "");
        assert_eq!(config.appearance.theme, crate::themes::DEFAULT_THEME);
        assert!(config.colors.contains_key("normal"));
    }

    #[test]
    fn theme_choice_is_saved_without_losing_comments() {
        let text = "# my comment\n[font]\nsize = 15.0\n";
        let set = |text: &str, key: &str, value: &str| {
            with_settings(text, &[("appearance", key, value.into())]).unwrap()
        };
        let updated = set(text, "theme", "Nord");
        assert!(updated.contains("# my comment"));
        assert_eq!(Config::from_user_toml(&updated).unwrap().appearance.theme, "Nord");

        let updated = set("[appearance]\ntheme = \"Nord\" # picked\n", "theme", "Dracula");
        assert_eq!(Config::from_user_toml(&updated).unwrap().appearance.theme, "Dracula");

        let updated = set(&updated, "tabs", "vertical");
        let config = Config::from_user_toml(&updated).unwrap();
        assert_eq!((config.appearance.theme.as_str(), config.appearance.tabs), ("Dracula", TabsLayout::Vertical));
    }

    #[test]
    fn settings_of_any_type_are_saved() {
        let updated = with_settings(
            "[blocks]\ncompact = false # keep\n",
            &[("blocks", "compact", true.into()), ("font", "size", 15.0.into())],
        )
        .unwrap();
        assert!(updated.contains("# keep"));
        let config = Config::from_user_toml(&updated).unwrap();
        assert!(config.blocks.compact);
        assert_eq!(config.font.size, 15.0);
    }
}
