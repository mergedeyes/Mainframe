//! `C`: clears build output, installed dependencies and caches of the current project.

use std::{
    collections::BTreeSet,
    path::PathBuf,
    sync::mpsc::{self, Receiver},
    thread,
    time::Instant,
};

use ratatui::{
    crossterm::event::{KeyCode, KeyEvent},
    widgets::ListState,
};

use super::{App, Confirmed, Level, Mode};
use crate::{
    browser::human_size,
    config,
    cleanup::{self, Item},
    project::{self, Plan},
};

pub struct CleanupPicker {
    pub root: PathBuf,
    pub title: String,
    /// None while the project is still being measured.
    pub items: Option<Vec<Item>>,
    rx: Receiver<Vec<Item>>,
    pub started: Instant,
    pub marked: BTreeSet<usize>,
    pub state: ListState,
}

impl CleanupPicker {
    pub fn items(&self) -> &[Item] {
        self.items.as_deref().unwrap_or_default()
    }

    pub fn selected(&self) -> Option<&Item> {
        self.items().get(self.state.selected()?)
    }

    /// The marked rows, or the selected one when nothing is marked.
    pub fn chosen(&self) -> Vec<&Item> {
        let items = self.items();
        if self.marked.is_empty() {
            self.selected().into_iter().collect()
        } else {
            self.marked.iter().filter_map(|&i| items.get(i)).collect()
        }
    }

    pub fn measuring(&self) -> bool {
        self.items.is_none()
    }
}

impl App {
    pub(super) fn open_cleanup(&mut self) {
        let start = match self.browser.selected() {
            Some(e) if e.is_dir => e.path.clone(),
            _ => self.browser.cwd.clone(),
        };
        // Outside a project, everything below the folder: a group like Games/ or all projects.
        let (title, root) = match project::detect(&start) {
            Some(p) => (p.name, p.root),
            None if start == config::home() || start.parent().is_none() => {
                return self.set_status("no project here — C cleans a project or a folder of projects, not all of ~", Level::Err);
            }
            None => (config::pretty_path(&start), start),
        };
        let (tx, rx) = mpsc::channel();
        let scan = root.clone();
        thread::spawn(move || {
            let _ = tx.send(cleanup::scan(&scan));
        });
        self.mode = Mode::Cleanup(CleanupPicker {
            title,
            root,
            items: None,
            rx,
            started: Instant::now(),
            marked: BTreeSet::new(),
            state: ListState::default(),
        });
    }

    /// Takes the measurements once they're in; closes the picker when there's nothing.
    pub(super) fn poll_cleanup(&mut self) {
        let Mode::Cleanup(picker) = &mut self.mode else { return };
        if !picker.measuring() {
            return;
        }
        let Ok(items) = picker.rx.try_recv() else { return };
        if items.is_empty() {
            let msg = format!("nothing to clean in {}", picker.title);
            self.mode = Mode::Normal;
            return self.set_status(msg, Level::Info);
        }
        picker.items = Some(items);
        picker.state.select(Some(0));
    }

    pub(super) fn key_cleanup(&mut self, key: KeyEvent) {
        let Mode::Cleanup(picker) = &mut self.mode else { return };
        let len = picker.items().len();
        let cur = picker.state.selected().unwrap_or(0);
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.mode = Mode::Normal,
            KeyCode::Char('j') | KeyCode::Down => picker.state.select(Some((cur + 1).min(len.saturating_sub(1)))),
            KeyCode::Char('k') | KeyCode::Up => picker.state.select(Some(cur.saturating_sub(1))),
            KeyCode::Char(' ') if len > 0 => {
                if !picker.marked.remove(&cur) {
                    picker.marked.insert(cur);
                }
                picker.state.select(Some((cur + 1).min(len - 1)));
            }
            // Every row like this one: all debug builds of all projects below, say.
            KeyCode::Char('a') if len > 0 => {
                let items = picker.items();
                let label = items[cur].label.clone();
                let like: Vec<usize> = (0..len).filter(|&i| items[i].label == label).collect();
                if like.iter().all(|i| picker.marked.contains(i)) {
                    like.iter().for_each(|i| {
                        picker.marked.remove(i);
                    });
                } else {
                    picker.marked.extend(like);
                }
            }
            KeyCode::Enter | KeyCode::Char('C') => self.confirm_cleanup(),
            _ => {}
        }
    }

    fn confirm_cleanup(&mut self) {
        let Mode::Cleanup(picker) = &self.mode else { return };
        let chosen = picker.chosen();
        if chosen.is_empty() {
            return;
        }
        let root = picker.root.clone();
        let what = cleanup::summary(&chosen);
        // A size is only promised when every row was measured and none runs a tool.
        let measured = chosen.iter().all(|i| i.size.is_some() && i.cmd.is_none());
        let size = measured.then(|| cleanup::total(&chosen));
        let mut steps: Vec<String> = chosen.iter().map(|i| i.command(&root)).collect();
        steps.push(cleanup::done_line(&what, size));
        let plan = Plan { cmd: steps.join(" && "), cwd: root };
        let label = if chosen.len() == 1 { format!("clean {what}") } else { format!("clean {} things", chosen.len()) };
        let sized = match size {
            Some(n) => format!("{what} ({})", human_size(n)),
            None => what,
        };
        self.mode = Mode::Confirm { msg: format!("Delete {sized}? [y/N]"), action: Confirmed::Labeled(label, plan) };
    }
}
