//! The `g o` picker: every branch with when its work started and when it last changed,
//! numbered in the order to merge them.

use ratatui::{
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    widgets::ListState,
};

use super::{App, Level, Mode};
use crate::{
    git::{self, Branch, GIT},
    line::{Edit, LineEdit},
    project::{Plan, quote_str},
};

pub struct BranchPicker {
    /// What the numbers are measured against, e.g. `origin/main`.
    pub base: Option<String>,
    pub items: Vec<Branch>,
    pub filter: LineEdit,
    /// Indices into `items` that match the filter.
    pub visible: Vec<usize>,
    pub state: ListState,
}

impl BranchPicker {
    fn apply_filter(&mut self) {
        let words: Vec<String> = self.filter.text.split_whitespace().map(str::to_lowercase).collect();
        self.visible = (0..self.items.len())
            .filter(|&i| {
                let name = self.items[i].name.to_lowercase();
                words.iter().all(|w| name.contains(w))
            })
            .collect();
        self.state.select((!self.visible.is_empty()).then_some(0));
    }

    fn selected(&self) -> Option<&Branch> {
        self.visible.get(self.state.selected()?).map(|&i| &self.items[i])
    }
}

impl App {
    pub(super) fn open_branch_picker(&mut self) {
        let cwd = self.browser.cwd.clone();
        let (base, items) = git::branch_list(&cwd);
        if items.is_empty() {
            return self.set_status("no branches here", Level::Info);
        }
        let mut picker =
            BranchPicker { base, items, filter: LineEdit::default(), visible: Vec::new(), state: ListState::default() };
        picker.apply_filter();
        self.mode = Mode::Branches(picker);
    }

    pub(super) fn key_branch_picker(&mut self, key: KeyEvent) {
        let Mode::Branches(picker) = &mut self.mode else { return };
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
            KeyCode::Enter => self.switch_picked(),
            _ => {
                if picker.filter.key(key) == Edit::Changed {
                    picker.apply_filter();
                }
            }
        }
    }

    /// Switches to the selected branch, or to what was typed when nothing matches it.
    fn switch_picked(&mut self) {
        let Mode::Branches(picker) = std::mem::replace(&mut self.mode, Mode::Normal) else { return };
        let name = match picker.selected() {
            Some(b) if b.current => return self.set_status(format!("already on {}", b.name), Level::Info),
            Some(b) => b.name.clone(),
            None if !picker.filter.text.trim().is_empty() => picker.filter.text.trim().to_string(),
            None => return,
        };
        let cmd = format!("{GIT} switch {}", quote_str(&name));
        self.start_task("git switch".into(), Plan { cmd, cwd: self.browser.cwd.clone() });
    }
}
