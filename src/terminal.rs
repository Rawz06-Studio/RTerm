//! The session: a shell in a PTY, whose output is sorted into blocks.
//!
//! Three threads do the work off the UI: one reads the PTY, one interprets
//! what was read, one writes the input. The shared state (`Session`) is
//! protected by a lock that the UI takes in order to draw.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{self, Osc52, Term, TermMode};
use alacritty_terminal::tty;
use alacritty_terminal::vte::ansi::{CursorShape, CursorStyle, Processor, StdSyncHandler};
use anyhow::{Context as _, Result};
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};

use crate::block::{Block, LiveBlock};
use crate::config::{Config, CursorShapeConfig};
use crate::osc::{Mark, Scanner, Segment};
use crate::pty_io::PtyIo;

const ZSH_INTEGRATION: &str = include_str!("shell/zshenv.zsh");
const BASH_INTEGRATION: &str = include_str!("shell/bashrc.bash");
const WSL_START: &str = include_str!("shell/wsl-start.sh");
const POWERSHELL_INTEGRATION: &str = include_str!("shell/rterm.ps1");

/// Number of finished blocks kept in memory.
const MAX_BLOCKS: usize = 1000;

/// Relays the emulator's events to the UI.
#[derive(Clone)]
pub struct Listener(UnboundedSender<Event>);

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        let _ = self.0.unbounded_send(event);
    }
}

/// Size of the grid, in cells and in pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridSize {
    pub columns: usize,
    pub lines: usize,
    pub cell_width: f32,
    pub cell_height: f32,
}

impl GridSize {
    pub fn window_size(&self) -> WindowSize {
        WindowSize {
            num_lines: self.lines as u16,
            num_cols: self.columns as u16,
            cell_width: self.cell_width as u16,
            cell_height: self.cell_height as u16,
        }
    }
}

impl Dimensions for GridSize {
    fn total_lines(&self) -> usize {
        self.lines
    }

    fn screen_lines(&self) -> usize {
        self.lines
    }

    fn columns(&self) -> usize {
        self.columns
    }
}

/// What the UI should display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The emulator takes the whole window: a shell without integration, or
    /// a full-screen program (vim, less, htop…).
    Fullscreen,
    /// Block list; a command is running and receives the keyboard directly.
    Running,
    /// Block list and input editor.
    Editing,
}

/// A command that just finished, for the history.
pub struct FinishedCommand {
    pub command: String,
    pub cwd: Option<PathBuf>,
    pub exit: Option<i32>,
    pub duration: Duration,
}

pub struct Session {
    /// Receives everything the shell prints outside commands: the prompt and
    /// the echo of the input. Hidden as soon as shell integration is active.
    pub prompt: Term<Listener>,
    pub blocks: Vec<Block>,
    pub live: Option<LiveBlock>,
    /// True once the shell has reported a first prompt.
    pub integrated: bool,
    /// Whether the shell reports the moment a line is accepted (zsh). Bash
    /// does not: RTerm then opens the block itself when it sends a command.
    reports_line_finish: bool,
    pub cwd: Option<PathBuf>,
    /// The command sent by the editor, waiting to be picked up.
    pub pending_command: Option<String>,
    /// Whether the shell is waiting at a prompt. False between the end of a
    /// command and the next prompt, when a submitted command would be lost.
    at_prompt: bool,
    /// Exit code of the last command.
    pub last_exit: Option<i32>,
    /// The shell's `PATH`, as it reported it.
    pub path: Option<String>,
    /// The shell's aliases, functions and builtins, if it reported them.
    pub shell_commands: Option<Arc<HashSet<String>>>,
    /// Finished commands, waiting to be written to the history.
    pub finished: Vec<FinishedCommand>,
    next_id: u64,
    size: GridSize,
    term_config: term::Config,
    listener: Listener,
}

impl Session {
    fn new_term(&self) -> Term<Listener> {
        Term::new(self.term_config.clone(), &self.size, self.listener.clone())
    }

    fn next_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    /// The emulator currently receiving the shell's output and the keyboard.
    pub fn active_term(&self) -> &Term<Listener> {
        self.live.as_ref().map_or(&self.prompt, |live| &live.term)
    }

    pub fn active_term_mut(&mut self) -> &mut Term<Listener> {
        match &mut self.live {
            Some(live) => &mut live.term,
            None => &mut self.prompt,
        }
    }

