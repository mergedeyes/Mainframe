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

use super::{App, Confirmed, Level, Mode};
use crate::{
    config, filter_list,
    form::{Field, Form, FormKind},
    install,
    line::LineEdit,
    project::{self, Kind, Op, Plan, quote},
    settings::{self, Deploy, ProjectSettings},
};

pub struct DeployPicker {
    pub root: PathBuf,
    pub title: String,
    pub targets: Vec<Deploy>,
    pub state: ListState,
    /// keep_deploy_builds from the config, for the summary.
    pub keep_build: bool,
    /// The settings file doesn't parse, so no targets show.
    pub broken: bool,
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
        // A file that doesn't parse reads as no targets; the picker (with E) beats a form
        // for a new one then, and the status line says what's wrong.
        let broken = self.settings.deploy.is_empty()
            && fs::read_to_string(settings::file_for(&root))
                .is_ok_and(|t| toml::from_str::<ProjectSettings>(&t).is_err());
        if self.settings.deploy.is_empty() && !broken {
            return self.new_deploy_form();
        }
        let targets = self.settings.deploy.clone();
        let pos = select.and_then(|n| targets.iter().position(|d| d.name == n)).unwrap_or(0);
        self.mode = Mode::Deploy(DeployPicker {
            title: Self::project_title(&root),
            root,
            targets,
            state: ListState::default().with_selected(Some(pos)),
            keep_build: self.config.keep_deploy_builds,
            broken,
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
            KeyCode::Char('j') | KeyCode::Down => filter_list::step(&mut picker.state, len, 1),
            KeyCode::Char('k') | KeyCode::Up => filter_list::step(&mut picker.state, len, -1),
            KeyCode::Char('a') if picker.broken => {
                self.set_status("the settings file doesn't parse — E opens it", Level::Err);
            }
            KeyCode::Char('a') => self.new_deploy_form(),
            KeyCode::Char('e') => {
                if let Some(d) = target {
                    self.edit_deploy_form(cur, &d);
                }
            }
            KeyCode::Char('E') => self.edit_settings(&root),
            KeyCode::Char('x') | KeyCode::Delete => {
                if let Some(d) = target {
                    self.mode = Mode::Confirm {
                        msg: format!("Delete the deploy target {}? Nothing on {} is touched. [y/N]", d.name, d.host),
                        action: Confirmed::RemoveDeploy(d.name.clone(), cur, root),
                    };
                }
            }
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
                self.edit_path(&path, "");
            }
            Err(e) => {
                self.set_status(format!("can't create {}: {e}", config::pretty_path(&settings::dir())), Level::Err)
            }
        }
    }

    /// Writes the script and runs it: in the output pane, or in the real terminal when the
    /// target asks for passwords.
    pub(super) fn run_deploy(&mut self, d: &Deploy, root: &Path, what: Run) {
        if let Some(p) = d.problem() {
            return self.set_status(format!("deploy {}: {p} — D then e fixes it", d.name), Level::Err);
        }
        let (script, tag, label) = match what {
            Run::Deploy => (d.script(root, self.config.keep_deploy_builds), "deploy", format!("deploy {}", d.name)),
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
        let note = (!has_key)
            .then(|| "no ssh key found — ssh-keygen + ssh-copy-id <host>, or run it in the terminal".to_string());
        let defaults = Deploy {
            name,
            host: "pi@raspberrypi.local".into(),
            remote_path: format!("/usr/local/bin/{}", guess.bin),
            build: guess.build,
            artifact: guess.artifact,
            sudo: true,
            ..Deploy::default()
        };
        self.mode = Mode::Form(Form {
            kind: FormKind::Deploy {
                root: root.clone(),
                taken,
                keep_build: self.config.keep_deploy_builds,
                editing: None,
            },
            title: format!(" new deploy target · {} ", Self::project_title(&root)),
            fields: deploy_fields(&defaults, false),
            step: 0,
            note,
            completer: None,
        });
    }

    /// The form again, filled with target `idx` as it is; saving replaces it in the file.
    fn edit_deploy_form(&mut self, idx: usize, d: &Deploy) {
        let root = self.settings_root.clone();
        let taken: Vec<String> = self.settings.deploy.iter().map(|t| t.name.clone()).filter(|n| *n != d.name).collect();
        self.mode = Mode::Form(Form {
            kind: FormKind::Deploy {
                root: root.clone(),
                taken,
                keep_build: self.config.keep_deploy_builds,
                editing: Some(idx),
            },
            title: format!(" edit deploy target {} · {} ", d.name, Self::project_title(&root)),
            fields: deploy_fields(d, true),
            step: 0,
            note: None,
            completer: None,
        });
    }

    /// `x` in the picker, after the y/N.
    pub(super) fn remove_deploy(&mut self, name: &str, idx: usize, root: &Path) {
        match settings::remove_deploy(root, idx) {
            Ok(_) => {
                self.load_settings(true);
                if self.settings.deploy.is_empty() {
                    self.mode = Mode::Normal;
                } else {
                    self.open_deploy(
                        self.settings.deploy.get(idx.saturating_sub(1)).map(|d| d.name.clone()).as_deref(),
                    );
                }
                self.set_status(format!("deleted deploy target {name}"), Level::Ok);
            }
            Err(e) => self.set_status(format!("can't delete {name}: {e}"), Level::Err),
        }
    }

    /// Saves the form's target and shows it in the picker. False keeps the form open.
    pub(super) fn submit_deploy_form(&mut self, form: &Form) -> bool {
        let FormKind::Deploy { root, taken, editing, .. } = &form.kind else { return true };
        let d = deploy_from_form(form);
        let problem = d
            .problem()
            .or_else(|| {
                form.get("keep")
                    .trim()
                    .parse::<usize>()
                    .is_err()
                    .then(|| "keep must be a number; 0 keeps all backups".into())
            })
            .or_else(|| taken.contains(&d.name).then(|| format!("there's a target named {} already", d.name)));
        if let Some(p) = problem {
            self.set_status(p, Level::Err);
            return false;
        }
        let saved = match editing {
            Some(idx) => settings::replace_deploy(root, *idx, &d),
            None => settings::append_deploy(root, &d),
        };
        match saved {
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
    let from = form.get("from").trim();
    Deploy {
        name: form.get("name").trim().to_string(),
        host: form.get("host").trim().to_string(),
        remote_path: form.get("remote path").trim().to_string(),
        build: if build == "-" { String::new() } else { build.to_string() },
        from: from.to_string(),
        artifact: form.get("artifact").trim().to_string(),
        sudo: form.get("sudo") == "yes",
        service: form.get("service").trim().trim_end_matches(".service").to_string(),
        restart: form.get("restart").trim().to_string(),
        keep: form.get("keep").trim().parse().unwrap_or(Deploy::default().keep),
        ssh: form.get("ssh").trim().to_string(),
        interactive: form.get("run") == "terminal",
        key: form.get("key").trim().to_string(),
    }
}

/// The form's fields for `d`. A new target shows its values as defaults that Enter takes;
/// an edited one has them typed in already, so they can be changed or cleared.
fn deploy_fields(d: &Deploy, edit: bool) -> Vec<Field> {
    let text = |label, value: &str, hint: &str| {
        if edit {
            let mut f = Field::text(label, "", hint);
            f.input = LineEdit::new(value);
            f
        } else {
            Field::text(label, value, hint)
        }
    };
    let build_hint =
        if edit { "runs in the project root · empty: none" } else { "runs in the project root · - for none" };
    vec![
        text("name", &d.name, "what the picker calls it"),
        text("host", &d.host, "user@host or an alias from ~/.ssh/config"),
        text("remote path", &d.remote_path, "the file there that gets replaced"),
        text("build", &d.build, build_hint),
        text("from", &d.from, "a branch to build, e.g. main, in a separate checkout · empty: this folder as it is"),
        text("artifact", &d.artifact, "the built file, relative to the project root"),
        Field::choice("sudo", &["yes", "no"], usize::from(!d.sudo), "replace it as root (needed for /usr/…)"),
        text("service", &d.service, "systemd service: restarted and checked, created if missing · empty: none"),
        text("restart", &d.restart, "optional, any command run there afterwards"),
        text("keep", &d.keep.to_string(), "backups to keep there · 0 keeps all"),
        text("ssh", &d.ssh, "extra ssh options, e.g. -p 2222 -i ~/.ssh/pi"),
        text("key", &d.key, "optional key that deploys without the picker, e.g. F9"),
        Field::choice(
            "run",
            &["background", "terminal"],
            usize::from(d.interactive),
            "terminal: when ssh or sudo ask for a password",
        ),
    ]
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
        let arm = install::scan(root, install_dir)
            .into_iter()
            .find(|c| c.triple.as_deref().is_some_and(|t| t.starts_with("aarch64") || t.starts_with("arm")));
        if let Some(c) = arm {
            let triple = c.triple.clone().unwrap_or_default();
            let release = if c.kind.starts_with("release") { " --release" } else { "" };
            let example = if c.kind.contains("example") { format!(" --example {}", c.name) } else { String::new() };
            return Guess {
                build: format!("{cargo} build{release} --target {triple}{example}"),
                artifact: c.rel,
                bin: c.name,
            };
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
    let build =
        project.and_then(|p| project::plan(Op::Release, None, &p.root, &Default::default()).ok()).map(|p| p.cmd);
    Guess { build: build.unwrap_or_default(), artifact: String::new(), bin: dir_name }
}
