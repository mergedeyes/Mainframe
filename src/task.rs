//! Background commands whose output streams into the output pane.

use std::{
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
    process::{ExitStatus, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender, TryRecvError},
    },
    thread,
    time::{Duration, Instant},
};

use ratatui::text::Line;

use crate::{
    ansi,
    cmd::{Cmd, Launch},
    locate::{self, Loc},
    platform::ProcessTree,
    shell::Shell,
};

const MAX_LINES: usize = 20_000;

/// How a task ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Exit {
    Code(i32),
    /// Stopped by Mainframe, or by a signal.
    Killed,
    /// A program couldn't be started at all.
    NotStarted,
}

impl Exit {
    pub fn ok(&self) -> bool {
        *self == Exit::Code(0)
    }

    pub fn describe(&self) -> String {
        match self {
            Exit::Code(c) => format!("exit {c}"),
            Exit::Killed => "killed".into(),
            Exit::NotStarted => "didn't start".into(),
        }
    }
}

enum Msg {
    Line(String),
    Done(Exit),
}

/// Shared with the thread that runs the steps, so a kill reaches whichever one is running.
#[derive(Default)]
struct Control {
    stopped: AtomicBool,
    running: Mutex<Option<ProcessTree>>,
}

pub struct Task {
    pub label: String,
    rx: Receiver<Msg>,
    control: Arc<Control>,
    pub started: Instant,
    finished: bool,
}

impl Task {
    pub fn spawn(label: String, cmd: &Cmd, cwd: &Path, shell: &Shell) -> std::io::Result<Self> {
        let launches = cmd.launches(shell)?;
        let (tx, rx) = mpsc::channel();
        let control = Arc::new(Control::default());
        let ctl = control.clone();
        let cwd = cwd.to_path_buf();
        thread::Builder::new().name(format!("task {label}")).spawn(move || {
            let exit = run(launches, &cwd, &tx, &ctl);
            let _ = tx.send(Msg::Done(exit));
        })?;
        Ok(Self { label, rx, control, started: Instant::now(), finished: false })
    }

    /// Moves new output into `out` and reports completion once the last program has
    /// exited *and* all of its output has been read.
    pub fn poll(&mut self, out: &mut Output) -> Option<(Exit, Duration)> {
        if self.finished {
            return None;
        }
        loop {
            match self.rx.try_recv() {
                Ok(Msg::Line(line)) => out.push(ansi::parse_line(&line)),
                Ok(Msg::Done(exit)) => {
                    self.finished = true;
                    return Some((exit, self.started.elapsed()));
                }
                Err(TryRecvError::Empty) => return None,
                Err(TryRecvError::Disconnected) => {
                    self.finished = true;
                    return Some((Exit::NotStarted, self.started.elapsed()));
                }
            }
        }
    }

    /// Asks the running program and everything it started to stop, without waiting for it.
    pub fn signal(&self) {
        self.control.stopped.store(true, Ordering::SeqCst);
        if let Some(tree) = &*self.control.running.lock().unwrap_or_else(|e| e.into_inner()) {
            tree.terminate();
        }
    }

    pub fn kill(&mut self) {
        self.control.stopped.store(true, Ordering::SeqCst);
        if let Some(tree) = &*self.control.running.lock().unwrap_or_else(|e| e.into_inner()) {
            tree.kill();
        }
    }
}

impl Drop for Task {
    /// A task replaced while still running (rerun, watch mode) is killed; its thread reaps it.
    fn drop(&mut self) {
        if !self.finished {
            self.kill();
        }
    }
}

