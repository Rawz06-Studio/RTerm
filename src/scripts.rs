//! Scripts: files kept in `~/.config/rterm/scripts/`, run in one click from
//! the palette or the menu, like a `.sh` you double-click.
//!
//! RTerm types the command that runs the script into the active tab (or a
//! new tab when a command is already running there): its output becomes a
//! block like any other command.

use std::path::{Path, PathBuf};

const EXAMPLE_SH: &str = include_str!("scripts_example.sh");
const EXAMPLE_PS1: &str = include_str!("scripts_example.ps1");

/// Extensions run by PowerShell rather than by a Unix shell.
const WINDOWS_ONLY: &[&str] = &["ps1", "bat", "cmd", "exe"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Script {
    /// The file name without its extension.
    pub name: String,
    /// The first comment of the file, after the `#!` line.
    pub description: String,
    pub path: PathBuf,
}

/// The shell that will run a script.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Runner {
    /// zsh, bash… on this machine.
    Unix,
    /// zsh or bash under WSL: Windows paths become `/mnt/<drive>/…`.
    Wsl,
    PowerShell,
}

pub fn directory() -> Option<PathBuf> {
    Some(dirs::home_dir()?.join(".config/rterm/scripts"))
}

/// Every script, sorted by name. On the first call, the directory is
/// created with an example.
pub fn load_all() -> Vec<Script> {
    let Some(directory) = directory() else {
        return Vec::new();
    };
    if !directory.exists() && std::fs::create_dir_all(&directory).is_ok() {
        // Windows: one for PowerShell, one for WSL.
        let examples: &[(&str, &str)] = if cfg!(windows) {
            &[("example.ps1", EXAMPLE_PS1), ("example.sh", EXAMPLE_SH)]
        } else {
            &[("example.sh", EXAMPLE_SH)]
        };
        for (name, content) in examples {
            let path = directory.join(name);
            if std::fs::write(&path, content).is_ok() {
                make_executable(&path);
            }
        }
    }
    list(&directory)
}

fn list(directory: &Path) -> Vec<Script> {
    let mut scripts: Vec<Script> = std::fs::read_dir(directory)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .filter_map(|path| {
            let file_name = path.file_name()?.to_str()?;
            // Hidden files and notes are not scripts.
            if file_name.starts_with('.') || has_extension(&path, &["md", "txt"]) {
                return None;
            }
            let name = path.file_stem()?.to_str()?.to_owned();
            let description = std::fs::read_to_string(&path).map(|text| describe(&text)).unwrap_or_default();
            Some(Script { name, description, path })
        })
        .collect();
    scripts.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then(a.path.cmp(&b.path)));
    scripts
}

/// The first comment line, skipping the `#!` line.
fn describe(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.starts_with("#!") && !line.is_empty())
        .find_map(|line| {
            line.strip_prefix('#')
                .or_else(|| line.strip_prefix("::"))
                .or_else(|| line.strip_prefix("REM ").or_else(|| line.strip_prefix("rem ")))
        })
        .map(|comment| comment.trim().to_owned())
        .unwrap_or_default()
}

fn has_extension(path: &Path, extensions: &[&str]) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extensions.iter().any(|known| known.eq_ignore_ascii_case(extension)))
}

impl Script {
    /// Whether this shell can run the script: PowerShell runs `.ps1`,
    /// `.bat`, `.cmd` and `.exe`, the Unix shells everything else.
    pub fn runs_in(&self, shell: Runner) -> bool {
        has_extension(&self.path, WINDOWS_ONLY) == (shell == Runner::PowerShell)
    }

    /// The command that runs the script in this shell.
    pub fn command(&self, shell: Runner) -> String {
        if shell == Runner::PowerShell {
            return format!("& '{}'", self.path.to_string_lossy().replace('\'', "''"));
        }
        // Shorter to read in the block: `~/.config/rterm/scripts/x.sh`.
        let in_home = dirs::home_dir().and_then(|home| self.path.strip_prefix(home).ok().map(Path::to_owned));
        let path = match (shell, in_home) {
            (Runner::Wsl, _) => quote(&wsl_path(&self.path)),
            (_, Some(relative)) => format!("~/{}", quote(&relative.to_string_lossy())),
            _ => quote(&self.path.to_string_lossy()),
        };
        // The `#!` line names the interpreter: it is used even when the file
        // is not executable (created by an editor, or under WSL).
        let text = std::fs::read_to_string(&self.path).unwrap_or_default();
        if let Some(interpreter) = text.lines().next().and_then(|line| line.strip_prefix("#!")) {
            let interpreter = interpreter.trim();
            if !interpreter.is_empty() {
                return format!("{interpreter} {path}");
            }
        }
        let interpreter = match self.path.extension().and_then(|extension| extension.to_str()) {
            Some("bash") => "bash",
            Some("zsh") => "zsh",
            Some("py") => "python3",
            Some("js" | "mjs") => "node",
            Some("rb") => "ruby",
            Some("pl") => "perl",
            Some("fish") => "fish",
            _ => "sh",
        };
        format!("{interpreter} {path}")
    }
}

