//! `J`: every project in the projects folder, most recently active first; jump to one,
//! or make a new one (`^G`).

use std::{fs, path::PathBuf};

use ratatui::{
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    widgets::ListState,
};

use super::{App, Foreground, Level, Mode};
use crate::{
    config,
    form::{Field, Form, FormKind},
    fuzzy,
    line::{Edit, LineEdit},
    notes,
    project::quote,
    projects::{self, Entry},
    scaffold,
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
        // An empty list still opens: ^G makes the first project.
        let items = projects::scan(&root);
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
            KeyCode::Char('g') if ctrl => {
                let root = picker.root.clone();
                self.new_project_form(&root);
            }
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

impl App {
    fn new_project_form(&mut self, root: &std::path::Path) {
        self.mode = Mode::Form(Form {
            kind: FormKind::NewProject,
            title: " new project ".into(),
            fields: vec![
                Field::text("name", "", "folder name"),
                Field::dir("location", config::pretty_path(root), "Tab completes · a group folder like Games/ works too"),
                Field::choice("stack", scaffold::STACKS, 0, "picks the checks in CLAUDE.md"),
                Field::choice("CLAUDE.md", &["yes", "no"], 0, "local only, listed in .git/info/exclude"),
            ],
            step: 0,
            note: Some(template_note()),
            completer: None,
        });
    }

    /// Makes the project the form describes and goes there. False keeps the form open.
    pub(super) fn submit_new_project(&mut self, form: &Form) -> bool {
        let name = form.get("name").trim();
        let location = config::expand_tilde(form.get("location").trim());
        let dir = match scaffold::target(&location, name) {
            Ok(dir) => dir,
            Err(e) => {
                self.set_status(e, Level::Err);
                return false;
            }
        };
        let claude_md = (form.get("CLAUDE.md") == "yes")
            .then(|| scaffold::render(&scaffold::template(), name, form.get("stack")));
        let res = scaffold::create(&dir, claude_md.as_deref());
        // The folder exists even when a later step failed; going there shows what's in it.
        if dir.is_dir() {
            self.browser.cd(dir.clone());
            self.invalidate();
        }
        match res {
            Ok(()) => {
                let with = if claude_md.is_some() { " with a local CLAUDE.md" } else { "" };
                self.set_status(format!("created {} — git repo on main{with}", config::pretty_path(&dir)), Level::Ok);
            }
            Err(e) => self.set_status(e, Level::Err),
        }
        true
    }

    /// `^T` in the new-project form: the CLAUDE.md template in the editor, copied from the
    /// built-in one first if there's no file yet. The form stays open underneath.
    pub(super) fn edit_claude_template(&mut self) {
        let path = scaffold::template_path();
        if !path.exists() {
            let created = path
                .parent()
                .map_or(Ok(()), fs::create_dir_all)
                .and_then(|_| fs::write(&path, scaffold::CLAUDE_MD));
            if let Err(e) = created {
                return self.set_status(format!("can't create {}: {e}", path.display()), Level::Err);
            }
        }
        if let Mode::Form(form) = &mut self.mode {
            form.note = Some(template_note());
        }
        self.pending = Some(Foreground {
            cmd: format!("{} {}", self.config.editor, quote(&path)),
            cwd: self.browser.cwd.clone(),
            wait: false,
            then_reload_config: false,
            cd_to_output: false,
        });
    }
}

/// Which template CLAUDE.md comes from, for the form's note line.
fn template_note() -> String {
    let path = config::pretty_path(&scaffold::template_path());
    if scaffold::template_path().exists() {
        format!("template: {path} · ^T edits it")
    } else {
        format!("template: built-in · ^T copies it to {path} to edit")
    }
}

enum Pick {
    Go,
    Notes,
    VsCode,
}
