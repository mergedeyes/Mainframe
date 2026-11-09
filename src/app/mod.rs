//! App state and the key dispatch for the normal mode; each other mode lives in its own file.

mod actions;
mod cleanup;
mod deploy;
mod file_finder;
mod files;
mod git_menu;
mod installer;
mod notes_view;
mod project_picker;
mod prompt;
mod task_picker;
mod tasks;
mod workspaces;

use std::{
    env,
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::{Duration, Instant},
};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub use cleanup::CleanupPicker;
pub use deploy::{DeployPicker, deploy_from_form};
pub use file_finder::FileFinder;
pub use installer::{AfterReview, DesktopReview, InstallPicker};
pub use notes_view::NotesView;
pub use project_picker::ProjectPicker;
pub use task_picker::TaskPicker;
pub use tasks::Tab;
use workspaces::Workspaces;

use crate::{
    browser::{self, Browser, Preview},
    complete::Completer,
    config::{self, Config},
    form::Form,
    git::{self, GIT, GitInfo},
    history::History,
    line::{Edit, LineEdit},
    notes,
    project::{self, Op, Plan, Project, quote},
    projects,
    settings::{Deploy, ProjectSettings},
    watch::{self, Watcher},
};

/// How often the whole worktree is walked for edits outside the current directory.
const REPO_WATCH_INTERVAL: Duration = Duration::from_secs(2);

pub enum Mode {
    Normal,
    Filter(LineEdit),
    /// `/` in the output pane.
    Search(LineEdit),
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
    Files(FileFinder),
    Projects(ProjectPicker),
    Notes(NotesView),
    Form(Form),
    DesktopReview(DesktopReview),
    Deploy(DeployPicker),
    Cleanup(CleanupPicker),
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
    PushBranch,
    SaveOutput,
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
            PromptKind::PushBranch => "push branch",
            PromptKind::SaveOutput => "save output to",
        }
    }
}

/// What happens when a y/N confirmation is answered with y.
pub enum Confirmed {
    Delete(Vec<PathBuf>),
    Task(Plan),
    /// A task with its own tab label.
    Labeled(String, Plan),
    /// Put a deploy target's newest backup back (target, project root).
    Rollback(Box<Deploy>, PathBuf),
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
    pub yank: Vec<PathBuf>,

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
    /// Last look at the repo's HEAD, index and refs (keyed by repo root).
    git_fp: Option<(PathBuf, u64)>,
    dir_checked: Instant,
    /// Edits anywhere in the repo, so dirty markers outside this directory stay current.
    repo_watch: Option<(PathBuf, Watcher)>,

    // Written by the UI so paging knows how tall things are.
    pub browser_height: u16,

