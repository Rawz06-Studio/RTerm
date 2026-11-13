//! Command-line completion: command names, subcommands and options described
//! in the specs, git branches, files and directories.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};

use serde::Deserialize;

const BUILT_IN_SPECS: &str = include_str!("specs.toml");

/// Maximum number of candidates returned.
const MAX_CANDIDATES: usize = 300;

/// The most common zsh builtins, which are not in the `PATH`.
const BUILTINS: [&str; 22] = [
    "alias", "bg", "cd", "echo", "eval", "exec", "exit", "export", "fg", "history", "jobs",
    "kill", "popd", "pushd", "pwd", "read", "set", "source", "type", "unalias", "unset", "which",
];

/// Commands that only expect directories.
const DIRECTORY_COMMANDS: [&str; 4] = ["cd", "pushd", "rmdir", "mkdir"];

/// Prefixes after which the next word is still a command.
const COMMAND_PREFIXES: [&str; 6] = ["sudo", "env", "time", "exec", "nohup", "command"];

/// git subcommands that take a branch name.
const GIT_BRANCH_COMMANDS: [&str; 9] =
    ["checkout", "switch", "merge", "rebase", "branch", "diff", "log", "push", "cherry-pick"];

/// The description of a command, read from a specs file.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Spec {
    description: String,
    subcommands: BTreeMap<String, String>,
    options: BTreeMap<String, String>,
    flags: BTreeMap<String, BTreeMap<String, String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// The text that replaces the word being typed.
    pub insert: String,
    /// What is shown in the menu.
    pub label: String,
    pub detail: String,
    /// False for a directory: we carry on into its content, without a space.
    pub append_space: bool,
}

impl Candidate {
    fn word(word: &str, detail: &str) -> Candidate {
        Candidate {
            insert: word.to_owned(),
            label: word.to_owned(),
            detail: detail.to_owned(),
            append_space: true,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Completion {
    /// The part of the text the chosen candidate replaces.
    pub range: Range<usize>,
    pub candidates: Vec<Candidate>,
}

impl Completion {
    /// The prefix shared by every candidate, if it extends what is typed.
    pub fn common_prefix(&self) -> Option<&str> {
        let first = &self.candidates.first()?.insert;
        let mut end = first.len();
        for candidate in &self.candidates[1..] {
            end = first
                .char_indices()
                .zip(candidate.insert.chars())
                .take_while(|((_, a), b)| a == b)
                .last()
                .map_or(0, |((index, ch), _)| index + ch.len_utf8())
                .min(end);
        }
        (end > self.range.len()).then(|| &first[..end])
    }
}

/// What completion needs to know about the pane's shell.
/// Where a shell's files are, seen from RTerm, when they are elsewhere: a
/// WSL shell's `/home/me` is `\\wsl$\Ubuntu\home\me` for RTerm on Windows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Remote {
    pub root: PathBuf,
    /// The shell's home directory, in the shell's terms (`/home/me`).
    pub home: String,
}

impl Remote {
    pub fn wsl(distribution: &str, home: &str) -> Remote {
        Remote { root: PathBuf::from(format!(r"\\wsl$\{distribution}")), home: home.to_owned() }
    }

    /// The path RTerm reads for the shell's absolute path `path`.
    pub fn local(&self, path: &str) -> PathBuf {
        let mut local = self.root.clone();
        local.extend(path.split('/').filter(|part| !part.is_empty()));
        local
    }

    /// `path` with the shell's home directory shortened to `~`.
    pub fn display(&self, path: &str) -> String {
        match path.strip_prefix(self.home.as_str()) {
            Some("") => "~".to_owned(),
            Some(rest) if rest.starts_with('/') => format!("~{rest}"),
            _ => path.to_owned(),
        }
    }
}

pub struct ShellState<'a> {
    /// The shell's current directory, in the shell's terms.
    pub cwd: Option<&'a Path>,
    /// The shell's `PATH` (RTerm's own, failing that).
    pub path: Option<&'a str>,
    /// Aliases, functions and builtins reported by the shell.
    pub commands: Option<&'a HashSet<String>>,
    /// Set when the shell's files are elsewhere (WSL).
    pub remote: Option<&'a Remote>,
}

impl ShellState<'_> {
    /// The current directory, as a path RTerm can read.
    pub fn local_cwd(&self) -> Option<PathBuf> {
        let cwd = self.cwd?;
        Some(match self.remote {
            Some(remote) => remote.local(&cwd.to_string_lossy()),
            None => cwd.to_owned(),
        })
    }

