//! Color themes: the ones built into RTerm, and the ones the user drops
//! into `~/.config/rterm/themes/`.

use std::path::PathBuf;

use anyhow::{Context as _, Result, anyhow};

use crate::config::{Theme, merge};

const BUILT_IN: &str = include_str!("themes.toml");
pub const DEFAULT_THEME: &str = "RTerm Dark";

/// A theme as written, before its colors are validated.
pub struct ThemeSource {
    pub name: String,
    table: toml::Table,
}

fn built_in() -> Vec<ThemeSource> {
    let document: toml::Table = toml::from_str(BUILT_IN).expect("built-in themes are valid");
    let themes = document.get("theme").and_then(toml::Value::as_array).cloned().unwrap_or_default();
    themes
        .into_iter()
        .filter_map(|theme| {
            let mut table = theme.try_into::<toml::Table>().ok()?;
            let name = table.remove("name")?.as_str()?.to_owned();
            Some(ThemeSource { name, table })
        })
        .collect()
}

pub fn user_directory() -> Option<PathBuf> {
    Some(dirs::home_dir()?.join(".config/rterm/themes"))
}

/// The user's themes. An unreadable file is skipped: it must not prevent
/// the others from being used.
fn user_themes() -> Vec<ThemeSource> {
    let Some(entries) = user_directory().and_then(|directory| std::fs::read_dir(directory).ok())
    else {
        return Vec::new();
    };
    let mut themes: Vec<ThemeSource> = entries
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            if path.extension()? != "toml" {
                return None;
            }
            let mut table: toml::Table = toml::from_str(&std::fs::read_to_string(&path).ok()?).ok()?;
            // The name comes from the `name` key, otherwise from the file name.
            let name = match table.remove("name") {
                Some(toml::Value::String(name)) => name,
                _ => path.file_stem()?.to_string_lossy().into_owned(),
            };
            Some(ThemeSource { name, table })
        })
        .collect();
    themes.sort_by(|a, b| a.name.cmp(&b.name));
    themes
}

/// Every available theme. A user theme replaces the built-in theme of the
/// same name.
pub fn all() -> Vec<ThemeSource> {
    let mut themes = built_in();
    for theme in user_themes() {
        match themes.iter_mut().find(|existing| existing.name == theme.name) {
            Some(existing) => *existing = theme,
            None => themes.push(theme),
        }
    }
    themes
}

/// The theme `name`, with the tweaks from the config's `[colors]` section.
pub fn resolve(name: &str, overrides: &toml::Table) -> Result<Theme> {
    let source = all()
        .into_iter()
        .find(|theme| theme.name == name)
        .ok_or_else(|| anyhow!("theme \"{name}\" not found"))?;
    let mut table = source.table;
    merge(&mut table, overrides.clone());
    toml::Value::Table(table)
        .try_into()
        .with_context(|| format!("theme \"{name}\""))
}

/// Like [`resolve`], but falls back to the default theme on error.
pub fn resolve_or_default(name: &str, overrides: &toml::Table) -> (Theme, Option<String>) {
    match resolve(name, overrides) {
        Ok(theme) => (theme, None),
        Err(error) => {
            let fallback = resolve(DEFAULT_THEME, &toml::Table::new())
                .expect("the built-in default theme is valid");
            (fallback, Some(format!("{error:#}")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_built_in_theme_is_valid() {
        let themes = built_in();
        assert!(themes.iter().any(|theme| theme.name == DEFAULT_THEME));
        for theme in themes {
            let parsed: Result<Theme, _> = toml::Value::Table(theme.table).try_into();
            assert!(parsed.is_ok(), "theme {} is invalid: {:?}", theme.name, parsed.err());
        }
    }

    #[test]
    fn overrides_replace_single_colors() {
        let overrides: toml::Table =
            toml::from_str("background = \"#000000\"\n[normal]\nred = \"#ff0000\"").unwrap();
        let theme = resolve("Dracula", &overrides).unwrap();
        assert_eq!(format!("{:?}", theme.background), "#000000");
        assert_eq!(format!("{:?}", theme.normal.red), "#ff0000");
        assert_eq!(format!("{:?}", theme.normal.green), "#50fa7b");
    }

    #[test]
    fn unknown_theme_falls_back_with_an_error() {
        let (theme, error) = resolve_or_default("does not exist", &toml::Table::new());
        assert!(error.is_some());
        assert_eq!(format!("{:?}", theme.background), "#101216");
    }
}