/// `'it'\''s'`: a word the shell reads as is.
fn quote(text: &str) -> String {
    let safe = !text.is_empty()
        && text.chars().all(|ch| ch.is_ascii_alphanumeric() || "/._-+~:@".contains(ch))
        && !text.starts_with('~');
    if safe { text.to_owned() } else { format!("'{}'", text.replace('\'', r"'\''")) }
}

/// `C:\Users\me\x.sh` → `/mnt/c/Users/me/x.sh`, as WSL mounts the drives.
fn wsl_path(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    let mut chars = text.chars();
    match (chars.next(), chars.next()) {
        (Some(drive), Some(':')) if drive.is_ascii_alphabetic() => {
            format!("/mnt/{}{}", drive.to_ascii_lowercase(), &text[2..])
        }
        _ => text,
    }
}

#[cfg(unix)]
fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755));
}

#[cfg(not(unix))]
fn make_executable(_: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!("rterm-scripts-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        directory
    }

    #[test]
    fn lists_scripts_with_their_description() {
        let directory = folder("list");
        std::fs::write(directory.join("deploy.sh"), "#!/usr/bin/env bash\n\n# Deploys the site\necho ok\n").unwrap();
        std::fs::write(directory.join("Backup.ps1"), "# Copies the photos\n").unwrap();
        std::fs::write(directory.join("README.md"), "notes").unwrap();
        std::fs::write(directory.join(".hidden"), "").unwrap();
        std::fs::create_dir(directory.join("lib")).unwrap();

        let scripts = list(&directory);
        let names: Vec<&str> = scripts.iter().map(|script| script.name.as_str()).collect();
        assert_eq!(names, ["Backup", "deploy"]);
        assert_eq!(scripts[0].description, "Copies the photos");
        assert_eq!(scripts[1].description, "Deploys the site");
        assert!(scripts[1].runs_in(Runner::Unix) && scripts[1].runs_in(Runner::Wsl));
        assert!(!scripts[1].runs_in(Runner::PowerShell));
        assert!(scripts[0].runs_in(Runner::PowerShell) && !scripts[0].runs_in(Runner::Unix));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn builds_the_command_for_each_shell() {
        let directory = folder("command");
        let shebang = directory.join("with shebang.sh");
        std::fs::write(&shebang, "#!/usr/bin/env bash\necho hi\n").unwrap();
        let plain = directory.join("plain.py");
        std::fs::write(&plain, "print('hi')\n").unwrap();
        let script = |path: &Path| Script { name: String::new(), description: String::new(), path: path.to_owned() };

        let quoted = format!("'{}'", shebang.display());
        assert_eq!(script(&shebang).command(Runner::Unix), format!("/usr/bin/env bash {quoted}"));
        assert_eq!(script(&plain).command(Runner::Unix), format!("python3 {}", quote(&plain.to_string_lossy())));

        let windows = Script {
            name: String::new(),
            description: String::new(),
            path: PathBuf::from(r"C:\Users\it's me\run.ps1"),
        };
        assert_eq!(windows.command(Runner::PowerShell), r"& 'C:\Users\it''s me\run.ps1'");
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn windows_paths_are_seen_from_wsl() {
        assert_eq!(wsl_path(Path::new(r"C:\Users\me\x.sh")), "/mnt/c/Users/me/x.sh");
        assert_eq!(wsl_path(Path::new("/home/me/x.sh")), "/home/me/x.sh");
        assert_eq!(quote("it's"), r"'it'\''s'");
        assert_eq!(quote("/mnt/c/a.sh"), "/mnt/c/a.sh");
    }

    #[test]
    fn bundled_examples_describe_themselves() {
        assert!(!describe(EXAMPLE_SH).is_empty());
        assert!(!describe(EXAMPLE_PS1).is_empty());
    }
}
