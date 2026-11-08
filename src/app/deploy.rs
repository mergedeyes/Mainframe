//! `D`: the current project's deploy targets. Builds, copies the result to another machine
//! over ssh and swaps it in for the binary there, keeping the old one as a backup.

use std::{
    fs,
    path::{Path, PathBuf},
};

use ratatui::{
    crossterm::event::{KeyCode, KeyEvent},
    widgets::ListState,
};

use super::{App, Confirmed, Foreground, Level, Mode};
use crate::{
    config,
    form::{Field, Form, FormKind},
    install,
    project::{self, Kind, Op, Plan, quote},
    settings::{self, Deploy, ProjectSettings},
};

pub struct DeployPicker {
    pub root: PathBuf,
    pub title: String,
    pub targets: Vec<Deploy>,
    pub state: ListState,
}

impl DeployPicker {
    pub fn selected(&self) -> Option<&Deploy> {
        self.targets.get(self.state.selected()?)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Run {
    Deploy,
    Rollback,
    Backups,
}

impl App {
    /// Reads the project's settings again when the project changed (or `force`).
    pub(super) fn load_settings(&mut self, force: bool) {
        if !force && self.settings_root == self.notes_root {
            return;
        }
        self.settings_root = self.notes_root.clone();
        let (settings, err) = ProjectSettings::load(&self.settings_root);
        self.settings = settings;
        if let Some(e) = err {
            self.set_status(e, Level::Err);
        }
    }

    fn project_title(root: &Path) -> String {
        root.file_name().map_or_else(|| config::pretty_path(root), |n| n.to_string_lossy().into_owned())
    }

    /// The picker, or the form for a first target when there's none yet.
    pub(super) fn open_deploy(&mut self, select: Option<&str>) {
        self.load_settings(true);
        let root = self.settings_root.clone();
        if self.settings.deploy.is_empty() {
            return self.new_deploy_form();
        }
        let targets = self.settings.deploy.clone();
        let pos = select.and_then(|n| targets.iter().position(|d| d.name == n)).unwrap_or(0);
        self.mode = Mode::Deploy(DeployPicker {
            title: Self::project_title(&root),
            root,
            targets,
            state: ListState::default().with_selected(Some(pos)),
        });
    }

    pub(super) fn key_deploy(&mut self, key: KeyEvent) {
        let Mode::Deploy(picker) = &mut self.mode else { return };
        let len = picker.targets.len();
        let cur = picker.state.selected().unwrap_or(0);
        let root = picker.root.clone();
        let target = picker.selected().cloned();
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.mode = Mode::Normal,
            KeyCode::Char('j') | KeyCode::Down => picker.state.select(Some((cur + 1).min(len.saturating_sub(1)))),
            KeyCode::Char('k') | KeyCode::Up => picker.state.select(Some(cur.saturating_sub(1))),
            KeyCode::Char('a') => self.new_deploy_form(),
            KeyCode::Char('e') => self.edit_settings(&root),
            KeyCode::Enter | KeyCode::Char('D') => {
                if let Some(d) = target {
                    self.mode = Mode::Normal;
                    self.run_deploy(&d, &root, Run::Deploy);
                }
            }
            KeyCode::Char('l') => {
                if let Some(d) = target {
                    self.mode = Mode::Normal;
                    self.run_deploy(&d, &root, Run::Backups);
                }
            }
            KeyCode::Char('u') => {
                if let Some(d) = target {
                    self.mode = Mode::Confirm {
                        msg: format!("Put the newest backup of {} back on {}? [y/N]", d.remote_path, d.host),
                        action: Confirmed::Rollback(Box::new(d), root),
                    };
                }
            }
            _ => {}
        }
    }

    /// Opens the project's settings file in the editor, with the template if it's new,
    /// and comes back to the picker afterwards.
    pub(super) fn edit_settings(&mut self, root: &Path) {
        match settings::ensure_file(root) {
            Ok(path) => {
                self.mode = Mode::Normal;
                self.reopen_deploy = true;
                self.pending = Some(Foreground {
                    cmd: format!("{} {}", self.config.editor, quote(&path)),
                    cwd: self.browser.cwd.clone(),
                    wait: false,
                    then_reload_config: false,
                    cd_to_output: false,
                });
            }
            Err(e) => self.set_status(format!("can't create {}: {e}", config::pretty_path(&settings::dir())), Level::Err),
        }
    }

    /// Writes the script and runs it: in the output pane, or in the real terminal when the
    /// target asks for passwords.
    pub(super) fn run_deploy(&mut self, d: &Deploy, root: &Path, what: Run) {
        if let Some(p) = d.problem() {
            return self.set_status(format!("deploy {}: {p} — D then e fixes it", d.name), Level::Err);
        }
        let (script, tag, label) = match what {
            Run::Deploy => (d.script(root), "deploy", format!("deploy {}", d.name)),
            Run::Rollback => (d.rollback_script(), "rollback", format!("roll back {}", d.name)),
            Run::Backups => (d.backups_script(), "backups", format!("backups on {}", d.name)),
        };
        let file = settings::script_file(root, &d.name, tag);
        let written = file.parent().map_or(Ok(()), fs::create_dir_all).and_then(|_| fs::write(&file, script));
        if let Err(e) = written {
            return self.set_status(format!("can't write {}: {e}", config::pretty_path(&file)), Level::Err);
        }
        let plan = Plan { cmd: format!("sh {}", quote(&file)), cwd: root.to_path_buf() };
        if d.interactive {
            self.foreground(plan, true);
        } else {
            self.start_task(label, plan);
        }
    }

    pub(super) fn new_deploy_form(&mut self) {
        let root = self.settings_root.clone();
        let project = self.project.clone().filter(|p| p.root.starts_with(&root)).or_else(|| project::detect(&root));
        let guess = guess(&root, project.as_ref(), &self.config.install_dir());
        let taken: Vec<String> = self.settings.deploy.iter().map(|d| d.name.clone()).collect();
        let name = std::iter::once("pi".to_string())
            .chain((2..).map(|i| format!("pi{i}")))
            .find(|n| !taken.contains(n))
            .unwrap_or_default();
        let has_key = std::env::var_os("SSH_AUTH_SOCK").is_some()
            || fs::read_dir(config::home().join(".ssh"))
                .is_ok_and(|d| d.flatten().any(|e| e.file_name().to_string_lossy().starts_with("id_")));
        let note = (!has_key).then(|| "no ssh key found — ssh-keygen + ssh-copy-id <host>, or run it in the terminal".to_string());
        self.mode = Mode::Form(Form {
            kind: FormKind::Deploy { root: root.clone(), taken },
            title: format!(" new deploy target · {} ", Self::project_title(&root)),
            fields: vec![
                Field::text("name", name, "what the picker calls it"),
                Field::text("host", "pi@raspberrypi.local", "user@host or an alias from ~/.ssh/config"),
                Field::text("remote path", format!("/usr/local/bin/{}", guess.bin), "the file there that gets replaced"),
                Field::text("build", guess.build, "runs in the project root · - for none"),
                Field::text("artifact", guess.artifact, "the built file, relative to the project root"),
                Field::choice("sudo", &["yes", "no"], 0, "replace it as root (needed for /usr/…)"),
                Field::text("restart", "", "optional, runs there afterwards, e.g. sudo systemctl restart …"),
                Field::choice("run", &["background", "terminal"], 0, "terminal: when ssh or sudo ask for a password"),
            ],
            step: 0,
            note,
            completer: None,
        });
    }

    /// Saves the form's target and shows it in the picker. False keeps the form open.
    pub(super) fn submit_deploy_form(&mut self, form: &Form) -> bool {
        let FormKind::Deploy { root, taken } = &form.kind else { return true };
        let d = deploy_from_form(form);
        let problem = d.problem().or_else(|| taken.contains(&d.name).then(|| format!("there's a target named {} already", d.name)));
        if let Some(p) = problem {
            self.set_status(p, Level::Err);
            return false;
        }
        match settings::append_deploy(root, &d) {
            Ok(path) => {
                self.open_deploy(Some(&d.name));
                self.set_status(format!("saved to {} — Enter deploys, e edits", config::pretty_path(&path)), Level::Ok);
            }
            Err(e) => self.set_status(format!("can't save: {e}"), Level::Err),
        }
        true
    }
}

/// The target the deploy form currently describes.
pub fn deploy_from_form(form: &Form) -> Deploy {
    let build = form.get("build").trim();
    Deploy {
        name: form.get("name").trim().to_string(),
        host: form.get("host").trim().to_string(),
        remote_path: form.get("remote path").trim().to_string(),
        build: if build == "-" { String::new() } else { build.to_string() },
        artifact: form.get("artifact").trim().to_string(),
        sudo: form.get("sudo") == "yes",
        restart: form.get("restart").trim().to_string(),
        interactive: form.get("run") == "terminal",
        ..Deploy::default()
    }
}

struct Guess {
    build: String,
    artifact: String,
    bin: String,
}

/// A Pi runs 64-bit ARM these days; an ARM build that's already there wins.
const PI_TRIPLE: &str = "aarch64-unknown-linux-gnu";

fn guess(root: &Path, project: Option<&project::Project>, install_dir: &Path) -> Guess {
    let dir_name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "app".into());
    let cargo = if project::which("cross").is_some() { "cross" } else { "cargo" };
    let kind = project.map(|p| p.kind);
    let name = project.map_or(dir_name.clone(), |p| p.name.clone());

