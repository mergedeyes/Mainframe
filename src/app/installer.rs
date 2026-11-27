//! The `i` install picker and the review of `.desktop` entries it may change.

use std::path::{Path, PathBuf};

use ratatui::{
    crossterm::event::{KeyCode, KeyEvent},
    style::{Color, Style},
    text::{Line, Span},
    widgets::ListState,
};

use super::{App, Confirmed, Level, Mode};
use crate::{
    cmd::{Builtin, Cmd, Job, Step},
    config::{self, DesktopMode},
    desktop::{self, DesktopPlan},
    filter_list, git,
    install::{self, Candidate},
    project::{self, Kind, Plan},
};

pub struct InstallPicker {
    pub root: PathBuf,
    pub install_dir: PathBuf,
    /// Offer `cargo install --path .` as the first row.
    pub cargo: bool,
    pub items: Vec<Candidate>,
    pub state: ListState,
    /// Desktop entries for the `cargo install` row.
    pub cargo_desktop: Vec<DesktopPlan>,
    /// The checkout lacks commits its remote branch has: "main is 8 commits behind origin/main".
    pub behind: Option<String>,
}

impl InstallPicker {
    /// Desktop entries belonging to the selected row.
    pub fn selected_desktop(&self) -> &[DesktopPlan] {
        match self.selected() {
            Some(c) => &c.desktop,
            None => &self.cargo_desktop,
        }
    }

    pub fn len(&self) -> usize {
        self.items.len() + self.cargo as usize
    }

    /// None while the cargo row is selected.
    pub fn selected(&self) -> Option<&Candidate> {
        let i = self.state.selected()?;
        self.items.get(i.checked_sub(self.cargo as usize)?)
    }
}

/// Walks through desktop entries that would change, one diff at a time.
pub struct DesktopReview {
    pub plans: Vec<DesktopPlan>,
    pub idx: usize,
    pub accepted: Vec<DesktopPlan>,
    pub then: AfterReview,
    pub scroll: u16,
}

/// What the review was gating.
pub enum AfterReview {
    /// The candidate, the project root, and what runs first (`p`: pull, maybe rebuild).
    InstallBinary(Box<Candidate>, PathBuf, Option<Vec<Step>>),
    CargoInstall(PathBuf, Option<Vec<Step>>),
    Nothing,
}

impl App {
    /// Installs, after the review of the desktop entries it changes when the config asks for one.
    pub(super) fn start_install(&mut self, then: AfterReview, plans: Vec<DesktopPlan>) {
        let ask = self.config.desktop_entries == DesktopMode::Ask;
        match self.config.desktop_entries {
            DesktopMode::Never => self.finish_install(then, Vec::new()),
            _ => self.review_desktop(then, plans, ask),
        }
    }

    pub(super) fn open_install_picker(&mut self) {
        let start = match self.browser.selected() {
            Some(e) if e.is_dir => e.path.clone(),
            _ => self.browser.cwd.clone(),
        };
        let project = project::detect(&start);
        let root = project.as_ref().map_or_else(|| self.browser.cwd.clone(), |p| p.root.clone());
        let cargo = project.as_ref().is_some_and(|p| p.kind == Kind::Cargo);
        let install_dir = self.config.install_dir();
        let mut items = install::scan(&root, &install_dir);
        for c in &mut items {
            c.desktop = desktop::plans(&root, Some(&c.name), &install_dir);
        }
        let cargo_desktop = if cargo { desktop::plans(&root, None, &install_dir) } else { Vec::new() };
        if items.is_empty() && !cargo {
            self.set_status(format!("no executables found under {}", config::pretty_path(&root)), Level::Err);
            return;
        }
        // Start on the selected file if it's one of them, else on the first built binary.
        let selected = self.browser.selected().map(|e| e.path.clone());
        let pos = selected
            .and_then(|s| items.iter().position(|c| c.path == s))
            .map(|i| i + cargo as usize)
            .unwrap_or(if cargo && !items.is_empty() { 1 } else { 0 });
        let behind = git::behind_upstream(&root).map(|(branch, upstream, n)| {
            format!("{branch} is {n} commit{} behind {upstream}", if n == 1 { "" } else { "s" })
        });
        self.mode = Mode::Install(InstallPicker {
            root,
            install_dir,
            cargo,
            items,
            state: ListState::default().with_selected(Some(pos)),
            cargo_desktop,
            behind,
        });
    }

