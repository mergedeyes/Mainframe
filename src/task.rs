//! Background commands whose output streams into the output pane.

use std::{
    io::{BufRead, BufReader, Read},
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::mpsc::{self, Receiver, TryRecvError},
    thread,
    time::{Duration, Instant},
};

use ratatui::text::Line;

use crate::{
    ansi,
    locate::{self, Loc},
};

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

impl Drop for Task {
    /// A task replaced while still running (rerun, watch mode) is killed and reaped,
    /// so it doesn't linger as a zombie.
    fn drop(&mut self) {
        if let Ok(None) = self.child.try_wait() {
            self.kill();
            let _ = self.child.wait();
        }
    }
}

#[derive(Default)]
pub struct Output {
    pub lines: Vec<Line<'static>>,
    /// Lines scrolled up from the bottom; 0 means following new output.
    pub scroll: usize,
    /// Visible rows, set by the UI each frame.
    pub height: usize,
    /// Where the task ran; relative paths in its output start here.
    cwd: PathBuf,
    /// `file:line` references, by line index, in order.
    pub locs: Vec<(usize, Loc)>,
    /// Index into `locs` of the one picked with n / N.
    pub cursor: Option<usize>,
    /// Text searched for with `/` (lowercase); while set, n / N walk its matches.
    pub search: String,
    /// Line indices that contain `search`.
    pub matches: Vec<usize>,
    /// Index into `matches` of the current one.
    pub match_cursor: Option<usize>,
}

fn plain(line: &Line) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

impl Output {
    pub fn new(cwd: &Path) -> Self {
        Self { cwd: cwd.to_path_buf(), ..Self::default() }
    }

    pub fn push(&mut self, line: Line<'static>) {
        let text = plain(&line);
        if let Some(loc) = locate::find(&text, &self.cwd) {
            self.locs.push((self.lines.len(), loc));
        }
        if !self.search.is_empty() && text.to_lowercase().contains(&self.search) {
            self.matches.push(self.lines.len());
        }
        self.lines.push(line);
        if self.scroll > 0 {
            // Keep the view anchored while the user is reading back.
            self.scroll += 1;
        }
        if self.lines.len() > MAX_LINES {
            let n = MAX_LINES / 10;
            self.lines.drain(..n);
            let gone = self.locs.iter().take_while(|(i, _)| *i < n).count();
            self.locs.drain(..gone);
            for (i, _) in &mut self.locs {
                *i -= n;
            }
            self.cursor = self.cursor.and_then(|c| c.checked_sub(gone));
            let gone = self.matches.iter().take_while(|&&i| i < n).count();
            self.matches.drain(..gone);
            for i in &mut self.matches {
                *i -= n;
            }
            self.match_cursor = self.match_cursor.and_then(|c| c.checked_sub(gone));
        }
        self.scroll = self.scroll.min(self.max_scroll());
    }

    /// The location picked with n / N.
    pub fn selected_loc(&self) -> Option<&(usize, Loc)> {
        self.locs.get(self.cursor?)
    }

    /// Picks the next (or previous) location, wrapping around, and scrolls it into the
    /// middle of the pane. False when there are none.
    pub fn step_loc(&mut self, forward: bool) -> bool {
        let n = self.locs.len();
        if n == 0 {
            return false;
        }
        let c = match (self.cursor, forward) {
            (None, true) => 0,
            (None, false) => n - 1,
            (Some(c), true) => (c + 1) % n,
            (Some(c), false) => (c + n - 1) % n,
        };
        self.cursor = Some(c);
        self.reveal(self.locs[c].0);
        true
    }

    /// Sets the search and jumps to the newest match. False when nothing matches.
    pub fn set_search(&mut self, query: &str) -> bool {
        self.search = query.to_lowercase();
        self.match_cursor = None;
        self.matches = if self.search.is_empty() {
            Vec::new()
        } else {
            (0..self.lines.len()).filter(|&i| plain(&self.lines[i]).to_lowercase().contains(&self.search)).collect()
        };
        self.step_match(false)
    }

    pub fn clear_search(&mut self) {
        self.search.clear();
        self.matches.clear();
        self.match_cursor = None;
    }

    /// Line index of the current search match.
    pub fn current_match(&self) -> Option<usize> {
        self.matches.get(self.match_cursor?).copied()
    }

    /// Next (or previous) search match, wrapping around.
    pub fn step_match(&mut self, forward: bool) -> bool {
        let n = self.matches.len();
        if n == 0 {
            return false;
        }
        let c = match (self.match_cursor, forward) {
            (None, true) => 0,
            (None, false) => n - 1,
            (Some(c), true) => (c + 1) % n,
            (Some(c), false) => (c + n - 1) % n,
        };
        self.match_cursor = Some(c);
        self.reveal(self.matches[c]);
        true
    }

    /// Scrolls so `line` sits in the middle of the pane.
    fn reveal(&mut self, line: usize) {
        let end = (line + self.height.div_ceil(2)).clamp(self.height.min(self.lines.len()), self.lines.len());
        self.scroll = self.lines.len() - end;
    }

    /// The whole output as plain text, for saving.
    pub fn text(&self) -> String {
        self.lines.iter().map(|l| plain(l) + "\n").collect()
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
