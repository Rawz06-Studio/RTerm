//! Command history.
//!
//! The list of commands comes from zsh's history, which zsh keeps maintaining
//! itself. RTerm adds, in a local SQLite database, what zsh does not keep:
//! the directory, exit code and duration of each command.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result};
use rusqlite::Connection;

use crate::commands::fold;

/// Number of commands taken from zsh's history at startup.
const SEED_LIMIT: usize = 5000;

/// What RTerm knows about the last run of a command.
#[derive(Debug, Clone, PartialEq)]
pub struct Meta {
    pub cwd: Option<String>,
    pub exit: Option<i32>,
    pub duration: Duration,
    /// Seconds since 1970.
    pub at: u64,
}

/// The history, shared by every pane.
pub struct HistoryStore {
    entries: Vec<String>,
    meta: HashMap<String, Meta>,
    database: Option<Connection>,
}

impl HistoryStore {
    /// Loads the history. `record` is `false` when the user does not want a
    /// database: only zsh's commands are used then.
    pub fn load(record: bool) -> HistoryStore {
        let mut store = HistoryStore { entries: Vec::new(), meta: HashMap::new(), database: None };
        if record {
            match database_path().and_then(|path| open_database(&path)) {
                Ok(database) => store.attach(database),
                Err(error) => eprintln!("rterm: rich history unavailable ({error:#})"),
            }
        }

        let zsh = zsh_history_path()
            .and_then(|path| std::fs::read(path).ok())
            .map(|bytes| parse_zsh_history(&bytes))
            .unwrap_or_default();
        // Without a zsh history, we fall back to what the database kept.
        if !zsh.is_empty() {
            store.entries = zsh;
        }
        store
    }

    #[cfg(test)]
    fn in_memory() -> HistoryStore {
        let mut store = HistoryStore { entries: Vec::new(), meta: HashMap::new(), database: None };
        let database = Connection::open_in_memory().unwrap();
        create_schema(&database).unwrap();
        store.attach(database);
        store
    }

    /// Attaches the database and reads back what it contains.
    fn attach(&mut self, database: Connection) {
        let rows = read_rows(&database).unwrap_or_default();
        for (command, meta) in rows {
            self.entries.push(command.clone());
            // Rows arrive from oldest to most recent.
            self.meta.insert(command, meta);
        }
        self.entries.dedup();
        self.database = Some(database);
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn get(&self, index: usize) -> Option<&str> {
        self.entries.get(index).map(String::as_str)
    }

    /// Adds a command that was just submitted.
    pub fn push(&mut self, command: &str) {
        if self.entries.last().is_none_or(|last| last != command) {
            self.entries.push(command.to_owned());
        }
    }

    /// Records the outcome of a finished command.
    pub fn record(&mut self, command: &str, cwd: Option<&Path>, exit: Option<i32>, duration: Duration) {
        let meta = Meta {
            cwd: cwd.map(|path| path.to_string_lossy().into_owned()),
            exit,
            duration,
            at: SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |elapsed| elapsed.as_secs()),
        };
        if let Some(database) = &self.database {
            let inserted = database.execute(
                "INSERT INTO history (command, cwd, exit_code, duration_ms, started_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                (command, &meta.cwd, meta.exit, meta.duration.as_millis() as i64, meta.at as i64),
            );
            if let Err(error) = inserted {
                eprintln!("rterm: command not saved to the history ({error})");
            }
        }
        self.meta.insert(command.to_owned(), meta);
    }

    /// Commands containing every word of `query`, from most recent to oldest,
    /// without duplicates.
    pub fn search(&self, query: &str, limit: usize) -> Vec<(&str, Option<&Meta>)> {
        let query = fold(query);
        let words: Vec<&str> = query.split_whitespace().collect();
        let mut seen = HashSet::new();
        self.entries
            .iter()
            .rev()
            .filter(|entry| {
                let folded = fold(entry);
                words.iter().all(|word| folded.contains(word))
            })
            .filter(|entry| seen.insert(entry.as_str()))
            .take(limit)
            .map(|entry| (entry.as_str(), self.meta.get(entry)))
            .collect()
    }
}

/// A pane's position in the history, when walking it with the arrow keys.
#[derive(Default)]
pub struct HistoryCursor {
    index: Option<usize>,
    /// What was typed before starting to walk up.
    draft: String,
}

impl HistoryCursor {
    /// Drops the navigation in progress (the user edited the text).
    pub fn reset(&mut self) {
        self.index = None;
    }

    /// Previous command. `current` is the editor's current text.
    pub fn previous<'a>(&mut self, store: &'a HistoryStore, current: &str) -> Option<&'a str> {
        let index = match self.index {
            None => {
                self.draft = current.to_owned();
                store.len().checked_sub(1)?
            }
            Some(index) => index.min(store.len()).checked_sub(1)?,
        };
        self.index = Some(index);
        store.get(index)
    }

    /// Next command, or the starting draft once back at the bottom.
    pub fn next(&mut self, store: &HistoryStore) -> Option<String> {
        let index = self.index? + 1;
        match store.get(index) {
            Some(entry) => {
                self.index = Some(index);
                Some(entry.to_owned())
            }
            None => {
                self.index = None;
                Some(std::mem::take(&mut self.draft))
            }
        }
    }
}

fn database_path() -> Result<PathBuf> {
    let home = dirs::home_dir().context("home directory not found")?;
    Ok(home.join(".config/rterm/history.db"))
}

fn open_database(path: &Path) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let database = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
    create_schema(&database)?;
    Ok(database)
}