    pub pending: Option<Foreground>,
    pub quit: bool,
    /// zoxide is installed and enabled in the config.
    zoxide: bool,
    history: History,
    /// Whose notes `M` opens from here, and how many of its todos are open (header badge).
    pub notes_root: PathBuf,
    pub open_todos: usize,
    /// `E` in the notes view: open them again once the editor closes.
    reopen_notes: Option<(PathBuf, String)>,
    /// The settings file of `notes_root` (deploy targets, project actions), and whose they are.
    pub settings: ProjectSettings,
    settings_root: PathBuf,
    /// `e` in the deploy picker: open it again once the editor closes.
    reopen_deploy: bool,
    /// The project whose output tabs are shown, and the other projects' tabs.
    pub workspace: Option<PathBuf>,
    workspaces: Workspaces,
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
            yank: Vec::new(),
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
            git_fp: None,
            dir_checked: Instant::now(),
            repo_watch: None,
            browser_height: 10,
            pending: None,
            quit: false,
            zoxide: false,
            history: History::load(),
            notes_root: PathBuf::new(),
            open_todos: 0,
            reopen_notes: None,
            settings: ProjectSettings::default(),
            settings_root: PathBuf::new(),
            reopen_deploy: false,
            workspace: None,
            workspaces: Workspaces::new(),
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
        self.poll_cleanup();
        self.poll_dir();
        if self.repo_watch.as_ref().is_some_and(|(_, w)| w.changed()) {
            self.request_git();
        }
        if let Some((_, _, at)) = &self.status
            && at.elapsed() > Duration::from_secs(6) {
                self.status = None;
            }
        self.refresh();
    }

    /// Something is changing on its own, so the screen should redraw often.
    pub fn busy(&self) -> bool {
        self.task_running() || self.git_pending || matches!(&self.mode, Mode::Cleanup(p) if p.measuring())
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
            self.update_notes_root();
        }
        self.sel_key = Some(sel);
        self.update_preview();
        let entry = self.browser.selected().cloned();
        self.run_hint = project::plan(Op::Run, entry.as_ref(), &self.browser.cwd, &self.config)
            .ok()
            .map(|p| p.cmd);
    }

    /// Notices changes made elsewhere: reloads the file list when this directory changed,
    /// reruns `git status` when a commit, checkout, stage, fetch or push moved the repo.
    fn poll_dir(&mut self) {
        if self.dir_checked.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.dir_checked = Instant::now();
        let fp = watch::dir_fingerprint(&self.browser.cwd);
        let (cwd, old) = std::mem::replace(&mut self.dir_fp, (self.browser.cwd.clone(), fp));
        if cwd == self.browser.cwd && old != fp {
            self.browser.reload();
            self.invalidate();
        }

        let now = self.git.as_ref().map(|g| {
            let fp = watch::files_fingerprint(&g.state_files()).wrapping_add(watch::refs_fingerprint(&g.refs_dir()));
            (g.root.clone(), fp)
        });
        let old = std::mem::replace(&mut self.git_fp, now.clone());
        if let (Some(now), Some(old)) = (now, old)
            && now.0 == old.0
            && now.1 != old.1
        {
            self.request_git();
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

    /// Recomputed whenever the directory changes. The git root counts when the projects
    /// folder doesn't place you; `git status` answers later, so it's redone then too.
    /// Entering another project also brings up its output tabs.
    fn update_notes_root(&mut self) {
        let dir = projects::dir(&self.config);
        let owner = projects::owner(&self.browser.cwd, dir.as_deref(), self.git.as_ref(), self.project.as_ref());
        self.notes_root = owner.clone().unwrap_or_else(|| self.browser.cwd.clone());
        self.refresh_todo_count();
        self.load_settings(false);
        if let Some(root) = owner {
            self.switch_workspace(root);
        }
    }

    pub(super) fn refresh_todo_count(&mut self) {
        self.open_todos = notes::open_count(&self.notes_root);
    }

    fn poll_git(&mut self) {
        while let Ok((generation, info)) = self.git_rx.try_recv() {
            if generation == self.git_gen {
                let root_changed = info.as_ref().map(|g| &g.root) != self.git.as_ref().map(|g| &g.root);
                self.git = info;
                self.git_pending = false;
                if root_changed {
                    self.update_notes_root();
                }
                self.sync_repo_watch();
                // Whether the selection previews as a diff depends on the new status.
                self.update_preview();
            }
        }
    }

    /// Keeps the worktree watcher on the current repo. Not on a repo at $HOME: that's
    /// everything, and dotfile repos mostly track files outside the subtrees you browse.
    fn sync_repo_watch(&mut self) {
        let root = self.git.as_ref().map(|g| g.root.clone()).filter(|r| *r != config::home());
        if self.repo_watch.as_ref().map(|(r, _)| r) != root.as_ref() {
            self.repo_watch = root.map(|r| (r.clone(), Watcher::with_interval(r, REPO_WATCH_INTERVAL)));
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
            Mode::Search(_) => self.key_search(key),
            Mode::Prompt { .. } => self.key_prompt(key),
            Mode::Confirm { .. } => self.key_confirm(key),
            Mode::GitMenu => self.key_git(key),
            Mode::Install(_) => self.key_install(key),
            Mode::Tasks(_) => self.key_task_picker(key),
            Mode::Files(_) => self.key_file_finder(key),
            Mode::Projects(_) => self.key_project_picker(key),
            Mode::Notes(_) => self.key_notes(key),
            Mode::Form(_) => self.key_form(key),
            Mode::DesktopReview(_) => self.key_desktop_review(key),
            Mode::Deploy(_) => self.key_deploy(key),
            Mode::Cleanup(_) => self.key_cleanup(key),
            Mode::Normal => self.key_normal(key),
        }
        if !self.show_output {
            self.focus = Focus::Browser;
        }
        self.refresh();
    }

    fn key_normal(&mut self, key: KeyEvent) {
        if let Some(d) = self.settings.deploy_for_key(&key).cloned() {
            let root = self.settings_root.clone();
            return self.run_deploy(&d, &root, deploy::Run::Deploy);
        }
        // The project's own actions win over the global ones.
        let actions = self.settings.actions.iter().chain(&self.config.actions);
        if let Some(action) = actions.into_iter().find(|a| a.matches(&key)).cloned() {
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
                KeyCode::Char('p') => self.open_file_finder(),
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
            KeyCode::Char('S') => {
                self.browser.sort = self.browser.sort.next();
                self.browser.reload();
                self.set_status(format!("sorted by {}", self.browser.sort.label()), Level::Info);
            }
            KeyCode::Char('/') => self.mode = Mode::Filter(LineEdit::new(self.browser.filter.clone())),
            KeyCode::Esc => {
                if !self.browser.filter.is_empty() {
                    self.browser.filter.clear();
                    self.browser.apply_filter();
                } else if !self.browser.marked.is_empty() {
                    self.browser.marked.clear();
                    self.set_status("marks cleared", Level::Info);
                }
            }
            KeyCode::Char('c') => self.prompt(PromptKind::Cd, ""),
            KeyCode::Char('Z') => self.zoxide_pick(),
            KeyCode::Char('J') => self.open_project_picker(),
            KeyCode::Char('M') => self.open_current_notes(),

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
            KeyCode::Char('D') => self.open_deploy(None),
            KeyCode::Char('C') => self.open_cleanup(),
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
            KeyCode::Char(' ') => self.browser.toggle_mark(),
            KeyCode::Char('y') => self.yank_targets(),
            KeyCode::Char('s') => self.stage(true),
            KeyCode::Char('u') => self.stage(false),
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
            KeyCode::Char('/') => self.mode = Mode::Search(LineEdit::new(out.search.clone())),
            KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                let name: String = self.tabs[self.tab]
                    .label
                    .chars()
                    .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
                    .collect();
                let name = name.split('-').filter(|p| !p.is_empty()).take(4).collect::<Vec<_>>().join("-");
                self.prompt(PromptKind::SaveOutput, &format!("{}.log", if name.is_empty() { "output" } else { &name }));
            }
            KeyCode::Esc if !out.search.is_empty() => out.clear_search(),
            KeyCode::Esc => self.focus = Focus::Browser,
            _ => return false,
        }
        true
    }

    fn key_search(&mut self, key: KeyEvent) {
        let Mode::Search(edit) = &mut self.mode else { return };
        let out = &mut self.tabs[self.tab].output;
        match key.code {
            KeyCode::Esc => {
                out.clear_search();
                self.mode = Mode::Normal;
            }
            KeyCode::Enter => {
                self.mode = Mode::Normal;
                let msg = match out.matches.len() {
                    0 if out.search.is_empty() => return,
                    0 => format!("no match for {}", out.search),
                    n => format!("{n} matches · n N step · Esc clears"),
                };
                self.set_status(msg, Level::Info);
            }
            _ => {
                if edit.key(key) == Edit::Changed {
                    out.set_search(&edit.text);
                }
            }
        }
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
            Confirmed::Delete(paths) => self.delete(&paths),
            Confirmed::Task(plan) => self.start_task(plan.cmd.replace(GIT, "git"), plan),
            Confirmed::Labeled(label, plan) => self.start_task(label, plan),
            Confirmed::Rollback(d, root) => self.run_deploy(&d, &root, deploy::Run::Rollback),
        }
    }
}