    if kind == Some(Kind::Cargo) {
        let arm = install::scan(root, install_dir).into_iter().find(|c| {
            c.triple.as_deref().is_some_and(|t| t.starts_with("aarch64") || t.starts_with("arm"))
        });
        if let Some(c) = arm {
            let triple = c.triple.clone().unwrap_or_default();
            let release = if c.kind.starts_with("release") { " --release" } else { "" };
            let example = if c.kind.contains("example") { format!(" --example {}", c.name) } else { String::new() };
            return Guess { build: format!("{cargo} build{release} --target {triple}{example}"), artifact: c.rel, bin: c.name };
        }
        return Guess {
            build: format!("{cargo} build --release --target {PI_TRIPLE}"),
            artifact: format!("target/{PI_TRIPLE}/release/{name}"),
            bin: name,
        };
    }
    if kind == Some(Kind::Go) {
        let out = format!("build/{dir_name}-linux-arm64");
        return Guess { build: format!("GOOS=linux GOARCH=arm64 go build -o {out} ."), artifact: out, bin: dir_name };
    }
    let build = project.and_then(|p| project::plan(Op::Release, None, &p.root, &Default::default()).ok()).map(|p| p.cmd);
    Guess { build: build.unwrap_or_default(), artifact: String::new(), bin: dir_name }
}
