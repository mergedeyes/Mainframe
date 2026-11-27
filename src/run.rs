//! Carries out a [`Cmd`]: programs and built-in steps one after another, into the output
//! pane (a background task) or the real terminal (an interactive run). Built-in steps get a
//! [`Ctx`] to print, run programs and read their output, the same way in both.

use std::{
    io::{self, BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::Sender,
    },
    thread,
    time::{Duration, Instant},
};

use crate::{
    cmd::{Builtin, Cmd, Job, Step},
    platform::{self, ProcessTree},
    shell::Shell,
};

/// How a run ended.
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

pub enum Msg {
    Line(String),
    Done(Exit),
}

/// Shared between a task and the thread running it, so a stop reaches whichever program runs.
#[derive(Default)]
pub struct Control {
    stopped: AtomicBool,
    running: Mutex<Option<ProcessTree>>,
}

impl Control {
    /// Stops the running program and everything it started, and whatever would come after.
    pub fn stop(&self, hard: bool) {
        self.stopped.store(true, Ordering::SeqCst);
        if let Some(tree) = &*self.running.lock().unwrap_or_else(|e| e.into_inner()) {
            if hard { tree.kill() } else { tree.terminate() }
        }
    }

    pub fn stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }
}

/// Where output goes.
pub enum Sink {
    /// The output pane, line by line; programs get no terminal.
    Pane(Sender<Msg>),
    /// The real terminal: programs use it directly. With `capture`, their stdout is kept
    /// instead (pickers like fzf draw on the terminal itself, so that doesn't disturb them).
    Terminal { capture: Option<String> },
}

/// What a built-in step works with.
pub struct Ctx<'a> {
    pub cwd: PathBuf,
    pub shell: Shell,
    sink: Sink,
    control: &'a Control,
}

const DIM: &str = "\x1b[2m";
const RESET: &str = "\x1b[0m";

impl<'a> Ctx<'a> {
    pub fn new(cwd: &Path, shell: Shell, sink: Sink, control: &'a Control) -> Self {
        Self { cwd: cwd.to_path_buf(), shell, sink, control }
    }

    /// What a terminal run captured, when it was asked to.
    pub fn captured(self) -> Option<String> {
        match self.sink {
            Sink::Terminal { capture } => capture,
            Sink::Pane(_) => None,
        }
    }

    pub fn say(&mut self, line: impl AsRef<str>) {
        match &self.sink {
            Sink::Pane(tx) => drop(tx.send(Msg::Line(line.as_ref().to_string()))),
            Sink::Terminal { .. } => println!("{}", line.as_ref()),
        }
    }

    /// A heading between the parts of a longer run: `── copy 2.1M → pi`.
    pub fn heading(&mut self, text: impl AsRef<str>) {
        self.say(format!("\x1b[1;36m── {}{RESET}", text.as_ref()));
    }

    pub fn success(&mut self, text: impl AsRef<str>) {
        self.say(format!("\x1b[1;32m✔ {}{RESET}", text.as_ref()));
    }

    pub fn failure(&mut self, text: impl AsRef<str>) {
        self.say(format!("\x1b[1;31m✘ {}{RESET}", text.as_ref()));
    }

    pub fn stopped(&self) -> bool {
        self.control.stopped()
    }

    /// Waits `d`, or until the task is stopped. False when it was.
    pub fn sleep(&self, d: Duration) -> bool {
        let end = Instant::now() + d;
        while Instant::now() < end {
            if self.stopped() {
                return false;
            }
            thread::sleep(Duration::from_millis(100).min(end - Instant::now()));
        }
        !self.stopped()
    }

    /// Runs `step`, shown as `$ …` first.
    pub fn run(&mut self, step: &Step) -> Exit {
        let display = step.display(&self.shell);
        self.say(format!("{DIM}$ {display}{RESET}"));
        let mut command = step.command();
        command.current_dir(step.dir.as_deref().unwrap_or(&self.cwd));
        self.spawn(command, &step.program)
    }

