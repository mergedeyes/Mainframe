use std::{
    env, fs,
    io::ErrorKind,
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use ratatui::{
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::ListState,
};

use crate::{
    browser::{self, Browser, Preview},
    config::{self, Action, Config},
    form::{Form, FormEvent, FormKind},
    git::{self, GIT, GitInfo},
    install::{self, Candidate},
    project::{self, Kind, Op, Plan, Project, quote, quote_str},
    task::{Output, Task},
};

pub enum Mode {
    Normal,
    Filter,
    Prompt { kind: PromptKind, input: String },
    Confirm { msg: String, action: Confirmed },
    Help,
    GitMenu,
    Install(InstallPicker),
    Form(Form),
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
            PromptKind::Cd => "go to",
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

pub struct InstallPicker {
    pub root: PathBuf,
    /// Offer `cargo install --path .` as the first row.
    pub cargo: bool,
    pub items: Vec<Candidate>,
    pub state: ListState,
}

impl InstallPicker {
    pub fn len(&self) -> usize {
        self.items.len() + self.cargo as usize
    }

    /// None while the cargo row is selected.
    pub fn selected(&self) -> Option<&Candidate> {
        let i = self.state.selected()?;
        self.items.get(i.checked_sub(self.cargo as usize)?)
    }
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
}

pub struct LastRun {
    pub label: String,
    pub ok: bool,
    pub detail: String,
}

pub struct App {
    pub config: Config,
    pub browser: Browser,
    pub output: Output,
    pub task: Option<Task>,
    pub last_run: Option<LastRun>,
    pub show_output: bool,
    pub zoom_output: bool,
    pub output_pct: u16,
    pub show_preview: bool,
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

    // Written by the UI so paging knows how tall things are.
    pub browser_height: u16,
    pub output_height: u16,

    pub pending: Option<Foreground>,
    pub quit: bool,
}

impl App {
    pub fn new(start: PathBuf, config: Config, config_err: Option<String>) -> Self {
        let mut app = Self {
            browser: Browser::new(start, config.show_hidden),
            show_preview: config.preview,
            config,
            output: Output::default(),
            task: None,
            last_run: None,
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
            browser_height: 10,
            output_height: 10,
            pending: None,
            quit: false,
        };
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
        if let Some(task) = &mut self.task
            && let Some((status, took)) = task.poll(&mut self.output) {
                let label = task.label.clone();
                self.task = None;
                let ok = status.success();
                let code = match status.code() {
                    Some(c) => format!("exit {c}"),
                    None => "killed".into(),
                };
                let detail = format!("{code} · {:.1}s", took.as_secs_f32());
                let style = if ok { Color::Green } else { Color::Red };
                self.output.push(Line::from(Span::styled(
                    format!("── {} {label}: {detail}", if ok { "✔" } else { "✘" }),
                    Style::new().fg(style).add_modifier(Modifier::BOLD),
                )));
                self.set_status(
                    format!("{label}: {detail}"),
                    if ok { Level::Ok } else { Level::Err },
                );
                self.last_run = Some(LastRun { label, ok, detail });
                // Builds create files and git commands change status; show both.
                self.browser.reload();
                self.invalidate();
            }
        if let Some((_, _, at)) = &self.status
            && at.elapsed() > Duration::from_secs(6) {
                self.status = None;
            }
        self.refresh();
    }

    pub fn task_running(&self) -> bool {
        self.task.is_some()
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
            self.info_cwd = self.browser.cwd.clone();
            self.project = project::detect(&self.browser.cwd);
            self.git = git::status(&self.browser.cwd);
        }
        self.sel_key = Some(sel);
        let entry = self.browser.selected().cloned();
        self.preview = match (&entry, self.show_preview) {
            (Some(e), true) => Some(browser::preview(e, self.browser.show_hidden)),
            _ => None,
        };
        self.run_hint = project::plan(Op::Run, entry.as_ref(), &self.browser.cwd, &self.config)
            .ok()
            .map(|p| p.cmd);
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        match self.mode {
            Mode::Help => self.mode = Mode::Normal,
            Mode::Filter => self.key_filter(key),
            Mode::Prompt { .. } => self.key_prompt(key),
            Mode::Confirm { .. } => self.key_confirm(key),
            Mode::GitMenu => self.key_git(key),
            Mode::Install(_) => self.key_install(key),
            Mode::Form(_) => self.key_form(key),
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
                KeyCode::Char('c') if self.task_running() => self.kill_task(),
                KeyCode::Char('c') => self.quit = true,
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
            KeyCode::Char('/') => self.mode = Mode::Filter,
            KeyCode::Esc => {
                if !self.browser.filter.is_empty() {
                    self.browser.filter.clear();
                    self.browser.apply_filter();
                }
            }
            KeyCode::Char('c') => self.prompt(PromptKind::Cd, ""),

            // actions
            KeyCode::Char('e') => self.edit_selected(),
            KeyCode::Char('o') => self.open_vscode(false),
            KeyCode::Char('O') => self.open_vscode(true),
            KeyCode::Char('r') => self.run_op(Op::Run, false),
            KeyCode::Char('R') => self.run_op(Op::Run, true),
            KeyCode::Char('b') => self.run_op(Op::Build, false),
            KeyCode::Char('B') => self.run_op(Op::Release, false),
            KeyCode::Char('t') => self.run_op(Op::Test, false),
            KeyCode::Char('i') => self.open_install_picker(),
            KeyCode::Char('g') => self.mode = Mode::GitMenu,
            KeyCode::Char('x') => self.kill_task(),
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
        let page = self.output_height.max(1) as isize;
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.output.scroll_by(-1),
            KeyCode::Char('k') | KeyCode::Up => self.output.scroll_by(1),
            KeyCode::PageDown => self.output.scroll_by(-page),
            KeyCode::PageUp => self.output.scroll_by(page),
            KeyCode::Home => self.output.scroll = self.output.lines.len(),
            KeyCode::End => self.output.scroll = 0,
            KeyCode::Esc => self.focus = Focus::Browser,
            _ => return false,
        }
        true
    }

    fn key_filter(&mut self, key: KeyEvent) {
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
            KeyCode::Backspace => {
                self.browser.filter.pop();
                self.browser.apply_filter();
            }
            KeyCode::Char(c) => {
                self.browser.filter.push(c);
                self.browser.apply_filter();
                self.browser.select_first();
            }
            _ => {}
        }
    }

    fn prompt(&mut self, kind: PromptKind, initial: &str) {
        self.mode = Mode::Prompt { kind, input: initial.to_string() };
    }

    fn key_prompt(&mut self, key: KeyEvent) {
        let Mode::Prompt { kind, input } = &mut self.mode else { return };
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => self.mode = Mode::Normal,
            KeyCode::Char('c') if ctrl => self.mode = Mode::Normal,
            KeyCode::Char('u') if ctrl => input.clear(),
            KeyCode::Char('w') if ctrl => {
                let trimmed = input.trim_end().len();
                let cut = input[..trimmed].rfind([' ', '/']).map_or(0, |i| i + 1);
                input.truncate(cut);
            }
            KeyCode::Backspace => {
                input.pop();
            }
            KeyCode::Char(c) => input.push(c),
            KeyCode::Enter => {
                let (kind, input) = (kind.clone(), input.trim().to_string());
                self.mode = Mode::Normal;
                if !input.is_empty() {
                    self.submit_prompt(kind, input);
                }
            }
            _ => {}
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

    // ── actions ────────────────────────────────────────────────────────────

    fn enter_dir(&mut self) {
        if let Some(e) = self.browser.selected().filter(|e| e.is_dir) {
            let path = e.path.clone();
            self.browser.cd(path);
            self.invalidate();
        }
    }

    fn open_selected(&mut self) {
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

    fn edit_selected(&mut self) {
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
        });
    }

    fn edit_config(&mut self) {
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
        });
    }

    fn open_vscode(&mut self, folder: bool) {
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

    fn open_shell(&mut self) {
        let shell = env::var("SHELL").unwrap_or_else(|_| "sh".into());
        self.pending = Some(Foreground {
            cmd: quote_str(&shell),
            cwd: self.browser.cwd.clone(),
            wait: false,
            then_reload_config: false,
        });
    }

    fn run_op(&mut self, op: Op, interactive: bool) {
        match project::plan(op, self.browser.selected(), &self.browser.cwd, &self.config) {
            Ok(plan) if interactive => self.foreground(plan, true),
            Ok(plan) => self.start_task(plan.cmd.clone(), plan),
            Err(msg) => self.set_status(msg, Level::Err),
        }
    }

    fn foreground(&mut self, plan: Plan, wait: bool) {
        self.pending = Some(Foreground { cmd: plan.cmd, cwd: plan.cwd, wait, then_reload_config: false });
    }

    fn custom_action(&mut self, action: &Action) {
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

    fn start_task(&mut self, label: String, plan: Plan) {
        if self.task_running() {
            self.set_status("a task is already running — x to kill it", Level::Err);
            return;
        }
        self.output.clear();
        self.output.push(Line::from(vec![
            Span::styled("$ ", Style::new().fg(Color::DarkGray)),
            Span::styled(plan.cmd.replace(GIT, "git"), Style::new().add_modifier(Modifier::BOLD)),
            Span::styled(format!("   in {}", config::pretty_path(&plan.cwd)), Style::new().fg(Color::DarkGray)),
        ]));
        match Task::spawn(label, &plan.cmd, &plan.cwd) {
            Ok(task) => {
                self.task = Some(task);
                self.last_run = None;
                self.show_output = true;
            }
            Err(e) => self.set_status(format!("failed to start: {e}"), Level::Err),
        }
    }

    fn kill_task(&mut self) {
        match &mut self.task {
            Some(task) => {
                task.kill();
                self.set_status("killed", Level::Info);
            }
            None => self.set_status("nothing running", Level::Info),
        }
    }

    fn spawn_detached(&mut self, cmd: &str) {
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
    pub fn after_foreground(&mut self, fg: &Foreground) {
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

    // ── git ────────────────────────────────────────────────────────────────

    fn key_git(&mut self, key: KeyEvent) {
        self.mode = Mode::Normal;
        let KeyCode::Char(c) = key.code else { return };
        if self.git.is_none() && !matches!(c, 'i' | 'g' | 'h') {
            self.set_status("not a git repository — g i to create one", Level::Err);
            return;
        }
        let cwd = self.browser.cwd.clone();
        let sel = self.browser.selected().map(|e| quote(&e.path));
        let needs_sel = matches!(c, 'L' | 'w' | 'a' | 'u' | 'x');
        let Some(f) = sel.or_else(|| (!needs_sel).then(String::new)) else {
            self.set_status("select a file first", Level::Err);
            return;
        };

        let cmd = match c {
            's' => format!("{GIT} status"),
            'd' => format!("{GIT} diff"),
            'D' => format!("{GIT} diff --staged"),
            'l' => format!("{GIT} log --graph --oneline --decorate --all -n 500"),
            'L' => format!("{GIT} log --follow --stat -n 100 -- {f}"),
            'w' => format!("{GIT} blame --date=short -- {f}"),
            'a' => format!("{GIT} add -- {f} && {GIT} status --short"),
            'A' => format!("{GIT} add -A && {GIT} status --short"),
            'u' => format!("{GIT} restore --staged -- {f} && {GIT} status --short"),
            'f' => format!("{GIT} fetch --all --prune"),
            'p' => format!("{GIT} pull"),
            'P' if self.git.as_ref().is_some_and(|g| !g.upstream) => format!("{GIT} push -u origin HEAD"),
            'P' => format!("{GIT} push"),
            'b' => format!("{GIT} branch -a -vv"),
            'z' => format!("{GIT} stash push -u"),
            'Z' => format!("{GIT} stash pop"),
            'i' => format!("{GIT} init"),
            'c' => return self.prompt(PromptKind::Commit { all: false }, ""),
            'C' => return self.prompt(PromptKind::Commit { all: true }, ""),
            'o' => return self.prompt(PromptKind::SwitchBranch, ""),
            'n' => return self.prompt(PromptKind::NewBranch, ""),
            'e' => {
                let cmd = format!("GIT_EDITOR={} git commit --amend", quote_str(&self.config.editor));
                return self.foreground(Plan { cmd, cwd }, true);
            }
            'x' => {
                let name = self.browser.selected().map(|e| e.name.clone()).unwrap_or_default();
                self.mode = Mode::Confirm {
                    msg: format!("Discard all uncommitted changes to {name}? [y/N]"),
                    action: Confirmed::Task(Plan { cmd: format!("{GIT} restore -- {f}"), cwd }),
                };
                return;
            }
            'h' => {
                let source = match &self.git {
                    Some(g) => g.root.clone(),
                    None => self.project.as_ref().map_or_else(|| cwd.clone(), |p| p.root.clone()),
                };
                match git::gh_repo_form(&source) {
                    Ok(form) => self.mode = Mode::Form(form),
                    Err(e) => self.set_status(e, Level::Err),
                }
                return;
            }
            'g' => {
                let Some(tool) = ["lazygit", "gitui", "tig"].into_iter().find(|t| project::which(t).is_some()) else {
                    self.set_status("none of lazygit, gitui or tig is installed", Level::Err);
                    return;
                };
                return self.foreground(Plan { cmd: tool.into(), cwd }, false);
            }
            _ => return,
        };
        self.start_task(cmd.replace(GIT, "git"), Plan { cmd, cwd });
    }

    fn key_form(&mut self, key: KeyEvent) {
        let Mode::Form(form) = &mut self.mode else { return };
        match form.key(key) {
            FormEvent::None => {}
            FormEvent::Cancel => {
                self.mode = Mode::Normal;
                self.set_status("cancelled", Level::Info);
            }
            FormEvent::Submit => {
                let Mode::Form(form) = std::mem::replace(&mut self.mode, Mode::Normal) else { return };
                match form.kind {
                    FormKind::GhRepoCreate => {
                        let source = config::expand_tilde(form.get("source"));
                        if !source.is_dir() {
                            self.set_status(format!("{} is not a directory", source.display()), Level::Err);
                            self.mode = Mode::Form(form);
                            return;
                        }
                        let cmd = git::gh_repo_create_cmd(&form);
                        self.start_task("gh repo create".into(), Plan { cmd, cwd: source });
                    }
                }
            }
        }
    }

    // ── install ────────────────────────────────────────────────────────────

    fn open_install_picker(&mut self) {
        let start = match self.browser.selected() {
            Some(e) if e.is_dir => e.path.clone(),
            _ => self.browser.cwd.clone(),
        };
        let project = project::detect(&start);
        let root = project.as_ref().map_or_else(|| self.browser.cwd.clone(), |p| p.root.clone());
        let cargo = project.as_ref().is_some_and(|p| p.kind == Kind::Cargo);
        let items = install::scan(&root, &self.config.install_dir());
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
        self.mode = Mode::Install(InstallPicker {
            root,
            cargo,
            items,
            state: ListState::default().with_selected(Some(pos)),
        });
    }

    fn key_install(&mut self, key: KeyEvent) {
        let Mode::Install(picker) = &mut self.mode else { return };
        let len = picker.len();
        let cur = picker.state.selected().unwrap_or(0);
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.mode = Mode::Normal,
            KeyCode::Char('j') | KeyCode::Down => picker.state.select(Some((cur + 1).min(len.saturating_sub(1)))),
            KeyCode::Char('k') | KeyCode::Up => picker.state.select(Some(cur.saturating_sub(1))),
            KeyCode::Enter | KeyCode::Char('i') => {
                let root = picker.root.clone();
                let chosen = picker.selected().cloned();
                self.mode = Mode::Normal;
                match chosen {
                    None => self.start_task("cargo install".into(), Plan { cmd: "cargo install --path .".into(), cwd: root }),
                    Some(c) => self.install_binary(&c, &root),
                }
            }
            KeyCode::Char('r') => {
                if let Some(c) = picker.selected().cloned() {
                    self.mode = Mode::Normal;
                    let plan = Plan { cmd: quote(&c.path), cwd: picker_cwd(&c) };
                    self.start_task(c.name.clone(), plan);
                }
            }
            _ => {}
        }
    }

    fn install_binary(&mut self, c: &Candidate, root: &Path) {
        let dir = self.config.install_dir();
        let dest = dir.join(&c.name);
        let dir_s = dir.to_string_lossy();
        let cmd = format!(
            "install -Dm755 -- {src} {dst} && echo 'installed → {pretty}' && \
             case \":$PATH:\" in *:{d}:*) ;; *) echo 'note: {pretty_dir} is not on your PATH' ;; esac",
            src = quote(&c.path),
            dst = quote(&dest),
            pretty = config::pretty_path(&dest),
            d = quote_str(&dir_s),
            pretty_dir = config::pretty_path(&dir),
        );
        self.start_task(format!("install {}", c.name), Plan { cmd, cwd: root.to_path_buf() });
    }

    // ── file operations ────────────────────────────────────────────────────

    fn resolve(&self, input: &str) -> PathBuf {
        let p = config::expand_tilde(input);
        if p.is_absolute() { p } else { self.browser.cwd.join(p) }
    }

    fn submit_prompt(&mut self, kind: PromptKind, input: String) {
        let cwd = self.browser.cwd.clone();
        match kind {
            PromptKind::Command => self.start_task(input.clone(), Plan { cmd: input, cwd }),
            PromptKind::Commit { all } => {
                let add = if all { format!("{GIT} add -A && ") } else { String::new() };
                let cmd = format!("{add}{GIT} commit -m {}", quote_str(&input));
                self.start_task("git commit".into(), Plan { cmd, cwd });
            }
            PromptKind::SwitchBranch => {
                let cmd = format!("{GIT} switch {}", quote_str(&input));
                self.start_task("git switch".into(), Plan { cmd, cwd });
            }
            PromptKind::NewBranch => {
                let cmd = format!("{GIT} switch -c {}", quote_str(&input));
                self.start_task("git switch -c".into(), Plan { cmd, cwd });
            }
            PromptKind::NewFile => {
                let path = self.resolve(&input);
                let res = path
                    .parent()
                    .map_or(Ok(()), fs::create_dir_all)
                    .and_then(|_| fs::OpenOptions::new().write(true).create_new(true).open(&path).map(drop));
                self.after_create(&path, res);
            }
            PromptKind::NewDir => {
                let path = self.resolve(&input);
                let res = fs::create_dir_all(&path);
                self.after_create(&path, res);
            }
            PromptKind::Rename(from) => {
                let mut to = self.resolve(&input);
                if to.is_dir() && !from.is_dir()
                    && let Some(name) = from.file_name() {
                        to = to.join(name);
                    }
                match move_path(&from, &to) {
                    Ok(()) => {
                        self.set_status(format!("→ {}", config::pretty_path(&to)), Level::Ok);
                        self.browser.reload();
                        if let Some(name) = to.file_name().filter(|_| to.parent() == Some(&self.browser.cwd)) {
                            self.browser.select_name(&name.to_string_lossy());
                        }
                    }
                    Err(e) => self.set_status(format!("rename failed: {e}"), Level::Err),
                }
                self.invalidate();
            }
            PromptKind::Cd => {
                let path = self.resolve(&input);
                if path.is_dir() {
                    self.browser.cd(path.canonicalize().unwrap_or(path));
                } else if path.exists() {
                    let parent = path.parent().unwrap_or(Path::new("/")).to_path_buf();
                    self.browser.cd(parent);
                    if let Some(name) = path.file_name() {
                        self.browser.select_name(&name.to_string_lossy());
                    }
                } else {
                    self.set_status(format!("no such path: {}", path.display()), Level::Err);
                }
                self.invalidate();
            }
        }
    }

    fn after_create(&mut self, path: &Path, res: std::io::Result<()>) {
        match res {
            Ok(()) => {
                self.set_status(format!("created {}", config::pretty_path(path)), Level::Ok);
                self.browser.reload();
                // Select the top-level component that appeared in this dir.
                if let Ok(rel) = path.strip_prefix(&self.browser.cwd)
                    && let Some(first) = rel.components().next() {
                        self.browser.select_name(&first.as_os_str().to_string_lossy());
                    }
            }
            Err(e) => self.set_status(format!("create failed: {e}"), Level::Err),
        }
        self.invalidate();
    }

    fn confirm_delete(&mut self) {
        let Some(e) = self.browser.selected() else { return };
        let verb = if trash_cmd().is_some() { "Trash" } else { "Permanently delete" };
        let what = if e.is_dir { format!("{}/ and everything in it", e.name) } else { e.name.clone() };
        self.mode = Mode::Confirm { msg: format!("{verb} {what}? [y/N]"), action: Confirmed::Delete(e.path.clone()) };
    }

    fn delete(&mut self, path: &Path) {
        let res = match trash_cmd() {
            Some(cmd) => run_quiet(&format!("{cmd} {}", quote(path))),
            None if path.is_dir() && !path.is_symlink() => fs::remove_dir_all(path),
            None => fs::remove_file(path),
        };
        match res {
            Ok(()) => self.set_status(format!("deleted {}", config::pretty_path(path)), Level::Ok),
            Err(e) => self.set_status(format!("delete failed: {e}"), Level::Err),
        }
        self.browser.reload();
        self.invalidate();
    }

    fn paste(&mut self, mv: bool) {
        let Some(src) = self.yank.clone() else {
            self.set_status("nothing yanked — y on a file first", Level::Info);
            return;
        };
        let Some(name) = src.file_name() else { return };
        let mut dst = self.browser.cwd.join(name);
        if mv && dst == src {
            self.set_status("already here", Level::Info);
            return;
        }
        if dst.exists() {
            if mv {
                self.set_status(format!("{} already exists here", name.to_string_lossy()), Level::Err);
                return;
            }
            dst = unique_name(&dst);
        }
        let res = if mv {
            move_path(&src, &dst)
        } else {
            run_quiet(&format!("cp -a -- {} {}", quote(&src), quote(&dst)))
        };
        match res {
            Ok(()) => {
                if mv {
                    self.yank = None;
                }
                let verb = if mv { "moved" } else { "copied" };
                self.set_status(format!("{verb} → {}", config::pretty_path(&dst)), Level::Ok);
                self.browser.reload();
                if let Some(n) = dst.file_name() {
                    self.browser.select_name(&n.to_string_lossy());
                }
            }
            Err(e) => self.set_status(format!("paste failed: {e}"), Level::Err),
        }
        self.invalidate();
    }
}

fn picker_cwd(c: &Candidate) -> PathBuf {
    c.path.parent().unwrap_or(Path::new("/")).to_path_buf()
}

fn move_path(from: &Path, to: &Path) -> std::io::Result<()> {
    if to.exists() {
        return Err(std::io::Error::new(ErrorKind::AlreadyExists, "target already exists"));
    }
    match fs::rename(from, to) {
        Err(e) if e.kind() == ErrorKind::CrossesDevices => {
            run_quiet(&format!("mv -n -- {} {}", quote(from), quote(to)))
        }
        other => other,
    }
}

/// `foo.txt` → `foo copy.txt`, `foo copy 2.txt`, …
fn unique_name(path: &Path) -> PathBuf {
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let ext = path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    (1..)
        .map(|n| {
            let suffix = if n == 1 { " copy".to_string() } else { format!(" copy {n}") };
            path.with_file_name(format!("{stem}{suffix}{ext}"))
        })
        .find(|p| !p.exists())
        .unwrap()
}

fn trash_cmd() -> Option<&'static str> {
    if project::which("gio").is_some() {
        Some("gio trash --")
    } else if project::which("trash-put").is_some() {
        Some("trash-put --")
    } else {
        None
    }
}

fn run_quiet(cmd: &str) -> std::io::Result<()> {
    let out = Command::new("sh").arg("-c").arg(cmd).stdin(Stdio::null()).output()?;
    if out.status.success() {
        Ok(())
    } else {
        let msg = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(std::io::Error::other(if msg.is_empty() { format!("{cmd} failed") } else { msg }))
    }
}
