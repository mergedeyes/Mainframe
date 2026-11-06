//! App state and the key dispatch for the normal mode; each other mode lives in its own file.

mod actions;
mod files;
mod git_menu;
mod installer;
mod prompt;
mod task_picker;
mod tasks;

use std::{
    env,
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::{Duration, Instant},
};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub use installer::{AfterReview, DesktopReview, InstallPicker};
pub use task_picker::TaskPicker;
pub use tasks::Tab;

use crate::{
    browser::{self, Browser, Preview},
    complete::Completer,
    config::{self, Config},
    form::Form,
    git::{self, GIT, GitInfo},
    history::History,
    line::{Edit, LineEdit},
    project::{self, Op, Plan, Project, quote},
    watch,
};

pub enum Mode {
    Normal,
    Filter(LineEdit),
    Prompt {
        kind: PromptKind,
        input: LineEdit,
        completer: Option<Completer>,
        /// Walking the `:` history: the entry shown and what was typed before.
        browsing: Option<(Option<usize>, String)>,
    },
    Confirm { msg: String, action: Confirmed },
    Help,
    GitMenu,
    Install(InstallPicker),
    Tasks(TaskPicker),
    Form(Form),
    DesktopReview(DesktopReview),
}

#[derive(Clone)]
pub enum PromptKind {
    Command,
    NewFile,
    NewDir,
    Rename(PathBuf),
    Cd,
    Commit { all: bool },
    SwitchBranch,
    NewBranch,
}

impl PromptKind {
    pub fn label(&self) -> &'static str {
        match self {
            PromptKind::Command => "run",
            PromptKind::NewFile => "new file",
            PromptKind::NewDir => "new dir",
            PromptKind::Rename(_) => "rename/move to",
            PromptKind::Cd => "go to (path or zoxide keywords)",
            PromptKind::Commit { all: false } => "commit message",
            PromptKind::Commit { all: true } => "commit all, message",
            PromptKind::SwitchBranch => "switch to branch",
            PromptKind::NewBranch => "new branch",
        }
    }
}

/// What happens when a y/N confirmation is answered with y.
pub enum Confirmed {
    Delete(PathBuf),
    Task(Plan),
}


#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Browser,
    Output,
}

#[derive(Clone, Copy)]
pub enum Level {
    Info,
    Ok,
    Err,
}

/// A command that needs the real terminal (editor, interactive run, shell).
pub struct Foreground {
    pub cmd: String,
    pub cwd: PathBuf,
    /// Pause with "press Enter" afterwards so output stays readable.
    pub wait: bool,
    pub then_reload_config: bool,
    /// Capture stdout (the tty stays free for pickers like fzf) and cd to it.
    pub cd_to_output: bool,
}


pub struct App {
    pub config: Config,
    pub browser: Browser,
    pub tabs: Vec<Tab>,
    /// Index of the tab shown in the output pane.
    pub tab: usize,
    pub show_output: bool,
    pub zoom_output: bool,
    pub output_pct: u16,
    pub show_preview: bool,
    /// Modified files preview as their `git diff` (V toggles).
    pub diff_preview: bool,
    pub focus: Focus,
    pub mode: Mode,
    pub status: Option<(String, Level, Instant)>,
    pub yank: Option<PathBuf>,

    // Cached per selection / per directory, refreshed in `refresh`.
    sel_key: Option<Option<PathBuf>>,
    info_cwd: PathBuf,
    pub preview: Option<Preview>,
    pub run_hint: Option<String>,
    pub project: Option<Project>,
    pub git: Option<GitInfo>,
    /// `git status` runs on a thread; only the answer for the latest request counts.
    git_gen: u64,
    git_pending: bool,
    git_tx: Sender<(u64, Option<GitInfo>)>,
    git_rx: Receiver<(u64, Option<GitInfo>)>,
    /// Last look at the current directory, to notice changes made elsewhere.
    dir_fp: (PathBuf, u64),
    dir_checked: Instant,

    // Written by the UI so paging knows how tall things are.
    pub browser_height: u16,

    pub pending: Option<Foreground>,
    pub quit: bool,
    /// zoxide is installed and enabled in the config.
    zoxide: bool,
    history: History,
}

