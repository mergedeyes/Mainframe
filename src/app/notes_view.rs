//! `M`: the current project's notes and todos. Every change is saved right away.

use std::{fs, path::PathBuf};

use ratatui::{
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    widgets::ListState,
};

use super::{App, Foreground, Level, Mode};
use crate::{
    config,
    line::LineEdit,
    notes::{Item, Notes},
    project::quote,
};

pub struct NotesView {
    pub root: PathBuf,
    pub title: String,
    pub notes: Notes,
    pub state: ListState,
    /// Inline editor for one line; `new` lines vanish again when cancelled or left empty.
    pub editing: Option<Editing>,
    /// The list before the last delete / clear, for `u`.
    undo: Option<Vec<Item>>,
}

pub struct Editing {
    pub index: usize,
    pub input: LineEdit,
    new: bool,
}

impl NotesView {
    fn selected(&self) -> Option<usize> {
        self.state.selected().filter(|&i| i < self.notes.items.len())
    }

    fn select(&mut self, i: usize) {
        let len = self.notes.items.len();
        self.state.select((len > 0).then(|| i.min(len - 1)));
    }

    /// Inserts `item` below the selection and starts editing it.
    fn insert(&mut self, item: Item) {
        let at = self.selected().map_or(self.notes.items.len(), |i| i + 1);
        self.notes.items.insert(at, item);
        self.select(at);
        self.editing = Some(Editing { index: at, input: LineEdit::default(), new: true });
    }

    fn snapshot(&mut self) {
        self.undo = Some(self.notes.items.clone());
    }
}

impl App {
    /// The notes of whatever project you're in (see `projects::owner`).
    pub(super) fn open_current_notes(&mut self) {
        let root = self.notes_root.clone();
        let title = config::pretty_path(&root);
        self.open_notes(root, title);
    }

    pub(super) fn open_notes(&mut self, root: PathBuf, title: String) {
        let notes = Notes::load(&root);
        // Start on the first open todo: that's usually what you came for.
        let first = notes.items.iter().position(Item::is_open).unwrap_or(0);
        let mut view = NotesView { root, title, notes, state: ListState::default(), editing: None, undo: None };
        view.select(first);
        self.mode = Mode::Notes(view);
    }

