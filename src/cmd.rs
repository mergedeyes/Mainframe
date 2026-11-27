//! What a task runs. Mainframe's own commands are programs started directly with their
//! arguments, and the logic between them is Rust ([`Builtin`]), so no shell's quoting or
//! syntax gets in the way on any system; only what the user writes goes through their shell.
//! `run` carries them out.

use std::{
    path::{Path, PathBuf},
    process::Command,
};

use serde::{Deserialize, Serialize};

use crate::{platform, shell::Shell};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Cmd {
    /// Run one after another, each only when the one before succeeded.
    Steps(Vec<Job>),
    /// A command line for the user's shell: typed with `:`, custom actions, runners.
    Shell(String),
    /// A POSIX sh script of Mainframe's own that isn't Rust yet. On Windows it runs in Git's sh.
    Sh(String),
}

/// One step of [`Cmd::Steps`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Job {
    /// A program.
    Run(Step),
    /// A step of Mainframe's own, in Rust.
    Do(Builtin),
}

impl From<Step> for Job {
    fn from(s: Step) -> Self {
        Job::Run(s)
    }
}

impl From<Builtin> for Job {
    fn from(b: Builtin) -> Self {
        Job::Do(b)
    }
}

impl Job {
    pub fn display(&self, shell: &Shell) -> String {
        match self {
            Job::Run(s) => s.display(shell),
            Job::Do(b) => b.describe(),
        }
    }
}

/// What used to be shell script between the programs: plain data, so a task can be
/// rerun and saved like any other. `run` has the code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Builtin {
    /// Prints a line.
    Say(String),
    /// Waits, unless the task is stopped.
    Sleep(u64),
    /// Deletes local branches whose remote branch is gone, when their work is in `base`.
    PruneGoneBranches { base: String },
}

