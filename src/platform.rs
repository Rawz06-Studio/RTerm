//! What differs between macOS and Linux: shortcuts, opening files, fonts
//! and the window's title bar.

use std::path::Path;

pub const IS_MAC: bool = cfg!(target_os = "macos");

/// Picks the macOS or the Linux variant of a value (usually a shortcut).
///
/// On Linux, Ctrl alone belongs to the shell (Ctrl+C, Ctrl+W…): RTerm's
/// shortcuts use Ctrl+Shift, like most Linux terminals.
pub const fn pick<T: Copy>(mac: T, linux: T) -> T {
    if IS_MAC { mac } else { linux }
}

/// The monospace font used when the config does not name one.
pub const DEFAULT_FONT: &str = pick("Menlo", "DejaVu Sans Mono");

/// Room left at the left of the tab bar for the window buttons: macOS draws
/// them over RTerm's own title bar, Linux draws its title bar above.
pub const WINDOW_BUTTONS_WIDTH: f32 = pick(78.0, 8.0);

/// Opens a file or a folder with the default application; `text` asks for
/// the default text editor (macOS only, Linux decides from the file type).
pub fn open(path: &Path, text: bool) -> std::io::Result<()> {
    let mut command = if IS_MAC {
        let mut command = std::process::Command::new("open");
        if text {
            command.arg("-t");
        }
        command
    } else {
        std::process::Command::new("xdg-open")
    };
    command.arg(path).spawn().map(drop)
}