    pub fn mode(&self) -> Mode {
        if !self.integrated || self.active_term().mode().contains(TermMode::ALT_SCREEN) {
            Mode::Fullscreen
        } else if self.live.is_some() || self.pending_command.is_some() || !self.at_prompt {
            Mode::Running
        } else {
            Mode::Editing
        }
    }

    fn resize(&mut self, size: GridSize) {
        self.size = size;
        self.prompt.resize(size);
        if let Some(live) = &mut self.live {
            live.term.resize(size);
        }
    }

    /// Sends a command typed in the editor: it is shown as soon as the
    /// shell starts it.
    pub fn submit(&mut self, command: String) {
        self.pending_command = Some(command);
        if !self.reports_line_finish && self.live.is_none() {
            // What the shell prints before running the command (its echo, a
            // syntax error) goes to this block.
            self.start_block(false);
        }
    }

    fn start_block(&mut self, executing: bool) {
        self.at_prompt = false;
        let live = LiveBlock {
            id: self.next_id(),
            command: self.pending_command.take().unwrap_or_default(),
            cwd: self.cwd.clone(),
            term: self.new_term(),
            started: Instant::now(),
            executing,
        };
        self.live = Some(live);
    }

    fn finish_block(&mut self, exit: Option<i32>) {
        let Some(live) = self.live.take() else {
            return;
        };
        let executed = live.executing;
        if executed {
            self.last_exit = exit;
            if !live.command.trim().is_empty() {
                self.finished.push(FinishedCommand {
                    command: live.command.clone(),
                    cwd: live.cwd.clone(),
                    exit,
                    duration: live.started.elapsed(),
                });
            }
        }
        if executed && live.command.trim() == "clear" {
            self.blocks.clear();
            return;
        }
        let block = Block::freeze(live, exit);
        // Empty line, comment, Ctrl+C at the prompt: nothing to show.
        if !executed && block.is_empty() {
            return;
        }
        self.push_block(block);
    }

    fn push_block(&mut self, block: Block) {
        if self.blocks.len() >= MAX_BLOCKS {
            self.blocks.remove(0);
        }
        self.blocks.push(block);
    }

    fn handle_mark(&mut self, mark: Mark) {
        match mark {
            Mark::PromptStart => {
                self.finish_block(None);
                if !self.integrated {
                    self.integrated = true;
                    // What the shell printed at startup becomes a block.
                    let id = self.next_id();
                    let mut startup = Block::from_term(id, &self.prompt);
                    startup.wrap(self.size.columns);
                    // Blank lines (what ConPTY draws on Windows) or macOS's
                    // "Last login" alone say nothing useful: we only keep the
                    // block if the shell said something else.
                    let text = startup.text(None);
                    let text = text.trim();
                    let only_last_login = text.lines().count() == 1 && text.starts_with("Last login");
                    if !text.is_empty() && !only_last_login {
                        self.push_block(startup);
                    }
                }
                self.pending_command = None;
                self.at_prompt = true;
                // The previous prompt is no longer needed: start from an empty screen.
                Processor::<StdSyncHandler>::new()
                    .advance(&mut self.prompt, b"\x1b[H\x1b[2J\x1b[3J");
            }
            Mark::LineFinish => {
                self.reports_line_finish = true;
                if self.live.is_none() {
                    self.start_block(false);
                }
            }
            Mark::CommandStart => match &mut self.live {
                Some(live) if !live.executing => {
                    // Discard what zsh printed between accepting the line and
                    // launching the command.
                    live.executing = true;
                    live.started = Instant::now();
                    let term = Term::new(self.term_config.clone(), &self.size, self.listener.clone());
                    live.term = term;
                }
                Some(_) => {}
                None => self.start_block(true),
            },
            Mark::CommandEnd(exit) => self.finish_block(exit),
            Mark::Cwd(path) => self.cwd = Some(PathBuf::from(path)),
            Mark::Path(path) => self.path = Some(path),
            Mark::Commands(names) => {
                let names = names.split_whitespace().map(str::to_owned).collect();
                self.shell_commands = Some(Arc::new(names));
            }
        }
    }
}

pub struct Terminal {
    pub session: Arc<FairMutex<Session>>,
    writer: Sender<Cow<'static, [u8]>>,
    io: PtyIo,
    size: GridSize,
}

