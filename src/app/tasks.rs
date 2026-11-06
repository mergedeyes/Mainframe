//! Output tabs: each holds one background task (running or finished) and what it printed.

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

use super::{App, Focus, Level};
use crate::{
    config,
    git::GIT,
    project::{self, Plan},
    task::{Output, Task},
    watch::Watcher,
};

/// Beyond this, a new task replaces the first idle tab.
const MAX_TABS: usize = 9;

pub struct LastRun {
    pub label: String,
    pub ok: bool,
    pub detail: String,
}

#[derive(Default)]
pub struct Tab {
    pub label: String,
    pub output: Output,
    pub task: Option<Task>,
    pub last_run: Option<LastRun>,
    /// What ran here, for rerunning it.
    pub plan: Option<Plan>,
    /// Watch mode: reruns `plan` whenever a file under the project changes.
    pub watch: Option<Watcher>,
}

impl Tab {
    pub fn running(&self) -> bool {
        self.task.is_some()
    }

    /// Running or watching: either way, new tasks shouldn't take this tab over.
    fn busy(&self) -> bool {
        self.running() || self.watch.is_some()
    }
}

impl App {
    /// The tab shown in the output pane.
    pub fn tab(&self) -> &Tab {
        &self.tabs[self.tab]
    }

    pub fn tab_mut(&mut self) -> &mut Tab {
        &mut self.tabs[self.tab]
    }

    pub fn task_running(&self) -> bool {
        self.tabs.iter().any(Tab::running)
    }

    pub fn kill_all(&mut self) {
        for task in self.tabs.iter_mut().filter_map(|t| t.task.as_mut()) {
            task.kill();
        }
    }

    /// Moves new output into every tab and wraps up tasks that finished.
    pub(super) fn poll_tasks(&mut self) {
        let mut done = None;
        for tab in &mut self.tabs {
            let Some(task) = &mut tab.task else { continue };
            let Some((status, took)) = task.poll(&mut tab.output) else { continue };
            tab.task = None;
            let ok = status.success();
            let code = match status.code() {
                Some(c) => format!("exit {c}"),
                None => "killed".into(),
            };
            let detail = format!("{code} · {:.1}s", took.as_secs_f32());
            let style = if ok { Color::Green } else { Color::Red };
            tab.output.push(Line::from(Span::styled(
                format!("── {} {}: {detail}", if ok { "✔" } else { "✘" }, tab.label),
                Style::new().fg(style).add_modifier(Modifier::BOLD),
            )));
            done = Some((format!("{}: {detail}", tab.label), if ok { Level::Ok } else { Level::Err }));
            tab.last_run = Some(LastRun { label: tab.label.clone(), ok, detail });
        }
        if let Some((msg, level)) = done {
            self.set_status(msg, level);
            // Builds create files and git commands change status; show both.
            self.browser.reload();
            self.invalidate();
        }
    }

    /// Where a new task goes: the shown tab if it's idle, else an idle tab that ran
    /// the same thing, else a new tab (`tabs.len()`), else the first idle one.
    fn tab_for(&self, label: &str) -> Option<usize> {
        if !self.tab().busy() {
            return Some(self.tab);
        }
        self.tabs
            .iter()
            .position(|t| !t.running() && t.label == label)
            .or_else(|| (self.tabs.len() < MAX_TABS).then_some(self.tabs.len()))
            .or_else(|| self.tabs.iter().position(|t| !t.busy()))
    }

    pub(super) fn start_task(&mut self, label: String, plan: Plan) {
        let Some(i) = self.tab_for(&label) else {
            self.set_status(format!("{MAX_TABS} tasks are running — x kills the shown one"), Level::Err);
            return;
        };
        if self.run_in(i, label, plan) {
            self.tab = i;
            self.show_output = true;
        }
    }

