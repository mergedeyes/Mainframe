//! Ctrl-P: fuzzy-find any file in the project and jump to it.

use std::{
    fs,
    path::{Path, PathBuf},
};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{App, Level, Mode};
use crate::{
    config,
    filter_list::{self, FilterList, Match},
    watch,
};

/// Enough for any project you'd browse by hand; keeps scoring instant.
const MAX_FILES: usize = 100_000;
/// Only the best this many are listed.
const SHOWN: usize = 200;

pub struct FileFinder {
    pub root: PathBuf,
    /// Paths relative to `root`.
    pub list: FilterList<String>,
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
        let list = FilterList::new(files, String::clone, Match::Fuzzy { limit: SHOWN });
        self.mode = Mode::Files(FileFinder { root, list });
    }

    pub(super) fn key_file_finder(&mut self, key: KeyEvent) {
        let Mode::Files(finder) = &mut self.mode else { return };
        match key.code {
            // Enter shows the file in the list, ^E opens it in the editor right away.
            KeyCode::Enter => self.pick_file(false),
            KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => self.pick_file(true),
            _ => {
                if finder.list.key(key) == filter_list::Key::Close {
                    self.mode = Mode::Normal;
                }
            }
        }
    }

    fn pick_file(&mut self, edit: bool) {
        let Mode::Files(finder) = std::mem::replace(&mut self.mode, Mode::Normal) else { return };
        let Some(path) = finder.list.selected().map(|f| finder.root.join(f)) else { return };
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
        let args = ["ls-files", "-z", "--cached", "--others", "--exclude-standard"];
        if let Some(out) = crate::git::git(root, &args) {
            let mut files: Vec<String> = String::from_utf8_lossy(&out)
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