impl Builtin {
    /// What it does, in the output pane's command line and in forms.
    pub fn describe(&self) -> String {
        match self {
            Builtin::Say(text) => format!("echo {text}"),
            Builtin::Sleep(s) => format!("wait {s}s"),
            Builtin::PruneGoneBranches { base } => {
                format!("delete branches whose remote branch is gone and whose work is in {base}")
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    pub program: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub env: Vec<(String, String)>,
    /// Skipped when this (relative to where the task runs) exists, like `meson setup build`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unless_exists: Option<PathBuf>,
    /// Runs here instead of where the task runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dir: Option<PathBuf>,
    /// A failure doesn't stop the steps after it (tidying up that may have nothing to do).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub may_fail: bool,
}

impl Step {
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            env: Vec::new(),
            unless_exists: None,
            dir: None,
            may_fail: false,
        }
    }

    /// A fixed command line, split at spaces: `Step::words("cargo build --release")`.
    pub fn words(line: &str) -> Self {
        let mut words = line.split_whitespace();
        Self::new(words.next().unwrap_or_default()).args(words)
    }

    /// From a setting like `editor = "code --wait"`: words, with "…" or '…' around one that
    /// has spaces (`"C:\Program Files\Notepad++\notepad++.exe"`). Backslashes are kept, and
    /// a leading `~` is the home folder, as a shell would have it.
    pub fn from_setting(s: &str) -> Option<Self> {
        let mut words = split_words(s).into_iter();
        let program = words.next()?;
        let program = if program.starts_with('~') {
            crate::config::expand_tilde(&program).to_string_lossy().into_owned()
        } else {
            program
        };
        Some(Self::new(program).args(words))
    }

    /// git with colors on, since the output pane isn't a terminal.
    pub fn git() -> Self {
        Self::new("git").args(["-c", "color.ui=always"])
    }

    pub fn arg(mut self, a: impl Into<String>) -> Self {
        self.args.push(a.into());
        self
    }

    pub fn args<S: Into<String>>(mut self, args: impl IntoIterator<Item = S>) -> Self {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    pub fn path(self, p: &Path) -> Self {
        self.arg(p.to_string_lossy())
    }

    pub fn env(mut self, key: &str, value: impl Into<String>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    pub fn unless_exists(mut self, p: impl Into<PathBuf>) -> Self {
        self.unless_exists = Some(p.into());
        self
    }

    pub fn in_dir(mut self, dir: &Path) -> Self {
        self.dir = Some(dir.to_path_buf());
        self
    }

    pub fn may_fail(mut self) -> Self {
        self.may_fail = true;
        self
    }

    /// This, then `next` if it succeeded.
    pub fn then(self, next: impl Into<Job>) -> Cmd {
        Cmd::Steps(vec![self.into(), next.into()])
    }

    pub fn command(&self) -> Command {
        let mut c = Command::new(resolve(&self.program));
        c.args(&self.args);
        c.envs(self.env.iter().map(|(k, v)| (k, v)));
        c
    }

    /// The step as you'd type it into `shell`. The colors git is asked for are left out.
    pub fn display(&self, shell: &Shell) -> String {
        let mut args: &[String] = &self.args;
        if self.program == "git" && args.starts_with(&["-c".into(), "color.ui=always".into()]) {
            args = &args[2..];
        }
        let mut out: Vec<String> = self.env.iter().map(|(k, v)| format!("{k}={}", shell.quote(v))).collect();
        out.push(shell.quote(&self.program));
        out.extend(args.iter().map(|a| shell.quote(a)));
        let mut line = out.join(" ");
        if let Some(d) = &self.dir {
            line = format!("{line}   (in {})", crate::config::pretty_path(d));
        }
        match &self.unless_exists {
            Some(p) => format!("{line} (if there's no {})", p.display()),
            None => line,
        }
    }
}

impl From<Step> for Cmd {
    fn from(step: Step) -> Self {
        Cmd::Steps(vec![step.into()])
    }
}

impl From<Builtin> for Cmd {
    fn from(b: Builtin) -> Self {
        Cmd::Steps(vec![b.into()])
    }
}

impl Cmd {
    /// A fixed command line, split at spaces.
    pub fn words(line: &str) -> Self {
        Step::words(line).into()
    }

    /// This, then `step` if it succeeded.
    pub fn then(self, step: Step) -> Self {
        match self {
            Cmd::Steps(mut steps) => {
                steps.push(step.into());
                Cmd::Steps(steps)
            }
            Cmd::Shell(line) => Cmd::Shell(format!("{line} && {}", step.display(&Shell::default()))),
            Cmd::Sh(line) => Cmd::Sh(format!("{line} && {}", step.display(&Shell::sh()))),
        }
    }

    /// How the output pane shows it: as you'd type it into `shell`.
    pub fn display(&self, shell: &Shell) -> String {
        match self {
            Cmd::Steps(steps) => steps.iter().map(|s| s.display(shell)).collect::<Vec<_>>().join(" && "),
            Cmd::Shell(line) => line.clone(),
            Cmd::Sh(line) => line.replace(crate::git::GIT, "git"),
        }
    }
}

/// On Windows `Command` only finds `.exe` files on PATH by itself; `code`, `npm` and `npx`
/// are `.cmd` scripts. A full path also lets std run those safely.
pub fn resolve(program: &str) -> PathBuf {
    if cfg!(windows) && !program.contains(['/', '\\']) {
        platform::which(program).unwrap_or_else(|| program.into())
    } else {
        program.into()
    }
}

/// A program's name without folder or extension, lowercased: `C:\Tools\Code.exe` → `code`.
/// Splits at `/` and `\` on every system, since settings may be written for either.
pub fn program_stem(program: &str) -> String {
    let file = program.rsplit(['/', '\\']).next().unwrap_or(program);
    let stem = match file.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && ext.chars().all(|c| c.is_ascii_alphanumeric()) => stem,
        _ => file,
    };
    stem.to_ascii_lowercase()
}

/// Words separated by whitespace; "…" or '…' keep spaces in one. No escapes, so Windows
/// paths come through as written.
pub fn split_words(s: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut quote = None;
    for c in s.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => cur.push(c),
            None if c == '"' || c == '\'' => {
                quote = Some(c);
                in_word = true;
            }
            None if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            None => {
                cur.push(c);
                in_word = true;
            }
        }
    }
    if in_word {
        words.push(cur);
    }
    words
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::Kind;

    fn posix() -> Shell {
        Shell { program: "sh".into(), kind: Kind::Posix }
    }

    #[test]
    fn settings_split_into_words() {
        assert_eq!(split_words("code --wait"), ["code", "--wait"]);
        assert_eq!(
            split_words(r#""C:\Program Files\Notepad++\notepad++.exe" -multiInst"#),
            [r"C:\Program Files\Notepad++\notepad++.exe", "-multiInst"]
        );
        assert_eq!(split_words("  vim  '' x"), ["vim", "", "x"]);
        assert!(split_words("   ").is_empty());
        let home = crate::config::home();
        assert_eq!(Step::from_setting("~/bin/ed -w").unwrap().program, home.join("bin/ed").to_string_lossy());
    }

    #[test]
    fn display_is_what_youd_type() {
        let cmd = Cmd::from(Step::git().args(["commit", "-m", "it's done"])).then(Step::words("git status --short"));
        assert_eq!(cmd.display(&posix()), r"git commit -m 'it'\''s done' && git status --short");
        let meson = Step::words("meson setup build").unless_exists("build");
        assert_eq!(meson.display(&posix()), "meson setup build (if there's no build)");
        let ps = Shell { program: "pwsh".into(), kind: Kind::PowerShell };
        assert_eq!(Step::new("cargo").args(["run", "--", "a b"]).display(&ps), "cargo run -- 'a b'");
    }

    #[test]
    fn steps_survive_saving() {
        let cmd = Cmd::from(Step::words("cargo build").env("RUSTFLAGS", "-D warnings"));
        let json = serde_json::to_string(&cmd).unwrap();
        assert_eq!(serde_json::from_str::<Cmd>(&json).unwrap(), cmd);
    }
}