    /// `run`, with a failure as the error to report.
    pub fn must(&mut self, step: &Step, failed: impl FnOnce() -> String) -> Result<(), String> {
        match self.run(step) {
            Exit::Killed => Err("stopped".into()),
            e if e.ok() => Ok(()),
            _ => Err(failed()),
        }
    }

    /// stdout of `step`, trimmed, when it succeeds. Nothing is shown: it's a question, not a step.
    pub fn ask(&self, step: &Step) -> Option<String> {
        let mut c = step.command();
        c.current_dir(step.dir.as_deref().unwrap_or(&self.cwd)).stdin(Stdio::null()).stderr(Stdio::null());
        hide_console(&mut c);
        let out = c.output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    /// Starts `command` (its folder already set) and waits for it, its output going to the sink.
    pub fn spawn(&mut self, mut command: Command, program: &str) -> Exit {
        if self.stopped() {
            return Exit::Killed;
        }
        let tx = match &mut self.sink {
            Sink::Terminal { capture } => {
                let status = match capture {
                    Some(c) => command.stderr(Stdio::inherit()).output().map(|o| {
                        c.push_str(&String::from_utf8_lossy(&o.stdout));
                        o.status
                    }),
                    None => command.status(),
                };
                return match status {
                    Ok(s) => exit_of(s),
                    Err(e) => {
                        let _ = writeln!(io::stderr(), "couldn't start {program}: {e}");
                        Exit::NotStarted
                    }
                };
            }
            Sink::Pane(tx) => tx.clone(),
        };
        command
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
                self.failure(format!("couldn't start {program}: {e}"));
                return Exit::NotStarted;
            }
        };
        {
            let mut running = self.control.running.lock().unwrap_or_else(|e| e.into_inner());
            let tree = ProcessTree::attach(&child);
            // A stop that came while it was starting.
            if self.stopped() {
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
        self.control.running.lock().unwrap_or_else(|e| e.into_inner()).take();
        match status {
            Ok(s) if s.success() => Exit::Code(0),
            _ if self.stopped() => Exit::Killed,
            Ok(s) => exit_of(s),
            Err(_) => Exit::Killed,
        }
    }
}

fn exit_of(status: std::process::ExitStatus) -> Exit {
    status.code().map_or(Exit::Killed, Exit::Code)
}

/// Questions asked from a background thread shouldn't flash a console window on Windows.
fn hide_console(c: &mut Command) {
    platform::detach(c);
}

/// Runs `cmd` to the end. The first step's command is already on screen (the task's
/// headline), so only later ones get a `$ …` line.
pub fn run(cmd: &Cmd, ctx: &mut Ctx) -> Exit {
    match cmd {
        Cmd::Shell(line) => {
            let command = in_dir(ctx.shell.command(line), &ctx.cwd);
            let name = ctx.shell.program.to_string_lossy().into_owned();
            ctx.spawn(command, &name)
        }
        Cmd::Sh(line) => {
            let Some(sh) = platform::posix_sh() else {
                ctx.failure("this still needs sh — install Git for Windows");
                return Exit::NotStarted;
            };
            let mut command = Command::new(sh);
            command.arg("-c").arg(line);
            let command = in_dir(command, &ctx.cwd);
            ctx.spawn(command, "sh")
        }
        Cmd::Steps(jobs) => {
            let mut first = true;
            for job in jobs {
                if ctx.stopped() {
                    return Exit::Killed;
                }
                match job {
                    Job::Run(step) => {
                        let cwd = step.dir.clone().unwrap_or_else(|| ctx.cwd.clone());
                        if step.unless_exists.as_ref().is_some_and(|p| cwd.join(p).exists()) {
                            continue;
                        }
                        let exit = if first {
                            let mut command = step.command();
                            command.current_dir(&cwd);
                            ctx.spawn(command, &step.program)
                        } else {
                            ctx.run(step)
                        };
                        first = false;
                        if !exit.ok() && !(step.may_fail && exit != Exit::Killed) {
                            return exit;
                        }
                    }
                    Job::Do(b) => {
                        first = false;
                        if let Err(msg) = builtin(b, ctx) {
                            if ctx.stopped() {
                                return Exit::Killed;
                            }
                            ctx.failure(msg);
                            return Exit::Code(1);
                        }
                    }
                }
            }
            Exit::Code(0)
        }
    }
}

fn in_dir(mut c: Command, dir: &Path) -> Command {
    c.current_dir(dir);
    c
}

pub fn builtin(b: &Builtin, ctx: &mut Ctx) -> Result<(), String> {
    match b {
        Builtin::Say(text) => {
            ctx.say(text);
            Ok(())
        }
        Builtin::Sleep(s) => {
            if ctx.sleep(Duration::from_secs(*s)) {
                Ok(())
            } else {
                Err("stopped".into())
            }
        }
        Builtin::PruneGoneBranches { base } => crate::git::prune_gone_branches(ctx, base),
        Builtin::RunShell(line) => {
            ctx.say(format!("{DIM}$ {line}{RESET}"));
            let command = in_dir(ctx.shell.command(line), &ctx.cwd.clone());
            let name = ctx.shell.program.to_string_lossy().into_owned();
            match ctx.spawn(command, &name) {
                e if e.ok() => Ok(()),
                Exit::Killed => Err("stopped".into()),
                _ => Err(format!("{line} failed")),
            }
        }
        Builtin::SetVersion { file, version } => {
            let path = ctx.cwd.join(file);
            let text = std::fs::read_to_string(&path).map_err(|e| format!("can't read {file}: {e}"))?;
            let new = crate::release::set_version_line(&text, version)
                .ok_or_else(|| format!("no version = \"…\" line in {file}"))?;
            std::fs::write(&path, new).map_err(|e| format!("can't write {file}: {e}"))?;
            ctx.say(format!("version = \"{version}\" in {file}"));
            Ok(())
        }
        Builtin::RequireVersion { file, version, why } => {
            let text = std::fs::read_to_string(ctx.cwd.join(file)).map_err(|e| format!("can't read {file}: {e}"))?;
            if crate::release::manifest_has_version(file, &text, version) { Ok(()) } else { Err(why.clone()) }
        }
        Builtin::DeleteBranch { remote, branch } => {
            // Quietly: either may be gone already.
            if ctx.ask(&Step::new("git").args(["branch", "-d", branch])).is_some() {
                ctx.say(format!("deleted branch {branch}"));
            }
            if ctx.ask(&Step::new("git").args(["push", remote, "--delete", branch])).is_some() {
                ctx.say(format!("deleted {branch} on {remote}"));
            }
            Ok(())
        }
        Builtin::FollowPages { repo, live } => crate::blog::follow_pages(ctx, repo, live),
        Builtin::InstallFile { src, dest } => crate::install::install_file(ctx, src, dest),
        Builtin::Delete(paths) => crate::cleanup::delete(ctx, paths),
        Builtin::Cleanup { jobs, what, expected } => crate::cleanup::clean(ctx, jobs, what, *expected),
        Builtin::RemoveFile(p) => {
            let pretty = crate::config::pretty_path(p);
            std::fs::remove_file(p).map_err(|e| format!("can't remove {pretty}: {e}"))?;
            ctx.say(format!("removed {pretty}"));
            Ok(())
        }
        Builtin::WatchRelease { repo, workflow, tag } => {
            crate::release::watch_release(ctx, repo.as_deref(), workflow, tag)
        }
    }
}

#[cfg(test)]
pub mod testing {
    use std::sync::mpsc;

    use super::*;

    /// Runs `cmd` in `cwd` to the end: whether it succeeded, and its output as plain text.
    pub fn run_in(cmd: &Cmd, cwd: &Path) -> (bool, String) {
        let control = Control::default();
        let (tx, rx) = mpsc::channel();
        let mut ctx = Ctx::new(cwd, Shell::default(), Sink::Pane(tx), &control);
        let exit = run(cmd, &mut ctx);
        drop(ctx);
        let mut out = String::new();
        for msg in rx {
            if let Msg::Line(l) = msg {
                out.extend(crate::ansi::parse_line(&l).spans.iter().map(|s| s.content.to_string()));
                out.push('\n');
            }
        }
        (exit.ok(), out)
    }
}
