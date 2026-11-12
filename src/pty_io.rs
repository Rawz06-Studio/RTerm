//! Moves bytes between the PTY and RTerm, on threads of our own.
//!
//! - Unix: blocking reads and writes on copies of the PTY's file.
//! - Windows: ConPTY's pipes are non-blocking: they are read when `polling`
//!   reports them ready, and written again when they are full.
//!
//! Either way, what the shell prints goes to `chunks`, and what arrives on
//! `inputs` goes to the shell.

use std::borrow::Cow;
use std::io::ErrorKind;
#[cfg(unix)]
use std::io::{Read, Write};
use std::sync::mpsc::{Receiver, SyncSender};

use alacritty_terminal::event::{OnResize, WindowSize};
use alacritty_terminal::tty;
use anyhow::Result;

const READ_BUFFER: usize = 64 * 1024;

pub struct PtyIo {
    #[cfg(unix)]
    pty: tty::Pty,
    #[cfg(windows)]
    pty: std::sync::Arc<std::sync::Mutex<tty::Pty>>,
}

impl PtyIo {
    /// Starts the reading and writing threads.
    pub fn start(
        pty: tty::Pty,
        chunks: SyncSender<Vec<u8>>,
        inputs: Receiver<Cow<'static, [u8]>>,
    ) -> Result<PtyIo> {
        #[cfg(unix)]
        {
            use anyhow::Context as _;
            // alacritty opens the PTY in non-blocking mode, for its own loop.
            // Our threads do blocking reads and writes.
            let reader = pty.file().try_clone().context("duplicating the PTY")?;
            let writer = pty.file().try_clone().context("duplicating the PTY")?;
            unix::set_blocking(&reader).context("switching the PTY to blocking mode")?;
            std::thread::Builder::new()
                .name("pty-read".into())
                .spawn(move || unix::read_loop(reader, chunks))?;
            std::thread::Builder::new()
                .name("pty-write".into())
                .spawn(move || unix::write_loop(writer, inputs))?;
            Ok(PtyIo { pty })
        }
        #[cfg(windows)]
        {
            let pty = std::sync::Arc::new(std::sync::Mutex::new(pty));
            let reader = pty.clone();
            let writer = pty.clone();
            std::thread::Builder::new()
                .name("pty-read".into())
                .spawn(move || windows::read_loop(reader, chunks))?;
            std::thread::Builder::new()
                .name("pty-write".into())
                .spawn(move || windows::write_loop(writer, inputs))?;
            Ok(PtyIo { pty })
        }
    }

    /// Tells the shell the new size of the window (SIGWINCH on Unix).
    pub fn resize(&mut self, size: WindowSize) {
        #[cfg(unix)]
        self.pty.on_resize(size);
        #[cfg(windows)]
        if let Ok(mut pty) = self.pty.lock() {
            pty.on_resize(size);
        }
    }

    /// The shell's process id.
    #[cfg(all(test, unix))]
    pub fn child_id(&self) -> u32 {
        self.pty.child().id()
    }
}

fn should_retry(kind: ErrorKind) -> bool {
    kind == ErrorKind::Interrupted
}

#[cfg(unix)]
mod unix {
    use std::fs::File;
    use std::os::fd::AsRawFd;

    use super::*;

    pub fn read_loop(mut pty: File, chunks: SyncSender<Vec<u8>>) {
        let mut buffer = vec![0u8; READ_BUFFER];
        loop {
            match pty.read(&mut buffer) {
                // End of file or error: the shell has exited.
                Ok(0) => break,
                Ok(read) => {
                    #[cfg(test)]
                    crate::terminal::tests::record_read(&buffer[..read]);
                    if chunks.send(buffer[..read].to_vec()).is_err() {
                        break;
                    }
                }
                Err(error) if should_retry(error.kind()) => {}
                Err(_) => break,
            }
        }
    }

    pub fn write_loop(mut pty: File, inputs: Receiver<Cow<'static, [u8]>>) {
        for input in inputs {
            if pty.write_all(&input).is_err() {
                break;
            }
        }
    }

    pub fn set_blocking(file: &File) -> std::io::Result<()> {
        let fd = file.as_raw_fd();
        // SAFETY: `fd` is an open descriptor, owned by `file` for the duration of the call.
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFL, 0);
            if flags < 0 || libc::fcntl(fd, libc::F_SETFL, flags & !libc::O_NONBLOCK) < 0 {
                return Err(std::io::Error::last_os_error());
            }
        }
        Ok(())
    }
}

#[cfg(windows)]
mod windows {
    use std::io::{Read, Write};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use alacritty_terminal::tty::{ChildEvent, EventedPty, EventedReadWrite};
    use polling::{Event, Events, PollMode, Poller};

    use super::*;

    pub fn read_loop(pty: Arc<Mutex<tty::Pty>>, chunks: SyncSender<Vec<u8>>) {
        let Ok(poller) = Poller::new().map(Arc::new) else {
            return;
        };
        {
            let Ok(mut pty) = pty.lock() else {
                return;
            };
            // SAFETY: the PTY is registered with this poller only, and
            // deregistered below, before the poller is dropped.
            if unsafe { pty.register(&poller, Event::readable(0), PollMode::Level) }.is_err() {
                return;
            }
        }

        let mut events = Events::new();
        let mut buffer = vec![0u8; READ_BUFFER];
        'poll: loop {
            events.clear();
            if let Err(error) = poller.wait(&mut events, None) {
                if should_retry(error.kind()) {
                    continue;
                }
                break;
            }
            let Ok(mut pty) = pty.lock() else {
                break;
            };
            // Everything available, including what the shell printed just
            // before exiting.
            loop {
                match pty.reader().read(&mut buffer) {
                    Ok(0) => break 'poll,
                    Ok(read) => {
                        if chunks.send(buffer[..read].to_vec()).is_err() {
                            break 'poll;
                        }
                    }
                    Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                    Err(error) if should_retry(error.kind()) => {}
                    Err(_) => break 'poll,
                }
            }
            let exited = events.iter().any(|event| event.key == tty::PTY_CHILD_EVENT_TOKEN)
                && matches!(pty.next_child_event(), Some(ChildEvent::Exited(_)));
            if exited {
                break;
            }
        }
        if let Ok(mut pty) = pty.lock() {
            let _ = pty.deregister(&poller);
        }
    }

    pub fn write_loop(pty: Arc<Mutex<tty::Pty>>, inputs: Receiver<Cow<'static, [u8]>>) {
        for input in inputs {
            let mut rest: &[u8] = &input;
            while !rest.is_empty() {
                let written = match pty.lock() {
                    Ok(mut pty) => pty.writer().write(rest),
                    Err(_) => return,
                };
                match written {
                    Ok(0) => return,
                    Ok(count) => rest = &rest[count..],
                    // The pipe is full: the shell has not read it yet.
                    Err(error) if error.kind() == ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(2));
                    }
                    Err(error) if should_retry(error.kind()) => {}
                    Err(_) => return,
                }
            }
        }
    }
}