impl Terminal {
    /// Launches the shell. `cwd` takes precedence over the config's start directory.
    pub fn spawn(
        config: &Config,
        size: GridSize,
        cwd: Option<PathBuf>,
    ) -> Result<(Terminal, UnboundedReceiver<Event>)> {
        let (events_tx, events_rx) = unbounded();
        let listener = Listener(events_tx);

        let term_config = term::Config {
            scrolling_history: config.terminal.scrollback,
            default_cursor_style: CursorStyle {
                shape: match config.terminal.cursor_shape {
                    CursorShapeConfig::Block => CursorShape::Block,
                    CursorShapeConfig::Beam => CursorShape::Beam,
                    CursorShapeConfig::Underline => CursorShape::Underline,
                },
                blinking: false,
            },
            osc52: if config.terminal.allow_osc52 {
                Osc52::OnlyCopy
            } else {
                Osc52::Disabled
            },
            ..term::Config::default()
        };

        let shell_program = shell_program(config);
        let mut env = HashMap::from([
            ("TERM".to_owned(), "xterm-256color".to_owned()),
            ("COLORTERM".to_owned(), "truecolor".to_owned()),
            ("TERM_PROGRAM".to_owned(), "RTerm".to_owned()),
            ("TERM_PROGRAM_VERSION".to_owned(), crate::VERSION.to_owned()),
        ]);
        let mut shell = (!config.shell.program.is_empty())
            .then(|| tty::Shell::new(config.shell.program.clone(), config.shell.args.clone()));
        // Windows always names the shell (alacritty's default is PowerShell).
        if cfg!(windows) && shell.is_none() {
            shell = Some(tty::Shell::new(shell_program.clone(), config.shell.args.clone()));
        }
        // An integration that fails to install does not prevent working: the
        // terminal then stays in classic display.
        let kind = Path::new(&shell_program)
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let integrate = |name: &str, result: Result<Vec<String>>| match result {
            Ok(args) => Some(tty::Shell::new(shell_program.clone(), args)),
            Err(error) => {
                eprintln!("rterm: {name} integration unavailable ({error:#})");
                None
            }
        };
        match kind.as_str() {
            _ if !config.shell.integration => {}
            "zsh" if cfg!(unix) => match install_integration("zsh", ".zshenv", ZSH_INTEGRATION) {
                Ok(script) => {
                    if let Ok(original) = std::env::var("ZDOTDIR") {
                        env.insert("RTERM_ORIG_ZDOTDIR".to_owned(), original);
                    }
                    let directory = script.parent().unwrap_or(&script);
                    env.insert("ZDOTDIR".to_owned(), directory.to_string_lossy().into_owned());
                }
                Err(error) => eprintln!("rterm: zsh integration unavailable ({error:#})"),
            },
            "bash" if cfg!(unix) => {
                // `--rcfile` only applies to non-login shells: the script
                // reads the login files itself where terminals start login
                // shells (macOS).
                if cfg!(target_os = "macos") {
                    env.insert("RTERM_BASH_LOGIN".to_owned(), "1".to_owned());
                    // You picked bash: macOS's advice to switch to zsh would
                    // only clutter the first block.
                    env.insert("BASH_SILENCE_DEPRECATION_WARNING".to_owned(), "1".to_owned());
                }
                let args = install_integration("bash", "rterm.bash", BASH_INTEGRATION).map(|script| {
                    let mut args = vec!["--rcfile".to_owned(), script.to_string_lossy().into_owned()];
                    args.extend(config.shell.args.iter().cloned());
                    args.push("-i".to_owned());
                    args
                });
                shell = integrate("bash", args).or(shell);
            }
            // WSL: a start script, run inside Linux, launches the login shell
            // with the zsh or bash integration. Only without arguments of
            // yours, which could name another distribution or command.
            "wsl" if cfg!(windows) && config.shell.args.is_empty() => {
                let args = install_wsl_integration().map(|directory| {
                    env.insert("RTERM_SHELL_DIR".to_owned(), directory.to_string_lossy().into_owned());
                    // Variables WSL passes on to Linux; `/p` translates the
                    // Windows path into a Linux one.
                    let mut shared = std::env::var("WSLENV").unwrap_or_default();
                    for name in ["RTERM_SHELL_DIR/p", "TERM", "COLORTERM", "TERM_PROGRAM", "TERM_PROGRAM_VERSION"] {
                        if !shared.is_empty() {
                            shared.push(':');
                        }
                        shared.push_str(name);
                    }
                    env.insert("WSLENV".to_owned(), shared);
                    ["--cd", "~", "-e", "sh", "-c", ". \"$RTERM_SHELL_DIR/wsl/start.sh\""]
                        .map(str::to_owned)
                        .to_vec()
                });
                shell = integrate("WSL", args).or(shell);
            }
            "pwsh" | "powershell" if cfg!(windows) => {
                let args = install_integration("powershell", "rterm.ps1", POWERSHELL_INTEGRATION).map(|script| {
                    let mut args = config.shell.args.clone();
                    let script = script.to_string_lossy().replace('\'', "''");
                    args.extend(["-NoLogo".to_owned(), "-NoExit".to_owned(), "-Command".to_owned()]);
                    args.push(format!(". '{script}'"));
                    args
                });
                shell = integrate("PowerShell", args).or(shell);
            }
            _ => {}
        }
        let working_directory = cwd.filter(|cwd| cwd.is_dir()).or_else(|| {
            if config.shell.working_directory.is_empty() {
                dirs::home_dir()
            } else {
                Some(PathBuf::from(&config.shell.working_directory))
            }
        });
        let options = tty::Options {
            shell,
            working_directory,
            drain_on_exit: true,
            env,
            #[cfg(windows)]
            escape_args: true,
        };
        let pty = tty::new(&options, size.window_size(), 0).context("opening the PTY")?;

        let session = Session {
            prompt: Term::new(term_config.clone(), &size, listener.clone()),
            blocks: Vec::new(),
            live: None,
            integrated: false,
            reports_line_finish: false,
            cwd: None,
            pending_command: None,
            at_prompt: false,
            last_exit: None,
            path: None,
            shell_commands: None,
            finished: Vec::new(),
            next_id: 0,
            size,
            term_config,
            listener: listener.clone(),
        };
        let session = Arc::new(FairMutex::new(session));

        let (chunks_tx, chunks_rx) = mpsc::sync_channel(64);
        let (writer_tx, writer_rx) = mpsc::channel();
        let io = PtyIo::start(pty, chunks_tx, writer_rx)?;
        let parser_session = session.clone();
        std::thread::Builder::new()
            .name("pty-parse".into())
            .spawn(move || parse_loop(chunks_rx, parser_session, listener))?;

        Ok((Terminal { session, writer: writer_tx, io, size }, events_rx))
    }

