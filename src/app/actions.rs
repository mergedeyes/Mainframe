//! Opening, editing and running things, in the background or in the full terminal.

use std::{
    env, fs,
    os::unix::process::CommandExt,
    path::PathBuf,
    process::{Command, Stdio},
    thread,
};

use super::{App, Foreground, Level};
use crate::{
    browser,
    config::{self, Action, Config},
    project::{self, Op, Plan, quote, quote_str},
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
        } else if project::which("xdg-open").is_some() {
            self.spawn_detached(&format!("xdg-open {}", quote(&e.path)));
        } else {
            self.set_status("binary file", Level::Info);
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
        self.pending = Some(Foreground {
            cmd: format!("{} {}", self.config.editor, quote(&e.path)),
            cwd: self.browser.cwd.clone(),
            wait: false,
            then_reload_config: false,
            cd_to_output: false,
        });
    }

    pub(super) fn edit_config(&mut self) {
        let path = Config::path();
        if !path.exists() {
            let created = path
                .parent()
                .map_or(Ok(()), fs::create_dir_all)
                .and_then(|_| fs::write(&path, config::TEMPLATE));
            if let Err(e) = created {
                self.set_status(format!("can't create {}: {e}", path.display()), Level::Err);
                return;
            }
        }
        self.pending = Some(Foreground {
            cmd: format!("{} {}", self.config.editor, quote(&path)),
            cwd: self.browser.cwd.clone(),
            wait: false,
            then_reload_config: true,
            cd_to_output: false,
        });
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
        self.spawn_detached(&format!("{} {}", self.config.vscode, quote(&target)));
        self.set_status(format!("opened {} in VS Code", config::pretty_path(&target)), Level::Ok);
    }

    pub(super) fn open_shell(&mut self) {
        let shell = env::var("SHELL").unwrap_or_else(|_| "sh".into());
        self.pending = Some(Foreground {
            cmd: quote_str(&shell),
            cwd: self.browser.cwd.clone(),
            wait: false,
            then_reload_config: false,
            cd_to_output: false,
        });
    }

    pub(super) fn run_op(&mut self, op: Op, interactive: bool) {
        match project::plan(op, self.browser.selected(), &self.browser.cwd, &self.config) {
            Ok(plan) if interactive => self.foreground(plan, true),
            Ok(plan) => self.start_task(plan.cmd.clone(), plan),
            Err(msg) => self.set_status(msg, Level::Err),
        }
    }

    pub(super) fn foreground(&mut self, plan: Plan, wait: bool) {
        self.pending = Some(Foreground { cmd: plan.cmd, cwd: plan.cwd, wait, then_reload_config: false, cd_to_output: false });
    }

    pub(super) fn custom_action(&mut self, action: &Action) {
        let sel = self.browser.selected();
        let cwd = &self.browser.cwd;
        let file = sel.map_or_else(|| cwd.clone(), |e| e.path.clone());
        let name = sel.map_or_else(String::new, |e| e.name.clone());
        let root = self.project.as_ref().map_or_else(|| cwd.clone(), |p| p.root.clone());
        let cmd = action
            .cmd
            .replace("{file}", &quote(&file))
            .replace("{name}", &quote_str(&name))
            .replace("{dir}", &quote(cwd))
            .replace("{root}", &quote(&root));
        let plan = Plan { cmd, cwd: cwd.clone() };
        if action.interactive {
            self.foreground(plan, true);
        } else {
            self.start_task(action.name.clone(), plan);
        }
    }

    pub(super) fn spawn_detached(&mut self, cmd: &str) {
        let spawned = Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .current_dir(&self.browser.cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn();
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
    }

    // ── zoxide ─────────────────────────────────────────────────────────────

    /// Best zoxide match for space-separated keywords, skipping where we already are.
    pub(super) fn zoxide_query(&self, input: &str) -> Option<PathBuf> {
        if !self.zoxide {
            return None;
        }
        let out = Command::new("zoxide")
            .args(["query", "--exclude"])
            .arg(&self.browser.cwd)
            .arg("--")
            .args(input.split_whitespace())
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()
            .filter(|o| o.status.success())?;
        let dir = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
        dir.is_dir().then_some(dir)
    }

    pub(super) fn zoxide_pick(&mut self) {
        if !self.zoxide {
            self.set_status("zoxide isn't installed (or is disabled in the config)", Level::Err);
            return;
        }
        self.pending = Some(Foreground {
            cmd: "zoxide query --interactive".into(),
            cwd: self.browser.cwd.clone(),
            wait: false,
            then_reload_config: false,
            cd_to_output: true,
        });
    }
}