impl App {
    pub fn new(start: PathBuf, config: Config, config_err: Option<String>) -> Self {
        let (git_tx, git_rx) = mpsc::channel();
        let mut app = Self {
            browser: Browser::new(start, config.show_hidden),
            show_preview: config.preview,
            diff_preview: true,
            config,
            tabs: vec![Tab::default()],
            tab: 0,
            show_output: false,
            zoom_output: false,
            output_pct: 40,
            focus: Focus::Browser,
            mode: Mode::Normal,
            status: None,
            yank: None,
            sel_key: None,
            info_cwd: PathBuf::new(),
            preview: None,
            run_hint: None,
            project: None,
            git: None,
            git_gen: 0,
            git_pending: false,
            git_tx,
            git_rx,
            dir_fp: (PathBuf::new(), 0),
            dir_checked: Instant::now(),
            browser_height: 10,
            pending: None,
            quit: false,
            zoxide: false,
            history: History::load(),
        };
        app.zoxide = app.config.zoxide && project::which("zoxide").is_some();
        app.browser.select_first();
        match config_err {
            Some(e) => app.set_status(e, Level::Err),
            None => app.set_status("? for help", Level::Info),
        }
        app.refresh();
        app
    }

    pub fn set_status(&mut self, msg: impl Into<String>, level: Level) {
        self.status = Some((msg.into(), level, Instant::now()));
    }

    /// Called every loop iteration: pumps task output and expires old messages.
    pub fn tick(&mut self) {
        self.poll_tasks();
        self.poll_watchers();
        self.poll_git();
        self.poll_dir();
        if let Some((_, _, at)) = &self.status
            && at.elapsed() > Duration::from_secs(6) {
                self.status = None;
            }
        self.refresh();
    }

    /// Something is changing on its own, so the screen should redraw often.
    pub fn busy(&self) -> bool {
        self.task_running() || self.git_pending
    }

    /// Forces preview, run hint, project and git info to be recomputed.
    pub fn invalidate(&mut self) {
        self.sel_key = None;
    }

    fn refresh(&mut self) {
        let sel = self.browser.selected().map(|e| e.path.clone());
        if self.sel_key.as_ref() == Some(&sel) {
            return;
        }
        if self.sel_key.is_none() || self.info_cwd != self.browser.cwd {
            if self.info_cwd != self.browser.cwd && self.zoxide {
                self.spawn_detached(&format!("zoxide add -- {}", quote(&self.browser.cwd)));
            }
            self.info_cwd = self.browser.cwd.clone();
            let _ = env::set_current_dir(&self.browser.cwd);
            self.project = project::detect(&self.browser.cwd);
            self.request_git();
        }
        self.sel_key = Some(sel);
        self.update_preview();
        let entry = self.browser.selected().cloned();
        self.run_hint = project::plan(Op::Run, entry.as_ref(), &self.browser.cwd, &self.config)
            .ok()
            .map(|p| p.cmd);
    }

    /// Reloads the file list when something else changed this directory (or the git index).
    fn poll_dir(&mut self) {
        if self.dir_checked.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.dir_checked = Instant::now();
        let extra: Vec<PathBuf> = self
            .git
            .iter()
            .flat_map(|g| [g.root.join(".git/index"), g.root.join(".git/HEAD")])
            .collect();
        let fp = watch::dir_fingerprint(&self.browser.cwd, &extra);
        let (cwd, old) = std::mem::replace(&mut self.dir_fp, (self.browser.cwd.clone(), fp));
        if cwd == self.browser.cwd && old != fp {
            self.browser.reload();
            self.invalidate();
        }
    }

    fn request_git(&mut self) {
        self.git_gen += 1;
        self.git_pending = true;
        // Within the same repo the old status stays up until the new one arrives.
        if self.git.as_ref().is_some_and(|g| !self.browser.cwd.starts_with(&g.root)) {
            self.git = None;
        }
        let (tx, generation, cwd) = (self.git_tx.clone(), self.git_gen, self.browser.cwd.clone());
        thread::spawn(move || {
            // The app may be gone by now; nothing to do then.
            let _ = tx.send((generation, git::status(&cwd)));
        });
    }

    fn poll_git(&mut self) {
        while let Ok((generation, info)) = self.git_rx.try_recv() {
            if generation == self.git_gen {
                self.git = info;
                self.git_pending = false;
                // Whether the selection previews as a diff depends on the new status.
                self.update_preview();
            }
        }
    }

    fn update_preview(&mut self) {
        let entry = self.browser.selected();
        self.preview = match entry {
            Some(e) if self.show_preview => {
                let modified = !e.is_dir && self.git.as_ref().is_some_and(|g| g.modified(&e.path));
                let diff = (self.diff_preview && modified).then(|| git::diff(&e.path)).flatten();
                Some(diff.map_or_else(|| browser::preview(e, self.browser.show_hidden), Preview::Diff))
            }
            _ => None,
        };
    }