    fn home(&self) -> Option<PathBuf> {
        match self.remote {
            Some(remote) => Some(remote.local(&remote.home)),
            None => dirs::home_dir(),
        }
    }
}

/// The completion engine, shared by every pane.
pub struct Engine {
    specs: BTreeMap<String, Spec>,
    /// The executables of the `PATH`, with the `PATH` used to list them.
    executables: RefCell<(String, Vec<String>)>,
}

impl Engine {
    pub fn load() -> Engine {
        let mut specs: BTreeMap<String, Spec> =
            toml::from_str(BUILT_IN_SPECS).expect("built-in specs are valid");
        // The user's specs complement or replace the built-in ones.
        // An unreadable file is skipped, without affecting the others.
        let user_files = dirs::home_dir()
            .map(|home| home.join(".config/rterm/completions"))
            .and_then(|directory| std::fs::read_dir(directory).ok());
        for entry in user_files.into_iter().flatten().flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|extension| extension == "toml")
                && let Ok(text) = std::fs::read_to_string(&path)
                && let Ok(user) = toml::from_str::<BTreeMap<String, Spec>>(&text)
            {
                specs.extend(user);
            }
        }
        Engine { specs, executables: RefCell::default() }
    }

    /// Candidates for the word ending at the cursor.
    pub fn complete(&self, text: &str, cursor: usize, shell: &ShellState) -> Option<Completion> {
        let line = parse(&text[..cursor]);
        let word = &line.word;
        let range = line.word_start..cursor;

        // Position and name of the command, once `sudo`, `VAR=…`, etc. are set aside.
        let mut words = line.previous.iter().map(String::as_str).peekable();
        while words
            .next_if(|word| COMMAND_PREFIXES.contains(word) || is_assignment(word))
            .is_some()
        {}
        let command = words.next();
        let arguments: Vec<&str> = words.collect();

        let mut candidates = match command {
            None if word.contains('/') => paths(word, shell, line.quoted, false),
            None => self.commands(word, shell),
            Some(command) => self.arguments(command, &arguments, word, &line, shell),
        };
        candidates.truncate(MAX_CANDIDATES);
        (!candidates.is_empty()).then_some(Completion { range, candidates })
    }

    /// Updates the sorted list of `PATH` executables, if the `PATH` changed.
    fn refresh_executables(&self, path: Option<&str>, remote: Option<&Remote>) {
        let path = path.map(str::to_owned).or_else(|| std::env::var("PATH").ok()).unwrap_or_default();
        let mut cache = self.executables.borrow_mut();
        if cache.0 != path {
            let mut names = match remote {
                Some(remote) => list_remote_executables(&path, remote),
                None => list_executables(&path),
            };
            names.sort_unstable();
            names.dedup();
            *cache = (path, names);
        }
    }

    /// Tells whether `name` is a command: `Some(false)` only when the shell has
    /// given the list of its aliases and functions, otherwise we cannot tell.
    pub fn is_command(&self, name: &str, shell: &ShellState) -> Option<bool> {
        // A path: `./run.sh`, `bin/tool` (and `.\run.ps1` on Windows).
        if name.contains('/') || (cfg!(windows) && name.contains('\\')) {
            return Some(resolve_path(name, shell).is_some_and(|path| path.exists()));
        }
        if BUILTINS.contains(&name) || shell.commands.is_some_and(|commands| commands.contains(name)) {
            return Some(true);
        }
        // An empty PATH: the shell runs where RTerm cannot see its programs
        // (WSL). Anything else may exist there.
        if shell.path == Some("") {
            return None;
        }
        self.refresh_executables(shell.path, shell.remote);
        let executables = self.executables.borrow();
        if executables.1.binary_search_by(|known| known.as_str().cmp(name)).is_ok() {
            return Some(true);
        }
        // Nothing could be read from the PATH (an unreachable WSL, for
        // instance): RTerm cannot tell.
        if executables.1.is_empty() {
            return None;
        }
        drop(executables);
        // PowerShell loads a module's `Verb-Noun` cmdlets on first use: the
        // shell cannot list them all up front.
        if is_verb_noun(name) {
            return None;
        }
        shell.commands.map(|_| false)
    }

    fn commands(&self, prefix: &str, shell: &ShellState) -> Vec<Candidate> {
        self.refresh_executables(shell.path, shell.remote);
        let cache = self.executables.borrow();
        let from_shell = shell.commands.into_iter().flatten().map(String::as_str);

        let mut names: Vec<&str> = cache
            .1
            .iter()
            .map(String::as_str)
            .chain(BUILTINS)
            .chain(from_shell)
            .filter(|name| name.starts_with(prefix))
            .collect();
        names.sort_unstable();
        names.dedup();
        names
            .into_iter()
            .map(|name| {
                let detail = self.specs.get(name).map_or("", |spec| spec.description.as_str());
                Candidate::word(name, detail)
            })
            .collect()
    }

    fn arguments(
        &self,
        command: &str,
        arguments: &[&str],
        word: &str,
        line: &Line,
        shell: &ShellState,
    ) -> Vec<Candidate> {
        let spec = self.specs.get(command);
        // The subcommand is the first argument that is not an option.
        let subcommand = arguments.iter().copied().find(|argument| !argument.starts_with('-'));

        if !line.quoted && word.starts_with('-') {
            let Some(spec) = spec else {
                return Vec::new();
            };
            let specific = subcommand.and_then(|subcommand| spec.flags.get(subcommand));
            return specific
                .into_iter()
                .flatten()
                .chain(&spec.options)
                .filter(|(flag, _)| flag.starts_with(word))
                .map(|(flag, detail)| Candidate::word(flag, detail))
                .collect();
        }

        let mut candidates = Vec::new();
        if let Some(spec) = spec
            && subcommand.is_none()
            && !line.quoted
        {
            candidates.extend(
                spec.subcommands
                    .iter()
                    .filter(|(name, _)| name.starts_with(word))
                    .map(|(name, detail)| Candidate::word(name, detail)),
            );
            if !candidates.is_empty() {
                return candidates;
            }
        }

        if command == "git"
            && subcommand.is_some_and(|subcommand| GIT_BRANCH_COMMANDS.contains(&subcommand))
            && let Some(cwd) = shell.local_cwd()
        {
            candidates.extend(
                git_branches(&cwd)
                    .iter()
                    .filter(|branch| branch.starts_with(word))
                    .map(|branch| Candidate::word(branch, "branch")),
            );
        }

        let directories_only = DIRECTORY_COMMANDS.contains(&command);
        candidates.extend(paths(word, shell, line.quoted, directories_only));
        candidates
    }
}

