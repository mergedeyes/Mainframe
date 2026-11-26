//! `J`: every project in the projects folder, most recently active first; jump to one,
//! or make a new one (`^G`).

use std::path::PathBuf;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{App, Confirmed, Level, Mode};
use crate::{
    config,
    filter_list::{self, FilterList, Match},
    form::{Field, Form, FormKind},
    notes,
    projects::{self, Entry},
    scaffold,
};

pub struct ProjectPicker {
    pub root: PathBuf,
    /// Best match first, most recent first while unfiltered.
    pub list: FilterList<Entry>,
    /// Open todos per item, same order as `list.items`.
    pub todos: Vec<usize>,
    /// Tasks running per item (its output tabs live on while you're elsewhere).
    pub running: Vec<usize>,
    /// The project you're in, marked in the list.
    pub current: PathBuf,
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
        self.mode = Mode::Projects(ProjectPicker {
            root,
            list: FilterList::new(items, |e| e.name.clone(), Match::Fuzzy { limit: usize::MAX }),
            todos,
            running,
            current: self.notes_root.clone(),
        });
    }

    pub(super) fn key_project_picker(&mut self, key: KeyEvent) {
        let Mode::Projects(picker) = &mut self.mode else { return };
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Enter => self.pick_project(Pick::Go),
            KeyCode::Char('t') if ctrl => self.pick_project(Pick::Notes),
            KeyCode::Char('o') if ctrl => self.pick_project(Pick::VsCode),
            KeyCode::Char('l') if ctrl => self.pick_project(Pick::Preset),
            KeyCode::Char('g') if ctrl => {
                let root = picker.root.clone();
                self.new_project_form(&root);
            }
            _ => {
                if picker.list.key(key) == filter_list::Key::Close {
                    self.mode = Mode::Normal;
                }
            }
        }
    }

    fn pick_project(&mut self, pick: Pick) {
        let Mode::Projects(picker) = std::mem::replace(&mut self.mode, Mode::Normal) else { return };
        let Some(e) = picker.list.selected().cloned() else { return };
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
            Pick::Preset => self.confirm_preset(e.path),
        }
    }

    /// `^L` in the file list: the preset for the project you're in, without going through `J`.
    pub(super) fn preset_here(&mut self) {
        let dir = projects::dir(&self.config);
        match projects::owner(&self.browser.cwd, dir.as_deref(), self.git.as_ref(), self.project.as_ref()) {
            Some(root) => self.confirm_preset(root),
            None => self.set_status("not in a project — ^L needs a project folder or a git repo", Level::Info),
        }
    }

    /// `^L`: says what bringing the CLAUDE.md preset into the project will do, and asks.
    fn confirm_preset(&mut self, dir: PathBuf) {
        let name = folder_name(&dir);
        let existing = scaffold::instruction_files(&dir);
        let msg = match existing.first() {
            None => format!(
                "Write a CLAUDE.md from the preset into {name} ({} checks, git ignores it)? [y/N]",
                scaffold::stacks(&dir).join(" + ")
            ),
            Some(f) => format!("Add the CLAUDE.md preset to {name}? Claude merges it into {f} at its next start [y/N]"),
        };
        self.mode = Mode::Confirm { msg, action: Confirmed::Preset(dir) };
    }

    pub(super) fn apply_preset(&mut self, dir: &std::path::Path) {
        let name = folder_name(dir);
        let preset = scaffold::render_stacks(&scaffold::template(), &name, &scaffold::stacks(dir));
        match scaffold::apply_preset(dir, &preset) {
            Ok(scaffold::Preset::Written) => {
                self.set_status(format!("{name}: wrote CLAUDE.md from the preset, git ignores it"), Level::Ok);
            }
            Ok(scaffold::Preset::Pending(again)) => {
                let again = if again { " (replaced the one still waiting)" } else { "" };
                let msg = format!("{name}: Claude merges the preset at its next start{again} — {}", scaffold::PENDING);
                self.set_status(msg, Level::Ok);
            }
            Err(e) => self.set_status(format!("{name}: {e}"), Level::Err),
        }
        self.browser.reload();
        self.invalidate();
    }
}

impl App {
    fn new_project_form(&mut self, root: &std::path::Path) {
        self.mode = Mode::Form(Form {
            kind: FormKind::NewProject,
            title: " new project ".into(),
            fields: vec![
                Field::text("name", "", "folder name"),
                Field::dir(
                    "location",
                    config::pretty_path(root),
                    "Tab completes · a group folder like Games/ works too",
                ),
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
        let claude_md =
            (form.get("CLAUDE.md") == "yes").then(|| scaffold::render(&scaffold::template(), name, form.get("stack")));
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
        if self.edit_path(&scaffold::template_path(), scaffold::CLAUDE_MD)
            && let Mode::Form(form) = &mut self.mode
        {
            form.note = Some(template_note());
        }
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
    Preset,
}

fn folder_name(dir: &std::path::Path) -> String {
    dir.file_name().map_or_else(|| config::pretty_path(dir), |n| n.to_string_lossy().into_owned())
}
