//! `J`: every project in the projects folder, most recently active first; jump to one.

use std::path::PathBuf;

use ratatui::{
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    widgets::ListState,
};

use super::{App, Level, Mode};
use crate::{
    config, fuzzy,
    line::{Edit, LineEdit},
    notes,
    projects::{self, Entry},
};

pub struct ProjectPicker {
    pub root: PathBuf,
    pub items: Vec<Entry>,
    /// Open todos per item, same order as `items`.
    pub todos: Vec<usize>,
    /// Tasks running per item (its output tabs live on while you're elsewhere).
    pub running: Vec<usize>,
    /// The project you're in, marked in the list.
    pub current: PathBuf,
    pub filter: LineEdit,
    /// Indices into `items`, best match first (most recent first while unfiltered).
    pub visible: Vec<usize>,
    pub state: ListState,
}

impl ProjectPicker {
    fn apply_filter(&mut self) {
        let q = self.filter.text.replace(' ', "").to_lowercase();
        let mut scored: Vec<(i64, usize)> = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(i, e)| fuzzy::score(&q, &e.name.to_lowercase()).map(|s| (s, i)))
            .collect();
        if !q.is_empty() {
            scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        }
        self.visible = scored.into_iter().map(|(_, i)| i).collect();
        self.state.select((!self.visible.is_empty()).then_some(0));
    }

    fn selected(&self) -> Option<&Entry> {
        self.visible.get(self.state.selected()?).map(|&i| &self.items[i])
    }
}

impl App {
    pub(super) fn open_project_picker(&mut self) {
        let Some(root) = projects::dir(&self.config) else {
            let msg = if self.config.projects_dir.trim().is_empty() {
                "no projects folder found — set projects_dir in the config (,)".to_string()
            } else {
                format!("projects_dir {} doesn't exist", self.config.projects_dir.trim())
            };
            return self.set_status(msg, Level::Err);
        };
        let items = projects::scan(&root);
        if items.is_empty() {
            return self.set_status(format!("no projects in {}", config::pretty_path(&root)), Level::Info);
        }
        let todos = items.iter().map(|e| notes::open_count(&e.path)).collect();
        let running = items.iter().map(|e| self.running_in(&e.path)).collect();
        let mut picker = ProjectPicker {
            root,
            items,
            todos,
            running,
            current: self.notes_root.clone(),
            filter: LineEdit::default(),
            visible: Vec::new(),
            state: ListState::default(),
        };
        picker.apply_filter();
        self.mode = Mode::Projects(picker);
    }

    pub(super) fn key_project_picker(&mut self, key: KeyEvent) {
        let Mode::Projects(picker) = &mut self.mode else { return };
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let len = picker.visible.len();
        let cur = picker.state.selected().unwrap_or(0);
        let next = Some((cur + 1).min(len.saturating_sub(1)));
        match key.code {
            KeyCode::Esc => self.mode = Mode::Normal,
            KeyCode::Char('c') if ctrl => self.mode = Mode::Normal,
            KeyCode::Down | KeyCode::Tab => picker.state.select(next),
            KeyCode::Char('n') if ctrl => picker.state.select(next),
            KeyCode::Up | KeyCode::BackTab => picker.state.select(Some(cur.saturating_sub(1))),
            KeyCode::Char('p') if ctrl => picker.state.select(Some(cur.saturating_sub(1))),
            KeyCode::PageDown => picker.state.select(Some((cur + 10).min(len.saturating_sub(1)))),
            KeyCode::PageUp => picker.state.select(Some(cur.saturating_sub(10))),
            KeyCode::Enter => self.pick_project(Pick::Go),
            KeyCode::Char('t') if ctrl => self.pick_project(Pick::Notes),
            KeyCode::Char('o') if ctrl => self.pick_project(Pick::VsCode),
            _ => {
                if picker.filter.key(key) == Edit::Changed {
                    picker.apply_filter();
                }
            }
        }
    }

    fn pick_project(&mut self, pick: Pick) {
        let Mode::Projects(picker) = std::mem::replace(&mut self.mode, Mode::Normal) else { return };
        let Some(e) = picker.selected().cloned() else { return };
        match pick {
            Pick::Go => {
                self.browser.cd(e.path);
                self.invalidate();
            }
            Pick::Notes => self.open_notes(e.path, e.name),
            Pick::VsCode => {
                self.spawn_detached(&format!("{} {}", self.config.vscode, crate::project::quote(&e.path)));
                self.set_status(format!("opened {} in VS Code", e.name), Level::Ok);
            }
        }
    }
}

enum Pick {
    Go,
    Notes,
    VsCode,
}
