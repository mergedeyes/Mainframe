//! Opening, editing and running things, in the background or in the full terminal.

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
};

use super::{App, Foreground, Level};
use crate::{
    browser,
    cmd::{Cmd, Step},
    config::{self, Action, Config},
    locate, platform, proc,
    project::{self, Op, Plan},
};

impl App {
    pub(super) fn enter_dir(&mut self) {
        if let Some(e) = self.browser.selected().filter(|e| e.is_dir) {
            let path = e.path.clone();
            self.browser.cd(path);
            self.invalidate();
        }
    }

    pub(super) fn open_selected(&mut self) {
        let Some(e) = self.browser.selected().cloned() else { return };
        if e.is_dir {
            self.enter_dir();
        } else if !browser::is_binary(&e.path) {
            self.edit_selected();
        } else if e.is_exec {
            self.run_op(Op::Run, false);
        } else if let Err(e) = platform::open(&e.path) {
            let msg = if e.kind() == std::io::ErrorKind::NotFound { "binary file".into() } else { e.to_string() };
            self.set_status(msg, Level::Info);
        }
    }

    pub(super) fn edit_selected(&mut self) {
        let Some(e) = self.browser.selected().cloned() else { return };
        if e.is_dir {
            self.set_status(format!("{} is a directory — o opens it in VS Code", e.name), Level::Info);
            return;
        }
        if browser::is_binary(&e.path) {
            self.set_status(format!("{} is a binary file", e.name), Level::Err);
            return;
        }
        let path = e.path.clone();
        self.edit_path(&path, "");
    }

    /// Opens `path` in the editor; a missing file is created with `initial` first.
    /// False when it couldn't be.
    pub(super) fn edit_path(&mut self, path: &Path, initial: &str) -> bool {
        if !path.exists() {
            let created = path.parent().map_or(Ok(()), fs::create_dir_all).and_then(|_| fs::write(path, initial));
            if let Err(e) = created {
                self.set_status(format!("can't create {}: {e}", config::pretty_path(path)), Level::Err);
                return false;
            }
        }
        let Some(editor) = Step::from_setting(&self.config.editor()) else {
            self.set_status("no editor set — editor = … in the config", Level::Err);
            return false;
        };
        self.in_terminal(editor.path(path).into());
        true
    }

    /// Hands the terminal to `cmd` and comes back as soon as it exits.
    pub(super) fn in_terminal(&mut self, cmd: Cmd) {
        self.pending = Some(Foreground {
            cmd,
            cwd: self.browser.cwd.clone(),
            wait: false,
            then_reload_config: false,
            cd_to_output: false,
        });
    }

    /// Opens the location picked with n / N in the editor, or in VS Code.
    pub(super) fn open_loc(&mut self, vscode: bool) -> bool {
        let Some((_, loc)) = self.tab().output.selected_loc().cloned() else { return false };
        if vscode {
            self.spawn_detached(locate::editor_cmd(&self.config.vscode, &loc));
            self.set_status(format!("opened {}:{} in VS Code", config::pretty_path(&loc.path), loc.line), Level::Ok);
        } else {
            self.in_terminal(locate::editor_cmd(&self.config.editor(), &loc).into());
        }
        true
    }

    pub(super) fn edit_config(&mut self) {
        if self.edit_path(&Config::path(), config::TEMPLATE)
            && let Some(fg) = &mut self.pending
        {
            fg.then_reload_config = true;
        }
    }

    pub(super) fn open_vscode(&mut self, folder: bool) {
        let target = if folder {
            self.project.as_ref().map_or_else(|| self.browser.cwd.clone(), |p| p.root.clone())
        } else {
            match self.browser.selected() {
                Some(e) => e.path.clone(),
                None => return,
            }
        };
        self.spawn_detached(vscode(&self.config.vscode).path(&target));
        self.set_status(format!("opened {} in VS Code", config::pretty_path(&target)), Level::Ok);
    }

    pub(super) fn open_shell(&mut self) {
        self.in_terminal(self.config.shell().interactive().into());
    }

