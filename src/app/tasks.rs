//! Output tabs: each holds one background task (running or finished) and what it printed.

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

use super::{App, Level};
use crate::{
    config,
    git::GIT,
    project::Plan,
    task::{Output, Task},
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
}

impl Tab {
    pub fn running(&self) -> bool {
        self.task.is_some()
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
        if !self.tab().running() {
            return Some(self.tab);
        }
        self.tabs
            .iter()
            .position(|t| !t.running() && t.label == label)
            .or_else(|| (self.tabs.len() < MAX_TABS).then_some(self.tabs.len()))
            .or_else(|| self.tabs.iter().position(|t| !t.running()))
    }

    pub(super) fn start_task(&mut self, label: String, plan: Plan) {
        let Some(i) = self.tab_for(&label) else {
            self.set_status(format!("{MAX_TABS} tasks are running — x kills the shown one"), Level::Err);
            return;
        };
        let mut output = Output::default();
        output.push(Line::from(vec![
            Span::styled("$ ", Style::new().fg(Color::DarkGray)),
            Span::styled(plan.cmd.replace(GIT, "git"), Style::new().add_modifier(Modifier::BOLD)),
            Span::styled(format!("   in {}", config::pretty_path(&plan.cwd)), Style::new().fg(Color::DarkGray)),
        ]));
        match Task::spawn(label.clone(), &plan.cmd, &plan.cwd) {
            Ok(task) => {
                let tab = Tab { label, output, task: Some(task), last_run: None };
                if i == self.tabs.len() {
                    self.tabs.push(tab);
                } else {
                    self.tabs[i] = tab;
                }
                self.tab = i;
                self.show_output = true;
            }
            Err(e) => self.set_status(format!("failed to start: {e}"), Level::Err),
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
