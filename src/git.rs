//! Reads the current git branch without running `git`.

use std::path::{Path, PathBuf};

/// The branch of the repository containing `directory`, or the start of the
/// commit hash if HEAD is detached. `None` outside a git repository.
pub fn current_branch(directory: &Path) -> Option<String> {
    let git_dir = directory.ancestors().find_map(|ancestor| git_dir(&ancestor.join(".git")))?;
    let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let head = head.trim();
    match head.strip_prefix("ref: ") {
        Some(reference) => {
            Some(reference.strip_prefix("refs/heads/").unwrap_or(reference).to_owned())
        }
        None => Some(head.chars().take(7).collect()),
    }
}

/// Resolves `.git`: a directory, or a `gitdir: …` file (worktrees, submodules).
fn git_dir(dot_git: &Path) -> Option<PathBuf> {
    if dot_git.is_dir() {
        return Some(dot_git.to_owned());
    }
    let content = std::fs::read_to_string(dot_git).ok()?;
    let target = Path::new(content.trim().strip_prefix("gitdir: ")?);
    Some(if target.is_absolute() { target.to_owned() } else { dot_git.parent()?.join(target) })
}