    /// Sends bytes to the shell, as if the user had typed them.
    pub fn write(&self, bytes: impl Into<Cow<'static, [u8]>>) {
        let bytes = bytes.into();
        if !bytes.is_empty() {
            let _ = self.writer.send(bytes);
        }
    }

    pub fn size(&self) -> GridSize {
        self.size
    }

    /// Resizes the grids and notifies the shell (SIGWINCH).
    pub fn resize(&mut self, size: GridSize) {
        if size == self.size {
            return;
        }
        self.size = size;
        self.session.lock().resize(size);
        self.io.resize(size.window_size());
    }
}

/// The shell that will be launched: the config's, otherwise the user's.
fn shell_program(config: &Config) -> String {
    if !config.shell.program.is_empty() {
        return config.shell.program.clone();
    }
    #[cfg(windows)]
    return windows_default_shell();
    #[cfg(not(windows))]
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_owned())
}

/// WSL when a Linux distribution is installed, otherwise PowerShell (the
/// newer `pwsh` if it is installed).
#[cfg(windows)]
fn windows_default_shell() -> String {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    // Without a console window flashing on the screen.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let quiet = |program: &str, args: &[&str]| {
        Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .ok()
            .filter(|output| output.status.success())
    };
    // `wsl --list` answers in UTF-16: any letter means a distribution.
    let has_distribution = quiet("wsl.exe", &["--list", "--quiet"])
        .is_some_and(|output| output.stdout.iter().any(|byte| byte.is_ascii_alphanumeric()));
    if has_distribution {
        "wsl.exe".to_owned()
    } else if quiet("pwsh.exe", &["-NoProfile", "-Command", "exit"]).is_some() {
        "pwsh.exe".to_owned()
    } else {
        "powershell.exe".to_owned()
    }
}

