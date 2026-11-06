//! Ctrl-P: fuzzy-find any file in the project and jump to it.

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use ratatui::{
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    widgets::ListState,
};

use super::{App, Level, Mode};
use crate::{
    config, fuzzy,
    line::{Edit, LineEdit},
    watch,
};

/// Enough for any project you'd browse by hand; keeps scoring instant.
const MAX_FILES: usize = 100_000;
/// Only the best this many are listed.
const SHOWN: usize = 200;

pub struct FileFinder {
    pub root: PathBuf,
    /// Paths relative to `root`.
    files: Vec<String>,
    pub filter: LineEdit,
    /// Indices into `files`, best match first.
    pub visible: Vec<usize>,
    pub state: ListState,
    pub total: usize,
}

impl FileFinder {
    fn apply_filter(&mut self) {
        let q = self.filter.text.replace(' ', "").to_lowercase();
        let mut scored: Vec<(i64, usize)> = self
            .files
            .iter()
            .enumerate()
            .filter_map(|(i, f)| fuzzy::score(&q, &f.to_lowercase()).map(|s| (s, i)))
            .collect();
        self.total = scored.len();
        if !q.is_empty() {
            scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        }
        self.visible = scored.into_iter().take(SHOWN).map(|(_, i)| i).collect();
        self.state.select((!self.visible.is_empty()).then_some(0));
    }

    pub fn file(&self, i: usize) -> &str {
        &self.files[i]
    }

    fn selected(&self) -> Option<PathBuf> {
        let i = *self.visible.get(self.state.selected()?)?;
        Some(self.root.join(&self.files[i]))
    }
}

impl App {
    pub(super) fn open_file_finder(&mut self) {
        // Search the whole repo (or project) even from deep inside it.
        let root = self
            .git
            .as_ref()
            .map(|g| g.root.clone())
            .or_else(|| self.project.as_ref().map(|p| p.root.clone()))
            .unwrap_or_else(|| self.browser.cwd.clone());
        let files = list_files(&root, self.git.is_some());
        if files.is_empty() {
            return self.set_status(format!("no files under {}", config::pretty_path(&root)), Level::Info);
        }
        let mut finder = FileFinder {
            root,
            files,
            filter: LineEdit::default(),
            visible: Vec::new(),
            state: ListState::default(),
            total: 0,
        };
        finder.apply_filter();
        self.mode = Mode::Files(finder);
    }

    pub(super) fn key_file_finder(&mut self, key: KeyEvent) {
        let Mode::Files(finder) = &mut self.mode else { return };
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let len = finder.visible.len();
        let cur = finder.state.selected().unwrap_or(0);
        match key.code {
            KeyCode::Esc => self.mode = Mode::Normal,
            KeyCode::Char('c') if ctrl => self.mode = Mode::Normal,
            KeyCode::Down | KeyCode::Tab => finder.state.select(Some((cur + 1).min(len.saturating_sub(1)))),
            KeyCode::Char('n') if ctrl => finder.state.select(Some((cur + 1).min(len.saturating_sub(1)))),
            KeyCode::Up | KeyCode::BackTab => finder.state.select(Some(cur.saturating_sub(1))),
            KeyCode::Char('p') if ctrl => finder.state.select(Some(cur.saturating_sub(1))),
            // Enter shows the file in the list, ^E opens it in the editor right away.
            KeyCode::Enter => self.pick_file(false),
            KeyCode::Char('e') if ctrl => self.pick_file(true),
            _ => {
                if finder.filter.key(key) == Edit::Changed {
                    finder.apply_filter();
                }
            }
        }
    }

    fn pick_file(&mut self, edit: bool) {
        let Mode::Files(finder) = std::mem::replace(&mut self.mode, Mode::Normal) else { return };
        let Some(path) = finder.selected() else { return };
        let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else { return };
        self.browser.cd(dir.to_path_buf());
        self.browser.select_name(&name.to_string_lossy());
        self.invalidate();
        if edit {
            self.edit_selected();
        }
    }
}

/// Tracked and untracked-but-not-ignored files from git, else a walk that skips the
/// usual build and dependency folders.
fn list_files(root: &Path, git: bool) -> Vec<String> {
    if git {
        let out = Command::new("git")
            .args(["ls-files", "-z", "--cached", "--others", "--exclude-standard"])
            .current_dir(root)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output();
        if let Ok(out) = out
            && out.status.success()
        {
            let mut files: Vec<String> = String::from_utf8_lossy(&out.stdout)
                .split('\0')
                .filter(|f| !f.is_empty() && root.join(f).exists())
                .take(MAX_FILES)
                .map(String::from)
                .collect();
            files.dedup();
            return files;
        }
    }
    let mut files = Vec::new();
    let mut stack = vec![PathBuf::new()];
    while let Some(rel) = stack.pop() {
        let Ok(rd) = fs::read_dir(root.join(&rel)) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let path = rel.join(&name);
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                if !name.starts_with('.') && !watch::SKIP.contains(&name.as_str()) {
                    stack.push(path);
                }
            } else if files.len() < MAX_FILES {
                files.push(path.to_string_lossy().into_owned());
            }
        }
    }
    files.sort();
    files
}
