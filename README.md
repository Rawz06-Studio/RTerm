<div align="center">

<img src="assets/icon.png" width="128" alt="RTerm icon">

# RTerm

**A modern terminal that stays on your machine.**

Command blocks, a real text editor for your input, completion, a command
palette — with no account, no cloud and no AI.

[Download](../../releases/latest) · [Features](#features) · [Configuration](#configuration) · [Build from source](#build-from-source)

</div>

![Command blocks, with a failed command highlighted](docs/screenshots/blocks.png)

## Why

Modern terminals have made the command line much nicer to use: output grouped
per command, an input area that behaves like an editor, completion with
descriptions. Too often that comes bundled with a sign-in screen, cloud sync
and an AI subscription.

RTerm keeps the first part and drops the second:

- **No account.** There is nothing to sign in to.
- **No cloud.** Your history and settings are plain files in
  `~/.config/rterm/`. The only network request RTerm makes is asking GitHub
  whether a new version exists, and you can turn it off.
- **No AI.** Completion comes from your `PATH`, your shell and small local
  description files.

It is written in Rust on top of [GPUI](https://www.gpui.rs) (the GPU-accelerated
UI framework behind Zed) and [`alacritty_terminal`](https://crates.io/crates/alacritty_terminal)
for terminal emulation.

## Features

### Blocks

Each command and its output form a block, with the directory it ran in, how
long it took and its exit code. Failed commands stand out in red. Click a block
to select it, drag to select text, `Cmd+C` to copy.

Full-screen programs such as `vim`, `less` or `htop` take over the whole pane,
then hand it back to the block list when they exit.

### An input editor, not a prompt

The command line is a small multi-line text editor: mouse selection, word
motions, undo and redo, `Shift+Enter` for a new line. It highlights what you
type as you type it — known commands in green, unknown ones in red, strings,
options, variables, and existing paths underlined — using the aliases and
functions of your own shell.

### Completion and history

![Completion menu and vertical tabs](docs/screenshots/completion.png)

- `Tab` completes commands, files, git branches, and subcommands and options
  (with descriptions) for `git`, `cargo`, `npm`, `docker` and `brew`.
- `Ctrl+R` searches your history. Commands run in RTerm remember their
  directory, exit code and duration.

### Tabs, splits and a command palette

![Command palette with the light theme](docs/screenshots/palette.png)

- Tabs at the top or in a column on the left, your choice.
- Split any pane to the right or downwards.
- `Cmd+Shift+P` opens the command palette: every action, every theme and your
  workflows, searchable.

### Workflows

Commands you keep retyping, with blanks to fill in. They are TOML files in
`~/.config/rterm/workflows/`:

```toml
[[workflow]]
name = "Who is listening on a port"
description = "Finds the process holding a port"
command = "lsof -nP -iTCP:{{port}} -sTCP:LISTEN"
arguments = { port = 3000 }
```

Pick one from the palette and it lands in the editor with its first parameter
selected; `Tab` jumps to the next one.

### Themes

Seven built-in themes — RTerm Dark, RTerm Light, Dracula, Nord, Gruvbox Dark,
Solarized Dark and Solarized Light. By default RTerm follows macOS's light or
dark mode, with a theme for each; pick **Manual** in Settings to keep a single
one. Add your own as `~/.config/rterm/themes/<name>.toml`.

## Install

RTerm runs on **macOS 11 or later** (Apple Silicon and Intel), on
**Linux** (x86_64 and ARM, glibc 2.35 or later: Ubuntu 22.04, Debian 12,
Fedora 36 and newer) and, as a first preview, on **Windows 10 and 11**
(x86_64). Downloads are in the [latest release](../../releases/latest).

### macOS

1. Download `RTerm-<version>-macos-universal.dmg`.
2. Open it and drag **RTerm** into **Applications**.

#### First launch

RTerm is signed, but not with a paid Apple Developer ID and it is not
notarized, so macOS asks for confirmation the first time:

- **macOS 15 and later:** open RTerm once (macOS refuses), then go to
  **System Settings → Privacy & Security**, scroll down and click
  **Open Anyway**.
- **Earlier versions:** right-click RTerm in Applications and choose **Open**,
  then **Open** again.

Or skip the dialog altogether from a terminal:

```bash
xattr -dr com.apple.quarantine /Applications/RTerm.app
```

You only need to do this once: later versions install themselves (see
[Updates](#updates)) and do not ask again.

### Linux

Linux support is new: expect rough edges, and please report them.

- **AppImage**: download `RTerm-<version>-linux-<arch>.AppImage`, make it
  executable (`chmod +x`) and run it.
- **Archive**: extract `RTerm-<version>-linux-<arch>.tar.gz` into `~/.local`
  (or `/usr/local`) to get `rterm` in your `PATH` and RTerm in your
  application menu:

  ```bash
  tar -xzf RTerm-*-linux-*.tar.gz --strip-components=1 -C ~/.local
  ```

RTerm uses Wayland or X11 and needs Vulkan graphics drivers, which desktop
distributions install by default.

### Windows (preview)

- **Installer**: run `RTerm-<version>-windows-x86_64-setup.exe`. It
  installs RTerm for your account only (no administrator prompt), with a
  Start menu entry and an uninstaller.
- **Portable**: extract `RTerm-<version>-windows-x86_64.zip` anywhere (a USB
  stick works) and run `RTerm.exe`; nothing is installed.

RTerm is not signed with a paid certificate, so Windows may say it protected
your PC: click **More info**, then **Run anyway**.

RTerm opens your WSL distribution when one is installed (in your Linux home),
otherwise PowerShell. On Windows, every shell runs as a classic terminal for
now: no blocks and no input editor yet. To pick a shell:

```toml
[shell]
program = "pwsh.exe"   # or "wsl.exe", "powershell.exe", "cmd.exe"
```

### Updates (macOS)

RTerm checks this repository's releases at launch and then once a day. When a
new version is out, an **Update to …** button appears in the tab bar: click it
to download and install it, then **Restart to Update** (or simply quit; the
next launch uses the new version). **RTerm → Check for Updates…** checks right
away.

Nothing is installed without a click. To stop the automatic check, add this to
`config.toml`:

```toml
[updates]
check = false
```

RTerm must be in a folder you can write to (`/Applications` is fine on a
single-user Mac) to replace itself.

## Keyboard shortcuts

On Linux and Windows, plain `Ctrl` shortcuts belong to the shell (`Ctrl+C`,
`Ctrl+W`…), so RTerm uses `Ctrl+Shift` instead, like most Linux terminals.

| Action | macOS | Linux and Windows |
| --- | --- | --- |
| Command palette | `Cmd+Shift+P` or `Cmd+P` | `Ctrl+Shift+P` |
| New window | `Cmd+N` | `Ctrl+Shift+N` |
| New tab | `Cmd+T` | `Ctrl+Shift+T` |
| Next / previous tab | `Ctrl+Tab` / `Ctrl+Shift+Tab` | same, or `Ctrl+PgDn` / `Ctrl+PgUp` |
| Go to tab 1–9 | `Cmd+1` … `Cmd+9` | `Alt+1` … `Alt+9` |
| Split right / down | `Cmd+D` / `Cmd+Shift+D` | `Ctrl+Shift+D` / `Ctrl+Shift+E` |
| Next / previous pane | `Cmd+Option+→` / `Cmd+Option+←` | `Ctrl+Alt+→` / `Ctrl+Alt+←` |
| Close pane | `Cmd+W` | `Ctrl+Shift+W` |
| Copy / paste | `Cmd+C` / `Cmd+V` | `Ctrl+Shift+C` / `Ctrl+Shift+V` |
| Complete | `Tab` | `Tab` |
| Search history | `Ctrl+R` | `Ctrl+R` |
| New line in the editor | `Shift+Enter` | `Shift+Enter` |
| Clear blocks | `Cmd+K` | `Ctrl+Shift+K` |
| Font size | `Cmd+=` / `Cmd+-` / `Cmd+0` | `Ctrl+=` / `Ctrl+-` / `Ctrl+0` |
| Settings | `Cmd+,` | `Ctrl+,` |

The editor also understands the usual readline keys (`Ctrl+A`, `Ctrl+E`,
`Ctrl+K`, `Ctrl+U`, `Ctrl+W`) and, on macOS, the system text shortcuts
(`Option+←/→`, `Cmd+←/→`, `Cmd+Backspace`).

## Configuration

`Cmd+,` opens **Settings** for the everyday options: theme (match the system
or manual), tab position, font size, command durations, compact blocks and
update checks. Changes apply immediately and are saved to `config.toml`,
whose comments are kept.

Everything lives in `~/.config/rterm/`:

| Path | What it is |
| --- | --- |
| `config.toml` | Settings. Created on first launch, fully commented. |
| `themes/*.toml` | Your own color themes. |
| `workflows/*.toml` | Your workflows. |
| `completions/*.toml` | Extra completion descriptions. |
| `history.db` | Local history database (SQLite). |

`config.toml` only needs the keys you want to change:

```toml
[font]
family = "JetBrains Mono"
size = 14.0

[appearance]
theme_mode = "manual"   # or "system", with light_theme and dark_theme
theme = "Nord"
tabs = "vertical"

[blocks]
show_duration = false
compact = true          # tighter blocks, no line between them

[history]
record = false   # do not keep the local history database
```

**Settings → Open Settings File…** opens it in your text editor.

## Shell integration

Blocks and the input editor rely on a small integration for **zsh** and
**bash** (3.2, the one macOS ships, and later) that RTerm loads automatically:
it tells RTerm when each command starts and ends. It lives in
`~/.config/rterm/shell/`, loads your usual configuration first (`.zshrc`,
`.bashrc`, `.bash_profile`…) and never modifies your dotfiles.

RTerm uses your login shell (`$SHELL`); to pick another one:

```toml
[shell]
program = "/bin/bash"
```

Because RTerm draws its own input editor, your prompt and line-editing plugins
(prompt themes, `zsh-syntax-highlighting`, `zsh-autosuggestions`, `ble.sh`)
are not displayed. Your aliases, functions, environment and history all work
as usual.

Other shells (fish, nushell, and every shell on Windows for now) run as a
classic terminal. To get the
same classic behavior with zsh or bash, set `integration = false` in the
`[shell]` section.

## Build from source

You need a recent stable [Rust toolchain](https://rustup.rs). On macOS, add
Xcode's command line tools; on Windows, the Visual Studio C++ build tools; on
Debian or Ubuntu, the libraries GPUI uses:

```bash
sudo apt install libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libxcb1-dev libx11-xcb-dev libfontconfig-dev libfreetype-dev libvulkan-dev libssl-dev libzstd-dev
```

Then:

```bash
git clone https://github.com/Rawz06-Studio/RTerm.git
cd RTerm
cargo run
```

To produce the same signed `RTerm.app`, `.dmg` and `.zip` as a release:

```bash
scripts/bundle-macos.sh
```

Set `RTERM_ARCH=native` to build only for your machine's architecture, which
is about twice as fast. On Linux, `scripts/bundle-linux.sh` produces the
`.tar.gz` and the `.AppImage`; on Windows, `scripts/bundle-windows.ps1`
produces the installer (with [Inno Setup](https://jrsoftware.org/isinfo.php))
and the portable `.zip`.

## Releases

Every push to the default branch runs the tests and publishes a new release
with its patch number incremented (`v0.1.0`, `v0.1.1`, …). To bump the minor or
major number, run the **Release** workflow manually and pick the part to
increment.

The version of a build is shown in **About RTerm** (in the RTerm menu on
macOS, in the command palette everywhere) and printed by `rterm --version`.

## Known limitations

- Blocks and the input editor require zsh or bash, on macOS or Linux.
- Windows is a preview: classic terminal only, no automatic updates.
- Linux: no automatic updates yet. RTerm asks the system to draw its title
  bar; desktops that leave that to applications (GNOME on Wayland) may show
  none.
- Mouse clicks are not forwarded to terminal programs yet.
- Split panes always share the space equally.

## License

[MIT](LICENSE)
