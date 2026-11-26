//! The `T` picker: every task the project defines, filtered as you type.

use std::path::PathBuf;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{App, Level, Mode};
use crate::{
    config,
    filter_list::{self, FilterList, Match},
    project::{self, Plan},
    targets::{self, Target},
};

pub struct TaskPicker {
    pub title: String,
    pub root: PathBuf,
    pub list: FilterList<Target>,
}

impl App {
    pub(super) fn open_task_picker(&mut self) {
        let start = match self.browser.selected() {
            Some(e) if e.is_dir => e.path.clone(),
            _ => self.browser.cwd.clone(),
        };
        let Some(p) = project::detect(&start) else {
            return self.set_status("no project here", Level::Err);
        };
        let items = targets::list(&p);
        if items.is_empty() {
            return self.set_status(format!("{} defines no tasks", config::pretty_path(&p.root)), Level::Info);
        }
        self.mode = Mode::Tasks(TaskPicker {
            title: format!("{} · {}", p.kind.label(), p.name),
            root: p.root,
            list: FilterList::new(items, |t| format!("{} {} {}", t.group, t.name, t.cmd), Match::Words),
        });
    }

    pub(super) fn key_task_picker(&mut self, key: KeyEvent) {
        let Mode::Tasks(picker) = &mut self.mode else { return };
        match key.code {
            // Enter runs in the output pane, ^R in the full terminal (like r / R).
            KeyCode::Enter => self.run_picked(false),
            KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => self.run_picked(true),
            _ => {
                if picker.list.key(key) == filter_list::Key::Close {
                    self.mode = Mode::Normal;
                }
            }
        }
    }

    fn run_picked(&mut self, interactive: bool) {
        let Mode::Tasks(picker) = std::mem::replace(&mut self.mode, Mode::Normal) else { return };
        let Some(t) = picker.list.selected() else { return };
        let plan = Plan { cmd: t.cmd.clone(), cwd: picker.root.clone() };
        if interactive {
            self.foreground(plan, true);
        } else {
            self.start_task(t.cmd.clone(), plan);
        }
    }
}