/// The end of the line, split into words the way the shell does.
#[derive(Debug, Default, PartialEq, Eq)]
struct Line {
    /// Complete words of the current command, before the word to complete.
    previous: Vec<String>,
    /// The word to complete, without quotes or backslashes.
    word: String,
    /// Where this word starts in the text (after an opening quote, if any).
    word_start: usize,
    /// True if the word is inside open quotes.
    quoted: bool,
}

fn parse(text: &str) -> Line {
    let mut line = Line::default();
    let mut quote: Option<char> = None;
    let mut in_word = false;
    let mut chars = text.char_indices().peekable();

    while let Some((index, ch)) = chars.next() {
        let after = index + ch.len_utf8();
        match (quote, ch) {
            (Some(open), _) if ch == open => quote = None,
            (Some('"'), '\\') | (None, '\\') => {
                if !in_word {
                    (in_word, line.word_start) = (true, index);
                }
                if let Some((_, escaped)) = chars.next() {
                    line.word.push(escaped);
                }
            }
            (Some(_), _) => line.word.push(ch),
            (None, '\'' | '"') => {
                quote = Some(ch);
                if !in_word {
                    // The word starts after the quote: its content is what
                    // completion replaces.
                    (in_word, line.word_start) = (true, after);
                }
            }
            (None, ' ' | '\t') => {
                if in_word {
                    line.previous.push(std::mem::take(&mut line.word));
                    in_word = false;
                }
            }
            // A command separator: start again from an empty line.
            (None, '|' | ';' | '&' | '\n' | '(') => {
                line = Line::default();
                in_word = false;
            }
            (None, _) => {
                if !in_word {
                    (in_word, line.word_start) = (true, index);
                }
                line.word.push(ch);
            }
        }
    }

    if !in_word {
        line.word_start = text.len();
    }
    line.quoted = quote.is_some();
    line
}

