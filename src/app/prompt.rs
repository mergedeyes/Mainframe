//! Footer prompts: completion while typing, and what each one does on Enter.

use std::{
    fs,
    path::{Path, PathBuf},
};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{App, Level, Mode, PromptKind, files::move_path};
use crate::{
    complete::{self, Completer},
    config, git,
    git::GIT,
    line::{Edit, LineEdit},
    project::{Plan, quote_str},
};

impl App {
    pub(super) fn prompt(&mut self, kind: PromptKind, initial: &str) {
        let mut completer = match kind {
            PromptKind::Cd => Some(Completer::new(complete::Kind::Cd, self.zoxide)),
            PromptKind::NewFile | PromptKind::Rename(_) => Some(Completer::new(complete::Kind::Path, false)),
            PromptKind::NewDir => Some(Completer::new(complete::Kind::Dir, false)),
            PromptKind::Command => Some(Completer::new(complete::Kind::Shell, false)),
            PromptKind::SwitchBranch => Some(Completer::list(git::branches(&self.browser.cwd))),
            PromptKind::Commit { .. } | PromptKind::NewBranch => None,
        };
        // Prefilled inputs (rename) shouldn't pop a list until the user types.
        if let Some(c) = completer.as_mut().filter(|_| initial.is_empty()) {
            c.update(initial, false);
        }
        self.mode = Mode::Prompt { kind, input: LineEdit::new(initial), completer };
    }

    pub(super) fn key_prompt(&mut self, key: KeyEvent) {
        let Mode::Prompt { kind, input, completer } = &mut self.mode else { return };
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let has_items = completer.as_ref().is_some_and(|c| !c.items.is_empty());
        match key.code {
            KeyCode::Esc => self.mode = Mode::Normal,
            KeyCode::Char('c') if ctrl => self.mode = Mode::Normal,
            KeyCode::Tab | KeyCode::BackTab | KeyCode::Down | KeyCode::Up
                if completer.is_some() && (has_items || matches!(key.code, KeyCode::Tab | KeyCode::BackTab)) =>
            {
                // Completion works on the word before the cursor and keeps the rest.
                let forward = matches!(key.code, KeyCode::Tab | KeyCode::Down);
                if let Some(c) = completer.as_mut() {
                    let mut head = input.head().to_string();
                    c.cycle(&mut head, forward);
                    input.replace_head(head);
                }
            }
            KeyCode::Enter => {
                let (kind, input) = (kind.clone(), input.text.trim().to_string());
                self.mode = Mode::Normal;
                if !input.is_empty() {
                    self.submit_prompt(kind, input);
                }
            }
            _ => {
                if input.key(key) != Edit::Ignored
                    && let Some(c) = completer.as_mut()
                {
                    c.update(input.head(), false);
                }
            }
        }
    }

    pub(super) fn resolve(&self, input: &str) -> PathBuf {
        let p = config::expand_tilde(input);
        if p.is_absolute() { p } else { self.browser.cwd.join(p) }
    }

    pub(super) fn submit_prompt(&mut self, kind: PromptKind, input: String) {
        let cwd = self.browser.cwd.clone();
        match kind {
            PromptKind::Command => self.start_task(input.clone(), Plan { cmd: input, cwd }),
            PromptKind::Commit { all } => {
                let add = if all { format!("{GIT} add -A && ") } else { String::new() };
                let cmd = format!("{add}{GIT} commit -m {}", quote_str(&input));
                self.start_task("git commit".into(), Plan { cmd, cwd });
            }
            PromptKind::SwitchBranch => {
                let cmd = format!("{GIT} switch {}", quote_str(&input));
                self.start_task("git switch".into(), Plan { cmd, cwd });
            }
            PromptKind::NewBranch => {
                let cmd = format!("{GIT} switch -c {}", quote_str(&input));
                self.start_task("git switch -c".into(), Plan { cmd, cwd });
            }
            PromptKind::NewFile => {
                let path = self.resolve(&input);
                let res = path
                    .parent()
                    .map_or(Ok(()), fs::create_dir_all)
                    .and_then(|_| fs::OpenOptions::new().write(true).create_new(true).open(&path).map(drop));
                self.after_create(&path, res);
            }
            PromptKind::NewDir => {
                let path = self.resolve(&input);
                let res = fs::create_dir_all(&path);
                self.after_create(&path, res);
            }
            PromptKind::Rename(from) => {
                let mut to = self.resolve(&input);
                if to.is_dir() && !from.is_dir()
                    && let Some(name) = from.file_name() {
                        to = to.join(name);
                    }
                match move_path(&from, &to) {
                    Ok(()) => {
                        self.set_status(format!("→ {}", config::pretty_path(&to)), Level::Ok);
                        self.browser.reload();
                        if let Some(name) = to.file_name().filter(|_| to.parent() == Some(&self.browser.cwd)) {
                            self.browser.select_name(&name.to_string_lossy());
                        }
                    }
                    Err(e) => self.set_status(format!("rename failed: {e}"), Level::Err),
                }
                self.invalidate();
            }
            PromptKind::Cd => {
                let path = self.resolve(&input);
                if path.is_dir() {
                    self.browser.cd(path.canonicalize().unwrap_or(path));
                } else if path.exists() {
                    let parent = path.parent().unwrap_or(Path::new("/")).to_path_buf();
                    self.browser.cd(parent);
                    if let Some(name) = path.file_name() {
                        self.browser.select_name(&name.to_string_lossy());
                    }
                } else if let Some(dir) = self.zoxide_query(&input) {
                    self.browser.cd(dir);
                } else {
                    self.set_status(format!("no such path or zoxide match: {input}"), Level::Err);
                }
                self.invalidate();
            }
        }
    }
}