    pub(super) fn run_op(&mut self, op: Op, interactive: bool) {
        match project::plan(op, self.browser.selected(), &self.browser.cwd, &self.config) {
            Ok(plan) if interactive => self.foreground(plan, true),
            Ok(plan) => self.start_task(plan.cmd.display(&self.config.shell()), plan),
            Err(msg) => self.set_status(msg, Level::Err),
        }
    }

    pub(super) fn foreground(&mut self, plan: Plan, wait: bool) {
        self.pending =
            Some(Foreground { cmd: plan.cmd, cwd: plan.cwd, wait, then_reload_config: false, cd_to_output: false });
    }

    pub(super) fn custom_action(&mut self, action: &Action) {
        let sel = self.browser.selected();
        let cwd = &self.browser.cwd;
        let file = sel.map_or_else(|| cwd.clone(), |e| e.path.clone());
        let name = sel.map_or_else(String::new, |e| e.name.clone());
        let root = self.project.as_ref().map_or_else(|| cwd.clone(), |p| p.root.clone());
        let shell = self.config.shell();
        let line = action
            .cmd
            .replace("{file}", &shell.quote_path(&file))
            .replace("{name}", &shell.quote(&name))
            .replace("{dir}", &shell.quote_path(cwd))
            .replace("{root}", &shell.quote_path(&root));
        let plan = Plan { cmd: Cmd::Shell(line), cwd: cwd.clone() };
        if action.interactive {
            self.foreground(plan, true);
        } else {
            self.start_task(action.name.clone(), plan);
        }
    }

    /// Starts a program (VS Code, a notification) on its own and doesn't wait for it.
    pub(super) fn spawn_detached(&mut self, step: Step) {
        let mut command = step.command();
        command.current_dir(&self.browser.cwd).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        platform::detach(&mut command);
        let spawned = command.spawn();
        match spawned {
            // Reap it in the background so it doesn't linger as a zombie.
            Ok(mut child) => drop(thread::spawn(move || child.wait())),
            Err(e) => self.set_status(format!("failed: {e}"), Level::Err),
        }
    }

    /// Runs after a foreground command returns.
    pub fn after_foreground(&mut self, fg: &Foreground, output: Option<String>) {
        if fg.cd_to_output {
            let dir = output.map(|o| PathBuf::from(o.trim())).filter(|d| d.is_dir());
            if let Some(dir) = dir {
                self.browser.cd(dir);
            }
        }
        // Whatever ran may have been an editor on the project's settings.
        self.load_settings(true);
        if fg.then_reload_config {
            let (cfg, err) = Config::load();
            self.config = cfg;
            match err {
                Some(e) => self.set_status(e, Level::Err),
                None => self.set_status("config reloaded", Level::Ok),
            }
        }
        self.browser.reload();
        self.invalidate();
        if std::mem::take(&mut self.reopen_deploy) {
            self.open_deploy(None);
        }
        if let Some(path) = self.reopen_blog.take() {
            self.open_blog_picker(Some(&path));
        }
        if let Some((root, title)) = self.reopen_notes.take() {
            self.open_notes(root, title);
            self.refresh_todo_count();
        }
    }

    // ── zoxide ─────────────────────────────────────────────────────────────

    /// Best zoxide match for space-separated keywords, skipping where we already are.
    pub(super) fn zoxide_query(&self, input: &str) -> Option<PathBuf> {
        if !self.zoxide {
            return None;
        }
        let mut cmd = Command::new("zoxide");
        cmd.args(["query", "--exclude"]).arg(&self.browser.cwd).arg("--").args(input.split_whitespace());
        let dir = PathBuf::from(proc::text(&mut cmd)?);
        dir.is_dir().then_some(dir)
    }

    pub(super) fn zoxide_pick(&mut self) {
        if !self.zoxide {
            self.set_status("zoxide isn't installed (or is disabled in the config)", Level::Err);
            return;
        }
        self.pending = Some(Foreground {
            cmd: Cmd::words("zoxide query --interactive"),
            cwd: self.browser.cwd.clone(),
            wait: false,
            then_reload_config: false,
            cd_to_output: true,
        });
    }
}

/// The `vscode` setting as a program and its arguments; `code` if it's empty.
pub(super) fn vscode(setting: &str) -> Step {
    Step::from_setting(setting).unwrap_or_else(|| Step::new("code"))
}