fn create_schema(database: &Connection) -> Result<()> {
    database.execute_batch(
        "CREATE TABLE IF NOT EXISTS history (
             id          INTEGER PRIMARY KEY,
             command     TEXT NOT NULL,
             cwd         TEXT,
             exit_code   INTEGER,
             duration_ms INTEGER NOT NULL,
             started_at  INTEGER NOT NULL
         );
         CREATE INDEX IF NOT EXISTS history_started_at ON history (started_at);",
    )?;
    Ok(())
}

/// The database's commands, from oldest to most recent.
fn read_rows(database: &Connection) -> Result<Vec<(String, Meta)>> {
    let mut statement = database.prepare(
        "SELECT command, cwd, exit_code, duration_ms, started_at FROM (
             SELECT * FROM history ORDER BY id DESC LIMIT ?1
         ) ORDER BY id ASC",
    )?;
    let rows = statement.query_map([SEED_LIMIT as i64], |row| {
        Ok((
            row.get::<_, String>(0)?,
            Meta {
                cwd: row.get(1)?,
                exit: row.get(2)?,
                duration: Duration::from_millis(row.get::<_, i64>(3)?.max(0) as u64),
                at: row.get::<_, i64>(4)?.max(0) as u64,
            },
        ))
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

fn zsh_history_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("HISTFILE") {
        return Some(PathBuf::from(path));
    }
    let directory = std::env::var_os("ZDOTDIR").map(PathBuf::from).or_else(dirs::home_dir)?;
    Some(directory.join(".zsh_history"))
}

/// Reads a zsh history file, in the plain or extended format
/// (`: timestamp:duration;command`). A line ending with `\` continues.
fn parse_zsh_history(bytes: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(&unmetafy(bytes)).into_owned();
    let mut entries: Vec<String> = Vec::new();
    let mut continued = false;

    for line in text.lines() {
        let (content, continues) = match line.strip_suffix('\\') {
            Some(stripped) => (stripped, true),
            None => (line, false),
        };
        if continued {
            if let Some(last) = entries.last_mut() {
                last.push('\n');
                last.push_str(content);
            }
        } else {
            let command = content
                .strip_prefix(": ")
                .and_then(|rest| rest.split_once(';'))
                .map_or(content, |(_, command)| command);
            entries.push(command.to_owned());
        }
        continued = continues;
    }

    entries.retain(|entry| !entry.trim().is_empty());
    entries.dedup();
    let excess = entries.len().saturating_sub(SEED_LIMIT);
    entries.drain(..excess);
    entries
}

/// zsh writes some bytes (those of a few accented characters, notably) in
/// "metafied" form: 0x83 followed by the byte XOR 0x20.
fn unmetafy(bytes: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(bytes.len());
    let mut iter = bytes.iter();
    while let Some(&byte) = iter.next() {
        if byte == 0x83 {
            if let Some(&next) = iter.next() {
                output.push(next ^ 0x20);
            }
        } else {
            output.push(byte);
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_extended_and_plain_formats() {
        let entries = parse_zsh_history(b": 1700000000:0;git status\nls -la\n: 1700000001:3;echo a\\\nb\n");
        assert_eq!(entries, vec!["git status", "ls -la", "echo a\nb"]);
    }

    #[test]
    fn decodes_metafied_bytes() {
        // "ĉ" = C4 89; zsh writes byte 89 as 83 A9.
        let entries = parse_zsh_history(b"echo \xc4\x83\xa9\n");
        assert_eq!(entries, vec!["echo ĉ"]);
    }

    #[test]
    fn navigation_restores_the_draft() {
        let mut store = HistoryStore::in_memory();
        store.push("one");
        store.push("two");
        let mut cursor = HistoryCursor::default();
        assert_eq!(cursor.previous(&store, "draft"), Some("two"));
        assert_eq!(cursor.previous(&store, "two"), Some("one"));
        assert_eq!(cursor.previous(&store, "one"), None);
        assert_eq!(cursor.next(&store).as_deref(), Some("two"));
        assert_eq!(cursor.next(&store).as_deref(), Some("draft"));
        assert_eq!(cursor.next(&store), None);
    }

    #[test]
    fn search_is_recent_first_deduplicated_and_accent_insensitive() {
        let mut store = HistoryStore::in_memory();
        for command in ["git status", "cargo build", "git commit -m übung", "git status"] {
            store.push(command);
        }
        let found: Vec<&str> = store.search("git", 10).into_iter().map(|(command, _)| command).collect();
        assert_eq!(found, vec!["git status", "git commit -m übung"]);
        let found: Vec<&str> = store.search("UBUNG commit", 10).into_iter().map(|(command, _)| command).collect();
        assert_eq!(found, vec!["git commit -m übung"]);
        assert_eq!(store.search("", 2).len(), 2);
    }

    #[test]
    fn recorded_commands_survive_a_reload() {
        let mut store = HistoryStore::in_memory();
        store.push("make test");
        store.record("make test", Some(Path::new("/tmp/project")), Some(2), Duration::from_millis(1500));

        // We reopen the same database, as on the next launch.
        let database = store.database.take().unwrap();
        let mut reloaded = HistoryStore { entries: Vec::new(), meta: HashMap::new(), database: None };
        reloaded.attach(database);

        let (command, meta) = reloaded.search("make", 10)[0];
        let meta = meta.expect("metadata read back");
        assert_eq!(command, "make test");
        assert_eq!(meta.cwd.as_deref(), Some("/tmp/project"));
        assert_eq!(meta.exit, Some(2));
        assert_eq!(meta.duration, Duration::from_millis(1500));
    }
}