/// Runs the programs in order on the task's thread, stopping at the first that fails.
fn run(launches: Vec<Launch>, cwd: &Path, tx: &Sender<Msg>, control: &Control) -> Exit {
    let several = launches.len() > 1;
    let mut first = true;
    for Launch { mut command, display, unless_exists } in launches {
        if control.stopped.load(Ordering::SeqCst) {
            return Exit::Killed;
        }
        if unless_exists.is_some_and(|p| cwd.join(p).exists()) {
            continue;
        }
        if several && !first {
            let _ = tx.send(Msg::Line(format!("\x1b[2m$ {display}\x1b[0m")));
        }
        first = false;
        command
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // stdout isn't a tty, so ask tools to keep their colors anyway.
            .env("CARGO_TERM_COLOR", "always")
            .env("CLICOLOR_FORCE", "1")
            .env("FORCE_COLOR", "1")
            .env("GIT_TERMINAL_PROMPT", "0");
        // Away from the terminal (password prompts fail fast instead of fighting the TUI
        // for it), and grouped so stopping it takes grandchildren like the binary under
        // `cargo run` along.
        ProcessTree::prepare(&mut command);
        let mut child = match command.spawn() {
            Ok(c) => c,
            Err(e) => {
                let program = command.get_program().to_string_lossy().into_owned();
                let _ = tx.send(Msg::Line(format!("\x1b[31mcouldn't start {program}: {e}\x1b[0m")));
                return Exit::NotStarted;
            }
        };
        {
            let mut running = control.running.lock().unwrap_or_else(|e| e.into_inner());
            let tree = ProcessTree::attach(&child);
            // A kill that came while it was starting.
            if control.stopped.load(Ordering::SeqCst) {
                tree.kill();
            }
            *running = Some(tree);
        }
        let out = child.stdout.take().map(|s| Box::new(s) as Box<dyn Read + Send>);
        let err = child.stderr.take().map(|s| Box::new(s) as Box<dyn Read + Send>);
        let readers: Vec<_> = [out, err]
            .into_iter()
            .flatten()
            .map(|stream| {
                let tx = tx.clone();
                thread::spawn(move || {
                    for chunk in BufReader::new(stream).split(b'\n') {
                        let Ok(mut bytes) = chunk else { break };
                        if bytes.last() == Some(&b'\r') {
                            bytes.pop();
                        }
                        if tx.send(Msg::Line(String::from_utf8_lossy(&bytes).into_owned())).is_err() {
                            break;
                        }
                    }
                })
            })
            .collect();
        for r in readers {
            let _ = r.join();
        }
        let status = child.wait();
        control.running.lock().unwrap_or_else(|e| e.into_inner()).take();
        let stopped = control.stopped.load(Ordering::SeqCst);
        match status {
            Ok(s) if s.success() => {}
            Ok(_) if stopped => return Exit::Killed,
            Ok(s) => return exit_of(s),
            Err(_) => return Exit::Killed,
        }
    }
    Exit::Code(0)
}

fn exit_of(status: ExitStatus) -> Exit {
    status.code().map_or(Exit::Killed, Exit::Code)
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

    /// Where the task ran.
    pub fn cwd(&self) -> &Path {
        &self.cwd
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{cmd::Step, testutil::temp_dir};

    /// Polls until the task ends; the plain text of its output, and how it ended.
    fn finish(cmd: Cmd, cwd: &Path) -> (Vec<String>, Exit) {
        let mut task = Task::spawn("t".into(), &cmd, cwd, &Shell::default()).unwrap();
        let mut out = Output::new(cwd);
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some((exit, _)) = task.poll(&mut out) {
                return (out.lines.iter().map(plain).collect(), exit);
            }
            assert!(Instant::now() < deadline, "task didn't finish");
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn steps_run_in_order_and_stop_at_the_first_failure() {
        let dir = temp_dir("task-steps");
        let cmd = Cmd::Steps(vec![
            Step::words("git --version"),
            Step::words("git no-such-command"),
            Step::words("git --version"),
        ]);
        let (lines, exit) = finish(cmd, &dir);
        assert_eq!(exit, Exit::Code(1));
        assert!(lines[0].starts_with("git version"), "{lines:?}");
        assert_eq!(lines.iter().filter(|l| l.starts_with("git version")).count(), 1, "{lines:?}");
        assert!(lines.iter().any(|l| l == "$ git no-such-command"), "{lines:?}");
    }

    #[test]
    fn steps_are_skipped_when_their_folder_exists() {
        let dir = temp_dir("task-skip");
        std::fs::create_dir(dir.join("build")).unwrap();
        let cmd = Step::words("git no-such-command").unless_exists("build").then(Step::words("git --version"));
        let (lines, exit) = finish(cmd, &dir);
        assert!(exit.ok(), "{lines:?}");
        assert!(lines.iter().all(|l| !l.contains("no-such-command")), "{lines:?}");
    }

    #[test]
    fn a_missing_program_says_so() {
        let dir = temp_dir("task-missing");
        let (lines, exit) = finish(Step::new("mainframe-no-such-program").into(), &dir);
        assert_eq!(exit, Exit::NotStarted);
        assert!(lines.iter().any(|l| l.starts_with("couldn't start mainframe-no-such-program")), "{lines:?}");
    }

    #[test]
    fn shell_lines_run_in_the_shell() {
        let dir = temp_dir("task-shell");
        let (lines, exit) = finish(Cmd::Shell("git --version".into()), &dir);
        assert!(exit.ok());
        assert!(lines.iter().any(|l| l.starts_with("git version")), "{lines:?}");
    }

    #[test]
    fn killing_stops_the_program_and_what_it_started() {
        let dir = temp_dir("task-kill");
        // A shell waiting on a child: both have to go for the task to end.
        let line = if cfg!(windows) { "Start-Sleep 30; Write-Output done" } else { "sleep 30; echo done" };
        let mut task = Task::spawn("t".into(), &Cmd::Shell(line.into()), &dir, &Shell::default()).unwrap();
        let mut out = Output::new(&dir);
        thread::sleep(Duration::from_millis(500));
        let started = Instant::now();
        task.kill();
        let exit = loop {
            if let Some((exit, _)) = task.poll(&mut out) {
                break exit;
            }
            assert!(started.elapsed() < Duration::from_secs(10), "kill didn't end the task");
            thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(exit, Exit::Killed);
        assert!(out.lines.iter().all(|l| plain(l) != "done"));
    }
}