    /// Starts `plan` in tab `i` (a new one when `i == tabs.len()`). Whatever ran there
    /// before is killed; a watcher stays.
    fn run_in(&mut self, i: usize, label: String, plan: Plan) -> bool {
        let mut output = Output::new(&plan.cwd);
        output.push(Line::from(vec![
            Span::styled("$ ", Style::new().fg(Color::DarkGray)),
            Span::styled(plan.cmd.replace(GIT, "git"), Style::new().add_modifier(Modifier::BOLD)),
            Span::styled(format!("   in {}", config::pretty_path(&plan.cwd)), Style::new().fg(Color::DarkGray)),
        ]));
        match Task::spawn(label.clone(), &plan.cmd, &plan.cwd) {
            Ok(task) => {
                let watch = self.tabs.get_mut(i).and_then(|t| t.watch.take());
                let tab = Tab { label, output, task: Some(task), last_run: None, plan: Some(plan), watch };
                if i == self.tabs.len() {
                    self.tabs.push(tab);
                } else {
                    self.tabs[i] = tab;
                }
                true
            }
            Err(e) => {
                self.set_status(format!("failed to start: {e}"), Level::Err);
                false
            }
        }
    }

    /// Runs the shown tab's command again, restarting it if it's still going.
    pub(super) fn rerun(&mut self) {
        let tab = self.tab();
        let Some(plan) = tab.plan.clone() else {
            return self.set_status("nothing ran here yet", Level::Info);
        };
        let label = tab.label.clone();
        if self.run_in(self.tab, label, plan) {
            self.show_output = true;
        }
    }

    /// Turns watch mode on or off for the shown tab.
    pub(super) fn toggle_watch(&mut self) {
        let tab = &mut self.tabs[self.tab];
        if tab.watch.take().is_some() {
            let msg = format!("stopped watching for {}", tab.label);
            return self.set_status(msg, Level::Info);
        }
        let Some(plan) = &tab.plan else {
            return self.set_status("run something first — W then reruns it on every file change", Level::Info);
        };
        let root = project::detect(&plan.cwd).map_or_else(|| plan.cwd.clone(), |p| p.root);
        let msg = format!("watching {} — {} reruns on every change · W stops", config::pretty_path(&root), tab.label);
        tab.watch = Some(Watcher::new(root));
        self.show_output = true;
        self.set_status(msg, Level::Ok);
    }

    /// Reruns watched tasks whose files changed.
    pub(super) fn poll_watchers(&mut self) {
        for i in 0..self.tabs.len() {
            let tab = &self.tabs[i];
            if tab.watch.as_ref().is_some_and(Watcher::changed)
                && let Some(plan) = tab.plan.clone()
            {
                let label = tab.label.clone();
                self.run_in(i, label, plan);
            }
        }
    }

    /// Kills the shown tab's task, or closes the tab when it has finished.
    pub(super) fn kill_task(&mut self) {
        let n = self.tabs.len();
        match &mut self.tabs[self.tab].task {
            Some(task) => {
                task.kill();
                self.set_status("killed", Level::Info);
            }
            None if n > 1 => {
                let tab = self.tabs.remove(self.tab);
                self.tab = self.tab.min(self.tabs.len() - 1);
                self.set_status(format!("closed {}", tab.label), Level::Info);
            }
            None => self.set_status("nothing running", Level::Info),
        }
    }

    /// n / N: picks the next or previous `file:line` in the shown output.
    pub(super) fn step_loc(&mut self, forward: bool) {
        let out = &mut self.tabs[self.tab].output;
        if !out.search.is_empty() {
            if out.step_match(forward) {
                let msg = format!("match {}/{}", out.match_cursor.unwrap_or(0) + 1, out.matches.len());
                self.show_output = true;
                self.focus = Focus::Output;
                self.set_status(msg, Level::Info);
            } else {
                let msg = format!("no match for {}", out.search);
                self.set_status(msg, Level::Info);
            }
            return;
        }
        let total = out.locs.len();
        if !out.step_loc(forward) {
            self.set_status("no file:line locations in this output", Level::Info);
            return;
        }
        let Some((_, loc)) = out.selected_loc() else { return };
        let path = match loc.path.strip_prefix(&self.browser.cwd) {
            Ok(rel) => rel.display().to_string(),
            Err(_) => config::pretty_path(&loc.path),
        };
        let msg = format!(
            "{}/{total}  {path}:{} — Enter edits · o opens in VS Code",
            out.cursor.unwrap_or(0) + 1,
            loc.line,
        );
        self.show_output = true;
        self.focus = Focus::Output;
        self.set_status(msg, Level::Info);
    }

    pub(super) fn switch_tab(&mut self, delta: isize) {
        let n = self.tabs.len() as isize;
        if n < 2 {
            self.set_status("only one output tab", Level::Info);
            return;
        }
        self.tab = (self.tab as isize + delta).rem_euclid(n) as usize;
        self.show_output = true;
    }
}