    pub(super) fn key_notes(&mut self, key: KeyEvent) {
        let Mode::Notes(view) = &mut self.mode else { return };
        if view.editing.is_some() {
            return self.key_notes_edit(key);
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            if key.code == KeyCode::Char('c') {
                self.close_notes();
            }
            return;
        }
        let len = view.notes.items.len();
        let cur = view.selected();
        let at = cur.unwrap_or(0);
        let mut changed = false;
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('M') => return self.close_notes(),
            KeyCode::Char('j') | KeyCode::Down => view.select(at + 1),
            KeyCode::Char('k') | KeyCode::Up => view.select(at.saturating_sub(1)),
            KeyCode::Home | KeyCode::Char('g') => view.select(0),
            KeyCode::End | KeyCode::Char('G') => view.select(len.saturating_sub(1)),
            KeyCode::Char('a') | KeyCode::Char('o') => view.insert(Item::todo("")),
            KeyCode::Char('n') => view.insert(Item::Note(String::new())),
            KeyCode::Enter | KeyCode::Char('e') | KeyCode::Char('i') => match cur {
                Some(i) => {
                    let input = LineEdit::new(view.notes.items[i].text());
                    view.editing = Some(Editing { index: i, input, new: false });
                }
                None => view.insert(Item::todo("")),
            },
            KeyCode::Char(' ') | KeyCode::Char('x') => {
                if let Some(Item::Todo { done, .. }) = cur.map(|i| &mut view.notes.items[i]) {
                    *done = !*done;
                    changed = true;
                    view.select(at + 1);
                }
            }
            // Turns a note line into a todo and back.
            KeyCode::Char('t') => {
                if let Some(i) = cur {
                    let item = &mut view.notes.items[i];
                    *item = match item {
                        Item::Note(text) => {
                            let text = text.trim_start_matches(['-', '*', ' ']).to_string();
                            Item::todo(text)
                        }
                        Item::Todo { text, .. } => Item::Note(format!("- {text}")),
                    };
                    changed = true;
                }
            }
            KeyCode::Char('d') | KeyCode::Delete => {
                if let Some(i) = cur {
                    view.snapshot();
                    view.notes.items.remove(i);
                    view.select(i);
                    changed = true;
                }
            }
            KeyCode::Char('C') => {
                if view.notes.done() > 0 {
                    view.snapshot();
                    view.notes.items.retain(|i| !matches!(i, Item::Todo { done: true, .. }));
                    view.select(at);
                    changed = true;
                }
            }
            KeyCode::Char('u') => {
                if let Some(items) = view.undo.take() {
                    view.notes.items = items;
                    view.select(at);
                    changed = true;
                }
            }
            KeyCode::Char('J') => {
                if let Some(i) = cur.filter(|&i| i + 1 < len) {
                    view.notes.items.swap(i, i + 1);
                    view.select(i + 1);
                    changed = true;
                }
            }
            KeyCode::Char('K') => {
                if let Some(i) = cur.filter(|&i| i > 0) {
                    view.notes.items.swap(i, i - 1);
                    view.select(i - 1);
                    changed = true;
                }
            }
            KeyCode::Char('E') => return self.edit_notes_file(),
            _ => {}
        }
        if changed {
            self.save_notes();
        }
    }

    fn key_notes_edit(&mut self, key: KeyEvent) {
        let Mode::Notes(view) = &mut self.mode else { return };
        let Some(ed) = view.editing.as_mut() else { return };
        match key.code {
            KeyCode::Esc => {
                if ed.new {
                    let i = ed.index;
                    view.notes.items.remove(i);
                    view.select(i.saturating_sub(1));
                }
                view.editing = None;
            }
            KeyCode::Enter => {
                let Some(ed) = view.editing.take() else { return };
                let text = ed.input.text.trim_end().to_string();
                let item = &mut view.notes.items[ed.index];
                // An emptied todo goes away; an empty note line is a blank line on purpose.
                if text.is_empty() && (ed.new || matches!(item, Item::Todo { .. })) {
                    if !ed.new {
                        view.snapshot();
                    }
                    view.notes.items.remove(ed.index);
                    view.select(ed.index.saturating_sub(usize::from(ed.new)));
                    if ed.new {
                        return;
                    }
                } else {
                    item.set_text(text);
                }
                self.save_notes();
            }
            _ => {
                ed.input.key(key);
            }
        }
    }

    fn save_notes(&mut self) {
        let Mode::Notes(view) = &self.mode else { return };
        if let Err(e) = view.notes.save() {
            let path = config::pretty_path(&view.notes.path);
            self.set_status(format!("can't save {path}: {e}"), Level::Err);
        }
    }

    fn close_notes(&mut self) {
        self.mode = Mode::Normal;
        self.refresh_todo_count();
    }

    /// `E`: the whole file in the editor, for longer notes; the view comes back afterwards.
    fn edit_notes_file(&mut self) {
        let Mode::Notes(view) = std::mem::replace(&mut self.mode, Mode::Normal) else { return };
        let path = view.notes.path.clone();
        if !path.exists() {
            let created = path
                .parent()
                .map_or(Ok(()), fs::create_dir_all)
                .and_then(|_| fs::write(&path, format!("# {}\n\n", view.title)));
            if let Err(e) = created {
                return self.set_status(format!("can't create {}: {e}", config::pretty_path(&path)), Level::Err);
            }
        }
        self.reopen_notes = Some((view.root, view.title));
        self.pending = Some(Foreground {
            cmd: format!("{} {}", self.config.editor, quote(&path)),
            cwd: self.browser.cwd.clone(),
            wait: false,
            then_reload_config: false,
            cd_to_output: false,
        });
    }
}
