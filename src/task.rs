//! Background commands whose output streams into the output pane.

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        mpsc::{self, Receiver, TryRecvError},
    },
    thread,
    time::{Duration, Instant},
};

use ratatui::text::Line;

pub use crate::run::Exit;
use crate::{
    ansi,
    cmd::Cmd,
    locate::{self, Loc},
    run::{self, Control, Ctx, Msg, Sink},
    shell::Shell,
};

const MAX_LINES: usize = 20_000;

pub struct Task {
    pub label: String,
    rx: Receiver<Msg>,
    control: Arc<Control>,
    pub started: Instant,
    finished: bool,
}

impl Task {
    pub fn spawn(label: String, cmd: &Cmd, cwd: &Path, shell: &Shell) -> std::io::Result<Self> {
        let (tx, rx) = mpsc::channel();
        let control = Arc::new(Control::default());
        let (ctl, cmd, cwd, shell) = (control.clone(), cmd.clone(), cwd.to_path_buf(), shell.clone());
        thread::Builder::new().name(format!("task {label}")).spawn(move || {
            let mut ctx = Ctx::new(&cwd, shell, Sink::Pane(tx.clone()), &ctl);
            let exit = run::run(&cmd, &mut ctx);
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
        self.control.stop(false);
    }

    pub fn kill(&mut self) {
        self.control.stop(true);
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
            Step::words("git --version").into(),
            Step::words("git no-such-command").into(),
            Step::words("git --version").into(),
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
        assert!(lines.iter().any(|l| l.contains("couldn't start mainframe-no-such-program")), "{lines:?}");
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