fn is_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| {
        !name.is_empty() && name.chars().all(|ch| ch.is_alphanumeric() || ch == '_')
    })
}

/// Names of the files found in the `PATH` directories.
fn list_executables(path: &str) -> Vec<String> {
    // ':' separates the directories on Unix, ';' on Windows.
    list_directories(std::env::split_paths(path).filter(|directory| !directory.as_os_str().is_empty()))
}

/// Names in a remote shell's `PATH` (WSL: Linux paths, separated by ':').
/// The Windows folders WSL adds (`/mnt/c/…`) are left out: reading them
/// through WSL is slow, and they are Windows programs.
fn list_remote_executables(path: &str, remote: &Remote) -> Vec<String> {
    list_directories(
        path.split(':')
            .filter(|directory| directory.starts_with('/') && !directory.starts_with("/mnt/"))
            .map(|directory| remote.local(directory)),
    )
}

fn list_directories(directories: impl Iterator<Item = PathBuf>) -> Vec<String> {
    let mut names = Vec::new();
    for directory in directories {
        let Ok(entries) = std::fs::read_dir(directory) else {
            continue;
        };
        for name in entries.flatten().filter_map(|entry| entry.file_name().into_string().ok()) {
            // Windows: `git.exe` is typed `git`.
            if cfg!(windows)
                && let Some((stem, extension)) = name.rsplit_once('.')
                && WINDOWS_PROGRAMS.iter().any(|program| program.eq_ignore_ascii_case(extension))
            {
                names.push(stem.to_owned());
            }
            names.push(name);
        }
    }
    names
}

/// `Get-ChildItem`, `Invoke-WebRequest`: a PowerShell command name.
fn is_verb_noun(name: &str) -> bool {
    let mut parts = name.split('-');
    let verb = parts.next().unwrap_or_default();
    let rest: Vec<&str> = parts.collect();
    !verb.is_empty()
        && !rest.is_empty()
        && std::iter::once(verb)
            .chain(rest)
            .all(|part| !part.is_empty() && part.chars().all(|ch| ch.is_ascii_alphanumeric()))
        && verb.starts_with(|ch: char| ch.is_ascii_alphabetic())
}

/// Extensions of the files Windows runs without their extension (PATHEXT).
const WINDOWS_PROGRAMS: &[&str] = &["exe", "cmd", "bat", "com", "ps1"];