    /// For decisions that can't wait for the background `git status`.
    fn await_git(&mut self) {
        if self.git_pending && self.git.is_none() {
            self.git = git::status(&self.browser.cwd);
            self.git_pending = false;
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        match self.mode {
            Mode::Help => self.mode = Mode::Normal,
            Mode::Filter(_) => self.key_filter(key),
            Mode::Prompt { .. } => self.key_prompt(key),
            Mode::Confirm { .. } => self.key_confirm(key),
            Mode::GitMenu => self.key_git(key),
            Mode::Install(_) => self.key_install(key),
            Mode::Tasks(_) => self.key_task_picker(key),
            Mode::Form(_) => self.key_form(key),
            Mode::DesktopReview(_) => self.key_desktop_review(key),
            Mode::Normal => self.key_normal(key),
        }
        if !self.show_output {
            self.focus = Focus::Browser;
        }
        self.refresh();
    }

    fn key_normal(&mut self, key: KeyEvent) {
        if let Some(action) = self.config.actions.iter().find(|a| a.matches(&key)).cloned() {
            self.custom_action(&action);
            return;
        }
        if self.focus == Focus::Output && self.key_output(key) {
            return;
        }

        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('c') if self.tab().running() => self.kill_task(),
                KeyCode::Char('c') => self.set_status("nothing running here — q quits", Level::Info),
                KeyCode::Char('r') => {
                    self.browser.reload();
                    self.invalidate();
                    self.set_status("reloaded", Level::Info);
                }
                KeyCode::Char('d') => self.browser.move_by(self.browser_height as isize / 2),
                KeyCode::Char('u') => self.browser.move_by(-(self.browser_height as isize / 2)),
                _ => {}
            }
            return;
        }

        let page = self.browser_height.max(1) as isize;
        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('?') => self.mode = Mode::Help,

            // navigation
            KeyCode::Char('j') | KeyCode::Down => self.browser.move_by(1),
            KeyCode::Char('k') | KeyCode::Up => self.browser.move_by(-1),
            KeyCode::PageDown => self.browser.move_by(page),
            KeyCode::PageUp => self.browser.move_by(-page),
            KeyCode::Home => self.browser.select_first(),
            KeyCode::End => self.browser.select_last(),
            KeyCode::Char('h') | KeyCode::Left | KeyCode::Backspace => {
                self.browser.parent();
                self.invalidate();
            }
            KeyCode::Char('l') | KeyCode::Right => self.enter_dir(),
            KeyCode::Enter => self.open_selected(),
            KeyCode::Char('~') => {
                self.browser.cd(config::home());
                self.invalidate();
            }
            KeyCode::Char('.') => {
                self.browser.show_hidden = !self.browser.show_hidden;
                self.browser.reload();
                self.invalidate();
            }
            KeyCode::Char('/') => self.mode = Mode::Filter(LineEdit::new(self.browser.filter.clone())),
            KeyCode::Esc => {
                if !self.browser.filter.is_empty() {
                    self.browser.filter.clear();
                    self.browser.apply_filter();
                }
            }
            KeyCode::Char('c') => self.prompt(PromptKind::Cd, ""),
            KeyCode::Char('Z') => self.zoxide_pick(),

            // actions
            KeyCode::Char('e') => self.edit_selected(),
            KeyCode::Char('o') => self.open_vscode(false),
            KeyCode::Char('O') => self.open_vscode(true),
            KeyCode::Char('r') => self.run_op(Op::Run, false),
            KeyCode::Char('R') => self.run_op(Op::Run, true),
            KeyCode::Char('b') => self.run_op(Op::Build, false),
            KeyCode::Char('B') => self.run_op(Op::Release, false),
            KeyCode::Char('t') => self.run_op(Op::Test, false),
            KeyCode::Char('T') => self.open_task_picker(),
            KeyCode::Char('i') => self.open_install_picker(),
            KeyCode::Char('g') => {
                self.await_git();
                self.mode = Mode::GitMenu;
            }
            KeyCode::Char('x') => self.kill_task(),
            KeyCode::Char('@') => self.rerun(),
            KeyCode::Char('W') => self.toggle_watch(),
            KeyCode::Char('n') => self.step_loc(true),
            KeyCode::Char('N') => self.step_loc(false),
            KeyCode::Char('[') => self.switch_tab(-1),
            KeyCode::Char(']') => self.switch_tab(1),
            KeyCode::Char(':') => self.prompt(PromptKind::Command, ""),
            KeyCode::Char('!') => self.open_shell(),
            KeyCode::Char(',') => self.edit_config(),