/// Writes what WSL needs (the start script and the zsh and bash
/// integrations) and returns the folder holding them.
fn install_wsl_integration() -> Result<PathBuf> {
    install_integration("zsh", ".zshenv", ZSH_INTEGRATION)?;
    install_integration("bash", "rterm.bash", BASH_INTEGRATION)?;
    let start = install_integration("wsl", "start.sh", WSL_START)?;
    let shell_directory = start.parent().and_then(Path::parent).context("integration folder")?;
    Ok(shell_directory.to_path_buf())
}

/// Writes an integration script to `~/.config/rterm/shell/<shell>/<file>`
/// and returns its path.
fn install_integration(shell: &str, file: &str, content: &str) -> Result<PathBuf> {
    let directory = dirs::home_dir()
        .context("home directory not found")?
        .join(".config/rterm/shell")
        .join(shell);
    std::fs::create_dir_all(&directory)
        .with_context(|| format!("creating {}", directory.display()))?;
    let script = directory.join(file);
    // Scripts run by Unix shells (WSL included) must not carry Windows line
    // endings, whatever the checkout they were built from.
    let content = content.replace("\r\n", "\n");
    let up_to_date = std::fs::read_to_string(&script).is_ok_and(|current| current == content);
    if !up_to_date {
        std::fs::write(&script, &content).with_context(|| format!("writing {}", script.display()))?;
    }
    Ok(script)
}