/// Files and directories whose name extends `word`.
fn paths(word: &str, shell: &ShellState, quoted: bool, directories_only: bool) -> Vec<Candidate> {
    // "src/ma": we list `src`, looking for names starting with "ma".
    let (typed_directory, prefix) = match word.rfind('/') {
        Some(index) => word.split_at(index + 1),
        None => ("", word),
    };
    let Some(directory) = resolve_directory(typed_directory, shell) else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&directory) else {
        return Vec::new();
    };

    let mut found: Vec<(String, bool)> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            // `metadata` follows links: a link to a directory is a directory.
            let is_directory = std::fs::metadata(entry.path()).is_ok_and(|meta| meta.is_dir());
            Some((name, is_directory))
        })
        .filter(|(name, is_directory)| {
            (*is_directory || !directories_only)
                // Hidden files only show up when the dot is typed.
                && (prefix.starts_with('.') || !name.starts_with('.'))
        })
        .collect();

    // The macOS disk ignores case: without an exact match, so do we.
    let exact: Vec<(String, bool)> =
        found.iter().filter(|(name, _)| name.starts_with(prefix)).cloned().collect();
    if exact.is_empty() {
        let lowered = prefix.to_lowercase();
        found.retain(|(name, _)| name.to_lowercase().starts_with(&lowered));
    } else {
        found = exact;
    }
    found.sort();

    found
        .into_iter()
        .map(|(name, is_directory)| {
            let suffix = if is_directory { "/" } else { "" };
            let (escaped_directory, escaped_name) = if quoted {
                (typed_directory.to_owned(), name.clone())
            } else {
                (escape(typed_directory), escape(&name))
            };
            Candidate {
                insert: format!("{escaped_directory}{escaped_name}{suffix}"),
                label: format!("{name}{suffix}"),
                detail: if is_directory { "directory".to_owned() } else { String::new() },
                append_space: !is_directory,
            }
        })
        .collect()
}

/// The full path of a file as typed (`~`, relative or absolute).
/// The path RTerm reads for what the user typed (`~/x`, `/etc`, `src/x`).
pub fn resolve_path(typed: &str, shell: &ShellState) -> Option<PathBuf> {
    if typed == "~" {
        return shell.home();
    }
    if let Some(rest) = typed.strip_prefix("~/") {
        return Some(shell.home()?.join(rest));
    }
    if let Some(remote) = shell.remote
        && typed.starts_with('/')
    {
        return Some(remote.local(typed));
    }
    let typed = Path::new(typed);
    if typed.is_absolute() { Some(typed.to_owned()) } else { Some(shell.local_cwd()?.join(typed)) }
}

fn resolve_directory(typed: &str, shell: &ShellState) -> Option<PathBuf> {
    if typed == "~/" {
        return shell.home();
    }
    resolve_path(typed, shell)
}