    pub(super) fn key_install(&mut self, key: KeyEvent) {
        let Mode::Install(picker) = &mut self.mode else { return };
        let len = picker.len();
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.mode = Mode::Normal,
            KeyCode::Char('j') | KeyCode::Down => filter_list::step(&mut picker.state, len, 1),
            KeyCode::Char('k') | KeyCode::Up => filter_list::step(&mut picker.state, len, -1),
            KeyCode::Enter | KeyCode::Char('i') => {
                let root = picker.root.clone();
                let plans = picker.selected_desktop().to_vec();
                let then = match picker.selected().cloned() {
                    Some(c) => AfterReview::InstallBinary(Box::new(c), root, None),
                    None => AfterReview::CargoInstall(root, None),
                };
                // An install from a checkout that isn't pulled is the old version: ask first.
                let behind = picker.behind.clone();
                self.mode = Mode::Normal;
                match behind {
                    Some(behind) => {
                        self.mode = Mode::Confirm {
                            msg: format!("{behind}: install without those commits? [y/N] (p in i pulls them first)"),
                            action: Confirmed::Install(then, plans),
                        };
                    }
                    None => self.start_install(then, plans),
                }
            }
            // Pull (and rebuild when it's a build), then install: the newest version, fetched now.
            KeyCode::Char('p') => {
                let root = picker.root.clone();
                let plans = picker.selected_desktop().to_vec();
                let then = match picker.selected().cloned() {
                    Some(c) => match install::pull_first(&c, picker.cargo) {
                        Ok(before) => AfterReview::InstallBinary(Box::new(c), root, Some(before)),
                        Err(e) => return self.set_status(e, Level::Info),
                    },
                    None => AfterReview::CargoInstall(root, Some(vec![install::pull()])),
                };
                self.mode = Mode::Normal;
                self.start_install(then, plans);
            }
            KeyCode::Char('d') => {
                let plans = picker.selected_desktop().to_vec();
                if plans.is_empty() {
                    return self.set_status("no .desktop entry in this project launches it", Level::Info);
                }
                self.mode = Mode::Normal;
                self.review_desktop(AfterReview::Nothing, plans, true);
            }
            KeyCode::Char('u') => {
                let Some(c) = picker.selected().cloned() else { return };
                let Some(shadow) = c.shadowed_by.clone() else {
                    return self.set_status("nothing shadows this one on PATH", Level::Info);
                };
                let job = install::removal(&shadow, &c.name);
                let shown = job.display(&self.config.shell());
                self.mode = Mode::Confirm {
                    msg: format!(
                        "Remove {} so {} runs? ({shown}) [y/N]",
                        config::pretty_path(&shadow),
                        config::pretty_path(&c.dest)
                    ),
                    action: Confirmed::Task(Plan { cmd: Cmd::Steps(vec![job]), cwd: self.browser.cwd.clone() }),
                };
            }
            KeyCode::Char('r') => {
                if let Some(c) = picker.selected().cloned() {
                    self.mode = Mode::Normal;
                    let plan = Plan { cmd: Step::new(c.path.to_string_lossy()).into(), cwd: picker_cwd(&c) };
                    self.start_task(c.name.clone(), plan);
                }
            }
            _ => {}
        }
    }

    /// Shows diffs for entries that would change (when `ask`), otherwise goes
    /// straight to installing.
    pub(super) fn review_desktop(&mut self, then: AfterReview, plans: Vec<DesktopPlan>, ask: bool) {
        let todo: Vec<DesktopPlan> = plans.into_iter().filter(DesktopPlan::needs_update).collect();
        if todo.is_empty() {
            if matches!(then, AfterReview::Nothing) {
                self.set_status("desktop entry is up to date", Level::Ok);
            }
            return self.finish_install(then, Vec::new());
        }
        if !ask {
            return self.finish_install(then, todo);
        }
        self.mode = Mode::DesktopReview(DesktopReview { plans: todo, idx: 0, accepted: Vec::new(), then, scroll: 0 });
    }

    pub(super) fn key_desktop_review(&mut self, key: KeyEvent) {
        let Mode::DesktopReview(review) = &mut self.mode else { return };
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.mode = Mode::Normal;
                self.set_status("cancelled — nothing installed", Level::Info)
            }
            KeyCode::Char('j') | KeyCode::Down => review.scroll = review.scroll.saturating_add(1),
            KeyCode::Char('k') | KeyCode::Up => review.scroll = review.scroll.saturating_sub(1),
            KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('n') => {
                if key.code != KeyCode::Char('n') {
                    review.accepted.push(review.plans[review.idx].clone());
                }
                review.idx += 1;
                review.scroll = 0;
                if review.idx == review.plans.len() {
                    let Mode::DesktopReview(review) = std::mem::replace(&mut self.mode, Mode::Normal) else { return };
                    self.finish_install(review.then, review.accepted);
                }
            }
            _ => {}
        }
    }

    pub(super) fn finish_install(&mut self, then: AfterReview, desktop: Vec<DesktopPlan>) {
        match then {
            AfterReview::InstallBinary(c, root, before) => self.install_binary(&c, &root, before.as_deref()),
            AfterReview::CargoInstall(root, before) => {
                let cmd = then_run(before.as_deref(), install::cargo_install(&self.config.install_dir()).into());
                self.start_task("cargo install".into(), Plan { cmd, cwd: root });
            }
            AfterReview::Nothing => {}
        }

        let mut written = Vec::new();
        let mut sudo = Vec::new();
        for plan in &desktop {
            if plan.system {
                match desktop::system_cmd(plan) {
                    Ok(cmd) => sudo.push(cmd),
                    Err(e) => return self.set_status(format!("desktop entry: {e}"), Level::Err),
                }
            } else if let Err(e) = desktop::apply_user(plan) {
                return self.set_status(format!("desktop entry {}: {e}", plan.target.display()), Level::Err);
            } else {
                written.push(config::pretty_path(&plan.target));
            }
        }
        if !written.is_empty() {
            if project::which("update-desktop-database").is_some() {
                let dir = desktop::app_dirs()[0].clone();
                self.spawn_detached(Step::words("update-desktop-database -q").path(&dir));
            }
            let msg = format!("desktop entry updated: {}", written.join(", "));
            if self.tab().running() {
                // The task's own result will take over the status line; keep a record in its output.
                self.tab_mut().output.push(Line::from(Span::styled(format!("✔ {msg}"), Style::new().fg(Color::Green))));
            }
            self.set_status(msg, Level::Ok);
        }
        if !sudo.is_empty() {
            self.foreground(Plan { cmd: Cmd::Sh(sudo.join(" && ")), cwd: self.browser.cwd.clone() }, true);
        }
    }

    pub(super) fn install_binary(&mut self, c: &Candidate, root: &Path, before: Option<&[Step]>) {
        let install = Builtin::InstallFile { src: c.path.clone(), dest: c.dest.clone() };
        let cmd = then_run(before, install.into());
        self.start_task(format!("install {}", c.name), Plan { cmd, cwd: root.to_path_buf() });
    }
}

/// `job`, after the steps in `before` when there are some.
fn then_run(before: Option<&[Step]>, job: Job) -> Cmd {
    let mut jobs: Vec<Job> = before.unwrap_or_default().iter().cloned().map(Into::into).collect();
    jobs.push(job);
    Cmd::Steps(jobs)
}

fn picker_cwd(c: &Candidate) -> PathBuf {
    c.path.parent().unwrap_or(Path::new("/")).to_path_buf()
}