/// Interprets the shell's output and distributes it among the blocks.
fn parse_loop(chunks: Receiver<Vec<u8>>, session: Arc<FairMutex<Session>>, listener: Listener) {
    let mut processor = Processor::<StdSyncHandler>::new();
    let mut scanner = Scanner::default();

    loop {
        // A program may ask to group its screen updates; if it does not
        // finish them in time, we apply them anyway.
        let chunk = match processor.sync_timeout().sync_timeout() {
            Some(deadline) => {
                match chunks.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                    Ok(chunk) => Some(chunk),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
            None => match chunks.recv() {
                Ok(chunk) => Some(chunk),
                Err(_) => break,
            },
        };

        let mut session = session.lock();
        match chunk {
            None => processor.stop_sync(session.active_term_mut()),
            Some(chunk) => scanner.feed(&chunk, |segment| match segment {
                Segment::Bytes(bytes) => processor.advance(session.active_term_mut(), bytes),
                Segment::Mark(mark) => {
                    // A mark switches emulators: first flush what was pending
                    // for the current emulator.
                    processor.stop_sync(session.active_term_mut());
                    session.handle_mark(mark);
                }
            }),
        }
        drop(session);
        listener.send_event(Event::Wakeup);
    }

    listener.send_event(Event::Exit);
}

// These tests launch real shells: zsh and bash on Unix, PowerShell (and WSL,
// when a distribution is installed) on Windows.
#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Bytes read from all PTYs, to tell a silent shell from a lost output.
    static BYTES_READ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    static FIRST_BYTES: std::sync::Mutex<Vec<u8>> = std::sync::Mutex::new(Vec::new());

    /// Called by the PTY reader with each chunk it reads.
    pub fn record_read(bytes: &[u8]) {
        BYTES_READ.fetch_add(bytes.len(), std::sync::atomic::Ordering::Relaxed);
        let mut first = FIRST_BYTES.lock().unwrap();
        if first.len() < 600 {
            first.extend_from_slice(&bytes[..bytes.len().min(600)]);
        }
    }

    /// Waits for a condition on the session to become true.
    fn wait_for(terminal: &Terminal, what: &str, condition: impl Fn(&Session) -> bool) {
        // WSL can take a while to start its virtual machine.
        let deadline = Instant::now() + Duration::from_secs(if cfg!(windows) { 60 } else { 15 });
        while !condition(&terminal.session.lock()) {
            if Instant::now() >= deadline {
                // What the shell showed, to understand a failure in CI.
                let session = terminal.session.lock();
                let screen = Block::from_term(0, &session.prompt).text(None);
                #[cfg(unix)]
                // SAFETY: signal 0 only checks that the process exists.
                let alive = unsafe { libc::kill(terminal.io.child_id() as i32, 0) } == 0;
                #[cfg(windows)]
                let alive = "unknown";
                panic!(
                    "timed out: {what} (integrated: {}, mode: {:?}, blocks: {}, shell alive: {alive}, bytes read: {})\n--- screen ---\n{screen}\n--- raw ---\n{:?}",
                    session.integrated,
                    session.mode(),
                    session.blocks.len(),
                    BYTES_READ.load(std::sync::atomic::Ordering::Relaxed),
                    String::from_utf8_lossy(&FIRST_BYTES.lock().unwrap()),
                );
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Sends a command the way the editor does, then waits for its block.
    fn run(terminal: &Terminal, command: &str) {
        let before = terminal.session.lock().blocks.len();
        terminal.session.lock().submit(command.to_owned());
        terminal.write(format!("{command}\r").into_bytes());
        wait_for(terminal, command, |session| {
            session.blocks.len() > before && session.mode() == Mode::Editing
        });
    }

    fn last_block(terminal: &Terminal) -> (String, String, Option<i32>) {
        let mut session = terminal.session.lock();
        let block = session.blocks.last_mut().expect("at least one block");
        block.wrap(80);
        (block.command.clone(), block.text(None), block.exit)
    }

    /// Runs the same scenario in a real zsh and a real bash: their commands
    /// must become blocks. One test for both, run one after the other,
    /// because the zsh part changes an environment variable.
    #[cfg(unix)]
    #[test]
    fn shell_commands_become_blocks() {
        if Path::new("/bin/zsh").exists() {
            // An empty ZDOTDIR sets the machine's zsh configuration aside: the
            // test only depends on zsh and the integration script.
            let zdotdir = std::env::temp_dir().join(format!("rterm-test-{}", std::process::id()));
            std::fs::create_dir_all(&zdotdir).unwrap();
            // Debian and Ubuntu run compinit from /etc/zsh/zshrc, which asks a
            // question (and waits) when it finds directories it finds insecure,
            // as on GitHub's runners. This variable skips it.
            std::fs::write(zdotdir.join(".zshenv"), "skip_global_compinit=1\n").unwrap();
            // SAFETY: this test is the only one reading or modifying the environment.
            unsafe { std::env::set_var("ZDOTDIR", &zdotdir) };
            check_shell("/bin/zsh", &["-i"], "parse error");
            let _ = std::fs::remove_dir_all(&zdotdir);
        }
        for bash in ["/bin/bash", "/usr/bin/bash", "/opt/homebrew/bin/bash", "/usr/local/bin/bash"] {
            if Path::new(bash).exists() {
                check_shell(bash, &[], "syntax error");
            }
        }
    }

    fn check_shell(program: &str, args: &[&str], syntax_error: &str) {
        eprintln!("checking {program}");
        let mut config = Config::defaults();
        config.shell.program = program.into();
        config.shell.args = args.iter().map(|arg| arg.to_string()).collect();
        let size = GridSize { columns: 80, lines: 24, cell_width: 8.0, cell_height: 16.0 };
        let (terminal, _events) = Terminal::spawn(&config, size, None).unwrap();

        wait_for(&terminal, "first prompt", |session| session.mode() == Mode::Editing);
        assert!(terminal.session.lock().cwd.is_some(), "{program}: the current directory is reported");
        assert!(terminal.session.lock().path.is_some(), "{program}: the PATH is reported");
        let known = terminal.session.lock().shell_commands.clone().expect("shell commands are reported");
        assert!(known.contains("cd") && known.contains("if"), "{program}: builtins and reserved words");

        run(&terminal, "echo hello-rterm");
        assert_eq!(
            last_block(&terminal),
            ("echo hello-rterm".into(), "hello-rterm".into(), Some(0)),
            "{program}"
        );

        // Output without a trailing line feed, over several lines.
        run(&terminal, "printf 'a\\nb'");
        assert_eq!(last_block(&terminal).1, "a\nb", "{program}");

        // A pipeline is a single command.
        run(&terminal, "echo one | tr a-z A-Z");
        assert_eq!(last_block(&terminal).1, "ONE", "{program}");

        run(&terminal, "false");
        let (command, output, exit) = last_block(&terminal);
        assert_eq!((command.as_str(), output.as_str(), exit), ("false", "", Some(1)), "{program}");
        assert_eq!(terminal.session.lock().last_exit, Some(1), "{program}");
        let finished: Vec<(String, Option<i32>)> = terminal
            .session
            .lock()
            .finished
            .iter()
            .map(|done| (done.command.clone(), done.exit))
            .collect();
        assert_eq!(finished.last(), Some(&("false".to_owned(), Some(1))), "{program}");
        assert_eq!(finished.len(), 4, "{program}: one history entry per executed command");

        // A syntax error runs nothing, but its message stays visible, without
        // the echo of the command.
        run(&terminal, "echo )");
        let (command, output, _) = last_block(&terminal);
        assert_eq!(command, "echo )", "{program}");
        assert!(output.contains(syntax_error), "{program}: unexpected output: {output:?}");
        assert!(!output.starts_with("echo )"), "{program}: the echo is left out: {output:?}");

        // `clear` empties the block list.
        terminal.session.lock().submit("clear".into());
        terminal.write(b"clear\r".to_vec());
        wait_for(&terminal, "clear", |session| session.blocks.is_empty());
    }

    /// Windows PowerShell 5.1 and, when installed, PowerShell 7.
    #[cfg(windows)]
    #[test]
    fn powershell_commands_become_blocks() {
        check_powershell("powershell.exe");
        let has_pwsh = std::process::Command::new("pwsh.exe")
            .args(["-NoProfile", "-Command", "exit"])
            .status()
            .is_ok_and(|status| status.success());
        if has_pwsh {
            check_powershell("pwsh.exe");
        }
    }

    /// bash (or zsh) inside WSL, through the start script. Needs a WSL
    /// distribution: CI installs one and runs it with `--ignored`.
    #[cfg(windows)]
    #[test]
    #[ignore = "needs a WSL distribution"]
    fn wsl_commands_become_blocks() {
        check_shell("wsl.exe", &[], "syntax error");
        // RTerm cannot read the Linux PATH: the shell sends an empty one.
        let mut config = Config::defaults();
        config.shell.program = "wsl.exe".into();
        let size = GridSize { columns: 80, lines: 24, cell_width: 8.0, cell_height: 16.0 };
        let (terminal, _events) = Terminal::spawn(&config, size, None).unwrap();
        wait_for(&terminal, "first prompt", |session| session.mode() == Mode::Editing);
        assert_eq!(terminal.session.lock().path.as_deref(), Some(""));
    }

    #[cfg(windows)]
    fn check_powershell(program: &str) {
        eprintln!("checking {program}");
        let mut config = Config::defaults();
        config.shell.program = program.into();
        let size = GridSize { columns: 80, lines: 24, cell_width: 8.0, cell_height: 16.0 };
        let (terminal, _events) = Terminal::spawn(&config, size, None).unwrap();

        wait_for(&terminal, "first prompt", |session| session.mode() == Mode::Editing);
        assert!(terminal.session.lock().cwd.is_some(), "{program}: the current directory is reported");
        assert!(terminal.session.lock().path.is_some(), "{program}: the PATH is reported");
        let known = terminal.session.lock().shell_commands.clone().expect("shell commands are reported");
        assert!(
            ["cd", "Get-Command", "if"].iter().all(|name| known.contains(*name)),
            "{program}: aliases, loaded cmdlets and keywords"
        );

        run(&terminal, "Write-Output hello-rterm");
        assert_eq!(
            last_block(&terminal),
            ("Write-Output hello-rterm".into(), "hello-rterm".into(), Some(0)),
            "{program}"
        );

        run(&terminal, "'a', 'b' | Write-Output");
        assert_eq!(last_block(&terminal).1, "a\nb", "{program}");

        // A program's exit code.
        run(&terminal, "cmd /c exit 3");
        let (_, _, exit) = last_block(&terminal);
        assert_eq!(exit, Some(3), "{program}");
        assert_eq!(terminal.session.lock().last_exit, Some(3), "{program}");

        run(&terminal, "echo )");
        let (command, output, _) = last_block(&terminal);
        assert_eq!(command, "echo )", "{program}");
        assert!(output.contains("Unexpected token"), "{program}: unexpected output: {output:?}");

        terminal.session.lock().submit("clear".into());
        terminal.write(b"clear\r".to_vec());
        wait_for(&terminal, "clear", |session| session.blocks.is_empty());
    }
}