            // files
            KeyCode::Char('a') => self.prompt(PromptKind::NewFile, ""),
            KeyCode::Char('A') => self.prompt(PromptKind::NewDir, ""),
            KeyCode::Char('m') | KeyCode::F(2) => {
                if let Some(e) = self.browser.selected() {
                    let (path, name) = (e.path.clone(), e.name.clone());
                    self.prompt(PromptKind::Rename(path), &name);
                }
            }
            KeyCode::Char('d') | KeyCode::Delete => self.confirm_delete(),
            KeyCode::Char('y') => {
                if let Some(e) = self.browser.selected() {
                    let msg = format!("yanked {} — p to copy here, P to move here", e.name);
                    self.yank = Some(e.path.clone());
                    self.set_status(msg, Level::Info);
                }
            }
            KeyCode::Char('p') => self.paste(false),
            KeyCode::Char('P') => self.paste(true),

            // view
            KeyCode::Char('v') => {
                self.show_preview = !self.show_preview;
                self.invalidate();
            }
            KeyCode::Char('V') => {
                self.diff_preview = !self.diff_preview;
                self.show_preview = true;
                self.update_preview();
                let msg = if self.diff_preview { "modified files preview as diff" } else { "preview shows file contents" };
                self.set_status(msg, Level::Info);
            }
            KeyCode::Char('w') => {
                self.show_output = !self.show_output;
                self.zoom_output = false;
            }
            KeyCode::Char('z') if self.show_output => self.zoom_output = !self.zoom_output,
            KeyCode::Tab if self.show_output => {
                self.focus = match self.focus {
                    Focus::Browser => Focus::Output,
                    Focus::Output => Focus::Browser,
                };
            }
            KeyCode::Char('+') | KeyCode::Char('=') => self.output_pct = (self.output_pct + 5).min(85),
            KeyCode::Char('-') => self.output_pct = self.output_pct.saturating_sub(5).max(15),
            _ => {}
        }
    }

    /// Scroll keys while the output pane has focus. Returns false to fall through.
    fn key_output(&mut self, key: KeyEvent) -> bool {
        let out = &mut self.tabs[self.tab].output;
        let page = out.height.max(1) as isize;
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => out.scroll_by(-1),
            KeyCode::Char('k') | KeyCode::Up => out.scroll_by(1),
            KeyCode::PageDown => out.scroll_by(-page),
            KeyCode::PageUp => out.scroll_by(page),
            KeyCode::Home => out.scroll_to_top(),
            KeyCode::End => out.scroll = 0,
            KeyCode::Enter => return self.open_loc(false),
            KeyCode::Char('o') => return self.open_loc(true),
            KeyCode::Esc => self.focus = Focus::Browser,
            _ => return false,
        }
        true
    }

    fn key_filter(&mut self, key: KeyEvent) {
        let Mode::Filter(edit) = &mut self.mode else { return };
        match key.code {
            KeyCode::Esc => {
                self.browser.filter.clear();
                self.browser.apply_filter();
                self.mode = Mode::Normal;
            }
            KeyCode::Enter => {
                self.mode = Mode::Normal;
                // Single match: jump straight into it.
                if self.browser.visible.len() == 1 && self.browser.selected().is_some_and(|e| e.is_dir) {
                    self.enter_dir();
                }
            }
            KeyCode::Down => self.browser.move_by(1),
            KeyCode::Up => self.browser.move_by(-1),
            _ => {
                if edit.key(key) == Edit::Changed {
                    self.browser.filter = edit.text.clone();
                    self.browser.apply_filter();
                    self.browser.select_first();
                }
            }
        }
    }

    fn key_confirm(&mut self, key: KeyEvent) {
        let Mode::Confirm { action, .. } = std::mem::replace(&mut self.mode, Mode::Normal) else { return };
        if !matches!(key.code, KeyCode::Char('y') | KeyCode::Char('Y')) {
            self.set_status("cancelled", Level::Info);
            return;
        }
        match action {
            Confirmed::Delete(path) => self.delete(&path),
            Confirmed::Task(plan) => self.start_task(plan.cmd.replace(GIT, "git"), plan),
        }
    }
}
