//! Background commands whose output streams into the output pane.

use std::{
    io::{BufRead, BufReader, Read},
    os::unix::process::CommandExt,
    path::Path,
    process::{Child, Command, ExitStatus, Stdio},
    sync::mpsc::{self, Receiver, TryRecvError},
    thread,
    time::{Duration, Instant},
};

use ratatui::text::Line;

use crate::ansi;

const MAX_LINES: usize = 20_000;

pub struct Task {
    pub label: String,
    child: Child,
    rx: Receiver<String>,
    pub started: Instant,
    readers_done: bool,
}

impl Task {
    pub fn spawn(label: String, cmd: &str, cwd: &Path) -> std::io::Result<Self> {
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg(cmd)
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // stdout isn't a tty, so ask tools to keep their colors anyway.
            .env("CARGO_TERM_COLOR", "always")
            .env("CLICOLOR_FORCE", "1")
            .env("FORCE_COLOR", "1")
            .env("GIT_TERMINAL_PROMPT", "0");
        // New session: no controlling terminal, so anything that wants a password
        // (sudo, ssh) fails fast instead of fighting the TUI for the tty, and the
        // process group lets `kill` take grandchildren (e.g. the binary under `cargo run`) too.
        unsafe {
            command.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
        let mut child = command.spawn()?;

        let (tx, rx) = mpsc::channel();
        let out = child.stdout.take().map(|s| Box::new(s) as Box<dyn Read + Send>);
        let err = child.stderr.take().map(|s| Box::new(s) as Box<dyn Read + Send>);
        for stream in [out, err].into_iter().flatten() {
            let tx = tx.clone();
            thread::spawn(move || {
                for chunk in BufReader::new(stream).split(b'\n') {
                    let Ok(bytes) = chunk else { break };
                    if tx.send(String::from_utf8_lossy(&bytes).into_owned()).is_err() {
                        break;
                    }
                }
            });
        }

        Ok(Self { label, child, rx, started: Instant::now(), readers_done: false })
    }

    /// Moves new output into `out` and reports completion once the process has
    /// exited *and* all of its output has been read.
    pub fn poll(&mut self, out: &mut Output) -> Option<(ExitStatus, Duration)> {
        loop {
            match self.rx.try_recv() {
                Ok(line) => out.push(ansi::parse_line(&line)),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.readers_done = true;
                    break;
                }
            }
        }
        if !self.readers_done {
            return None;
        }
        match self.child.try_wait() {
            Ok(Some(status)) => Some((status, self.started.elapsed())),
            _ => None,
        }
    }

    pub fn kill(&mut self) {
        let pid = self.child.id() as libc::pid_t;
        unsafe {
            libc::killpg(pid, libc::SIGTERM);
        }
        let _ = self.child.kill();
    }
}

#[derive(Default)]
pub struct Output {
    pub lines: Vec<Line<'static>>,
    /// Lines scrolled up from the bottom; 0 means following new output.
    pub scroll: usize,
    /// Visible rows, set by the UI each frame.
    pub height: usize,
}

impl Output {
    pub fn clear(&mut self) {
        self.lines.clear();
        self.scroll = 0;
    }

    pub fn push(&mut self, line: Line<'static>) {
        self.lines.push(line);
        if self.scroll > 0 {
            // Keep the view anchored while the user is reading back.
            self.scroll += 1;
        }
        if self.lines.len() > MAX_LINES {
            self.lines.drain(..MAX_LINES / 10);
        }
        self.scroll = self.scroll.min(self.max_scroll());
    }

    /// Furthest scroll that still fills the pane: the first line at the top.
    pub fn max_scroll(&self) -> usize {
        self.lines.len().saturating_sub(self.height)
    }

    pub fn scroll_by(&mut self, delta: isize) {
        self.scroll = (self.scroll as isize + delta).clamp(0, self.max_scroll() as isize) as usize;
    }

    pub fn scroll_to_top(&mut self) {
        self.scroll = self.max_scroll();
    }
}