/// Backslash-escapes the characters the shell would interpret.
fn escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for (index, ch) in text.char_indices() {
        let special = " \t!\"#$&'()*;<>?[\\]^`{|}".contains(ch);
        // A leading `~` must stay interpreted (home directory).
        if special || (ch == '~' && index > 0) {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    escaped
}

/// Local branches of the repository containing `directory`.
fn git_branches(directory: &Path) -> Vec<String> {
    let Some(git_dir) = directory.ancestors().map(|ancestor| ancestor.join(".git")).find(|git| git.is_dir())
    else {
        return Vec::new();
    };
    let mut branches = Vec::new();
    collect_refs(&git_dir.join("refs/heads"), "", &mut branches);
    // "Packed" branches are no longer files.
    if let Ok(packed) = std::fs::read_to_string(git_dir.join("packed-refs")) {
        branches.extend(
            packed
                .lines()
                .filter_map(|line| line.split_once(" refs/heads/"))
                .map(|(_, name)| name.to_owned()),
        );
    }
    branches.sort();
    branches.dedup();
    branches
}

fn collect_refs(directory: &Path, prefix: &str, branches: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        let path = entry.path();
        if path.is_dir() {
            collect_refs(&path, &format!("{prefix}{name}/"), branches);
        } else {
            branches.push(format!("{prefix}{name}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> Engine {
        Engine {
            specs: toml::from_str(BUILT_IN_SPECS).unwrap(),
            executables: RefCell::default(),
        }
    }

    /// A temporary working directory with a few files.
    fn workspace(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("rterm-completion-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src/deep")).unwrap();
        std::fs::create_dir_all(root.join("My documents")).unwrap();
        std::fs::create_dir_all(root.join("bin")).unwrap();
        for file in ["src/main.rs", "src/map.rs", "README.md", ".cache", "bin/rterm-tool"] {
            std::fs::write(root.join(file), "").unwrap();
        }
        root
    }

    fn inserts(engine: &Engine, text: &str, cwd: &Path, path: &str) -> Vec<String> {
        let shell = ShellState { cwd: Some(cwd), path: Some(path), commands: None, remote: None };
        engine
            .complete(text, text.len(), &shell)
            .map(|completion| completion.candidates.into_iter().map(|candidate| candidate.insert).collect())
            .unwrap_or_default()
    }

    /// A shell in WSL: RTerm reads its files under a root (`\\wsl$\…`),
    /// here a temporary folder holding a small Linux tree.
    #[test]
    fn a_wsl_shell_is_read_through_its_root() {
        let root = std::env::temp_dir().join(format!("rterm-completion-wsl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for directory in ["home/me/scripts", "usr/bin", "mnt/c/Windows"] {
            std::fs::create_dir_all(root.join(directory)).unwrap();
        }
        for file in ["home/me/scripts/run.sh", "home/me/notes.txt", "usr/bin/tool", "mnt/c/Windows/notepad.exe"] {
            std::fs::write(root.join(file), "").unwrap();
        }
        let remote = Remote { root: root.clone(), home: "/home/me".into() };
        let engine = engine();
        let commands: HashSet<String> = ["cd".to_owned()].into();
        let cwd = Path::new("/home/me/scripts");
        let shell = ShellState {
            cwd: Some(cwd),
            path: Some("/usr/bin:/mnt/c/Windows"),
            commands: Some(&commands),
            remote: Some(&remote),
        };
        let inserts = |text: &str| -> Vec<String> {
            engine
                .complete(text, text.len(), &shell)
                .map(|completion| completion.candidates.into_iter().map(|candidate| candidate.insert).collect())
                .unwrap_or_default()
        };

        // `./script`: highlighted as a command, completed with Tab.
        assert_eq!(engine.is_command("./run.sh", &shell), Some(true));
        assert_eq!(engine.is_command("./missing.sh", &shell), Some(false));
        assert_eq!(inserts("./r"), vec!["./run.sh"]);
        assert_eq!(inserts("cat ~/no"), vec!["~/notes.txt"]);
        assert_eq!(inserts("cat /home/me/no"), vec!["/home/me/notes.txt"]);
        // The Linux PATH, without the Windows folders WSL adds.
        assert_eq!(engine.is_command("tool", &shell), Some(true));
        assert_eq!(engine.is_command("notepad.exe", &shell), Some(false));
        assert_eq!(remote.display("/home/me/scripts"), "~/scripts");
        assert_eq!(remote.display("/home/meow"), "/home/meow");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn parses_words_quotes_and_separators() {
        let line = parse("FOO=1 git commit -m 'a message' && ls My\\ doc");
        assert_eq!(line.previous, vec!["ls"]);
        assert_eq!(line.word, "My doc");
        assert!(!line.quoted);

        let line = parse("cat \"My do");
        assert_eq!((line.word.as_str(), line.quoted, line.word_start), ("My do", true, 5));

        let line = parse("git ");
        assert_eq!((line.previous.len(), line.word.as_str(), line.word_start), (1, "", 4));
    }

    #[test]
    fn a_foreign_path_never_flags_commands() {
        let engine = engine();
        let commands: HashSet<String> = ["ll".to_owned()].into();
        let shell = |path| ShellState { cwd: None, path, commands: Some(&commands), remote: None };
        // A readable PATH: a missing command is flagged.
        let bin = workspace("foreign").join("bin").to_string_lossy().into_owned();
        assert_eq!(engine.is_command("nosuchrtermcommand", &shell(Some(&bin))), Some(false));
        // A PATH where nothing can be read: RTerm cannot tell.
        assert_eq!(engine.is_command("nosuchrtermcommand", &shell(Some("/nonexistent"))), None);
        // Empty PATH (WSL): RTerm cannot tell.
        assert_eq!(engine.is_command("nosuchrtermcommand", &shell(Some(""))), None);
        assert_eq!(engine.is_command("ll", &shell(Some(""))), Some(true));
        // A PowerShell cmdlet that is not loaded yet.
        assert_eq!(engine.is_command("Get-ChildItem", &shell(Some("/nonexistent"))), None);
        assert!(!is_verb_noun("-rf") && !is_verb_noun("ls") && !is_verb_noun("a--b"));
    }

    #[test]
    fn completes_commands_from_path_and_builtins() {
        let root = workspace("commands");
        let engine = engine();
        let path = root.join("bin").to_string_lossy().into_owned();
        assert_eq!(inserts(&engine, "rterm-", &root, &path), vec!["rterm-tool"]);
        assert_eq!(inserts(&engine, "sudo rterm-", &root, &path), vec!["rterm-tool"]);
        assert!(inserts(&engine, "ex", &root, &path).contains(&"export".to_owned()));
        // A command containing a `/` names a file, not a PATH entry.
        assert_eq!(inserts(&engine, "./bin/rt", &root, &path), vec!["./bin/rterm-tool"]);
    }

    #[test]
    fn completes_subcommands_flags_and_descriptions() {
        let root = workspace("specs");
        let engine = engine();
        assert_eq!(inserts(&engine, "git sta", &root, ""), vec!["stash", "status"]);
        assert_eq!(inserts(&engine, "git commit --am", &root, ""), vec!["--amend"]);
        assert_eq!(inserts(&engine, "cargo build --rel", &root, ""), vec!["--release"]);

        let shell = ShellState { cwd: Some(&root), path: Some(""), commands: None, remote: None };
        let completion = engine.complete("git comm", 8, &shell).unwrap();
        assert_eq!(completion.range, 4..8);
        assert_eq!(completion.candidates[0].detail, "Record the staged changes");
    }

    #[test]
    fn completes_paths_with_escaping_and_directory_rules() {
        let root = workspace("paths");
        let engine = engine();
        assert_eq!(inserts(&engine, "cat src/ma", &root, ""), vec!["src/main.rs", "src/map.rs"]);
        // A directory keeps its `/`; spaces are escaped outside quotes.
        assert_eq!(inserts(&engine, "ls My", &root, ""), vec!["My\\ documents/"]);
        assert_eq!(inserts(&engine, "ls \"My", &root, ""), vec!["My documents/"]);
        // Case is ignored when there is no exact match.
        assert_eq!(inserts(&engine, "cat read", &root, ""), vec!["README.md"]);
        // `cd` only offers directories, and no hidden files.
        assert_eq!(inserts(&engine, "cd ", &root, ""), vec!["My\\ documents/", "bin/", "src/"]);
        assert_eq!(inserts(&engine, "cat .", &root, ""), vec![".cache"]);
    }

    #[test]
    fn common_prefix_extends_what_was_typed() {
        let root = workspace("prefix");
        let engine = engine();
        let shell = ShellState { cwd: Some(&root), path: Some(""), commands: None, remote: None };
        let completion = engine.complete("cat src/m", 9, &shell).unwrap();
        assert_eq!(completion.common_prefix(), Some("src/ma"));
        let completion = engine.complete("cat src/ma", 10, &shell).unwrap();
        assert_eq!(completion.common_prefix(), None, "nothing more to add");
    }

    #[test]
    fn completes_git_branches() {
        let root = workspace("git");
        std::fs::create_dir_all(root.join(".git/refs/heads/feature")).unwrap();
        std::fs::write(root.join(".git/refs/heads/main"), "").unwrap();
        std::fs::write(root.join(".git/refs/heads/feature/blocks"), "").unwrap();
        std::fs::write(root.join(".git/packed-refs"), "abc123 refs/heads/legacy\n").unwrap();
        let engine = engine();
        assert_eq!(inserts(&engine, "git switch f", &root, ""), vec!["feature/blocks"]);
        let all = inserts(&engine, "git checkout ", &root.join("src"), "");
        assert!(all.starts_with(&["feature/blocks".into(), "legacy".into(), "main".into()]));
    }
}
