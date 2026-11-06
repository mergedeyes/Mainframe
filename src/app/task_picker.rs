//! The `T` picker: every task the project defines, filtered as you type.

use std::path::PathBuf;

use ratatui::{
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    widgets::ListState,
};

use super::{App, Level, Mode};
use crate::{
    config,
    line::{Edit, LineEdit},
    project::{self, Plan},
    targets::{self, Target},
};

pub struct TaskPicker {
    pub title: String,
    pub root: PathBuf,
    pub items: Vec<Target>,
    pub filter: LineEdit,
    /// Indices into `items` that match the filter.
    pub visible: Vec<usize>,
    pub state: ListState,
}

impl TaskPicker {
    fn apply_filter(&mut self) {
        let words: Vec<String> = self.filter.text.split_whitespace().map(str::to_lowercase).collect();
        self.visible = (0..self.items.len())
            .filter(|&i| {
                let t = &self.items[i];
                let hay = format!("{} {} {}", t.group, t.name, t.cmd).to_lowercase();
                words.iter().all(|w| hay.contains(w))
            })
            .collect();
        self.state.select((!self.visible.is_empty()).then_some(0));
    }

    pub fn selected(&self) -> Option<&Target> {
        self.visible.get(self.state.selected()?).map(|&i| &self.items[i])
    }
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
        let mut picker = TaskPicker {
            title: format!("{} · {}", p.kind.label(), p.name),
            root: p.root,
            items,
            filter: LineEdit::default(),
            visible: Vec::new(),
            state: ListState::default(),
        };
        picker.apply_filter();
        self.mode = Mode::Tasks(picker);
    }

    pub(super) fn key_task_picker(&mut self, key: KeyEvent) {
        let Mode::Tasks(picker) = &mut self.mode else { return };
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let len = picker.visible.len();
        let cur = picker.state.selected().unwrap_or(0);
        match key.code {
            KeyCode::Esc => self.mode = Mode::Normal,
            KeyCode::Char('c') if ctrl => self.mode = Mode::Normal,
            KeyCode::Down | KeyCode::Tab => picker.state.select(Some((cur + 1).min(len.saturating_sub(1)))),
            KeyCode::Char('n') if ctrl => picker.state.select(Some((cur + 1).min(len.saturating_sub(1)))),
            KeyCode::Up | KeyCode::BackTab => picker.state.select(Some(cur.saturating_sub(1))),
            KeyCode::Char('p') if ctrl => picker.state.select(Some(cur.saturating_sub(1))),
            // Enter runs in the output pane, ^R in the full terminal (like r / R).
            KeyCode::Enter => self.run_picked(false),
            KeyCode::Char('r') if ctrl => self.run_picked(true),
            _ => {
                if picker.filter.key(key) == Edit::Changed {
                    picker.apply_filter();
                }
            }
        }
    }

    fn run_picked(&mut self, interactive: bool) {
        let Mode::Tasks(picker) = std::mem::replace(&mut self.mode, Mode::Normal) else { return };
        let Some(t) = picker.selected() else { return };
        let plan = Plan { cmd: t.cmd.clone(), cwd: picker.root.clone() };
        if interactive {
            self.foreground(plan, true);
        } else {
            self.start_task(t.cmd.clone(), plan);
        }
    }
}
