//! The `g o` picker: every branch with when its work started and when it last changed,
//! numbered in the order to merge them.

use ratatui::crossterm::event::{KeyCode, KeyEvent};

use super::{App, Level, Mode};
use crate::{
    cmd::Step,
    filter_list::{self, FilterList, Match},
    git::{self, Branch},
    project::Plan,
};

pub struct BranchPicker {
    /// What the numbers are measured against, e.g. `origin/main`.
    pub base: Option<String>,
    pub list: FilterList<Branch>,
}

impl App {
    pub(super) fn open_branch_picker(&mut self) {
        let (base, items) = git::branch_list(&self.browser.cwd);
        if items.is_empty() {
            return self.set_status("no branches here", Level::Info);
        }
        self.mode =
            Mode::Branches(BranchPicker { base, list: FilterList::new(items, |b| b.name.clone(), Match::Words) });
    }

    pub(super) fn key_branch_picker(&mut self, key: KeyEvent) {
        let Mode::Branches(picker) = &mut self.mode else { return };
        match key.code {
            KeyCode::Enter => self.switch_picked(),
            _ => {
                if picker.list.key(key) == filter_list::Key::Close {
                    self.mode = Mode::Normal;
                }
            }
        }
    }

    /// Switches to the selected branch, or to what was typed when nothing matches it.
    fn switch_picked(&mut self) {
        let Mode::Branches(picker) = std::mem::replace(&mut self.mode, Mode::Normal) else { return };
        let typed = picker.list.filter.text.trim();
        let name = match picker.list.selected() {
            Some(b) if b.current => return self.set_status(format!("already on {}", b.name), Level::Info),
            Some(b) => b.name.clone(),
            None if !typed.is_empty() => typed.to_string(),
            None => return,
        };
        let cmd = Step::git().args(["switch", &name]).into();
        self.start_task("git switch".into(), Plan { cmd, cwd: self.browser.cwd.clone() });
    }
}
