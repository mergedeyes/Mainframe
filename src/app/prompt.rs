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
            PromptKind::NewFile | PromptKind::Rename(_) | PromptKind::SaveOutput => {
                Some(Completer::new(complete::Kind::Path, false))
            }
            PromptKind::NewDir => Some(Completer::new(complete::Kind::Dir, false)),
            PromptKind::Command => Some(Completer::new(complete::Kind::Shell, false)),
            PromptKind::SwitchBranch => Some(Completer::list(git::branches(&self.browser.cwd))),
            PromptKind::PushBranch => Some(Completer::list(git::local_branches(&self.browser.cwd))),
            PromptKind::RemoveWorktree => {
                Some(Completer::list(git::worktrees(&self.browser.cwd).iter().skip(1).map(|w| w.name()).collect()))
            }
            PromptKind::Commit { .. } | PromptKind::NewBranch => None,
        };
        // Prefilled inputs (rename) shouldn't pop a list until the user types.
        if let Some(c) = completer.as_mut().filter(|_| initial.is_empty()) {
            c.update(initial, false);
        }
        self.mode = Mode::Prompt { kind, input: LineEdit::new(initial), completer, browsing: None };
    }

    pub(super) fn key_prompt(&mut self, key: KeyEvent) {
        let Mode::Prompt { kind, input, completer, browsing } = &mut self.mode else { return };
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let has_items = completer.as_ref().is_some_and(|c| !c.items.is_empty());
        match key.code {
            KeyCode::Esc => self.mode = Mode::Normal,
            KeyCode::Char('c') if ctrl => self.mode = Mode::Normal,
            // In `:` the arrows walk the history, narrowed to what was typed before,
            // unless the completion dropdown is open: then they move through it.
            KeyCode::Up | KeyCode::Down if matches!(kind, PromptKind::Command) && !has_items => {
                let (pos, prefix) = browsing.take().unwrap_or_else(|| (None, input.text.clone()));
                let next = match key.code {
                    KeyCode::Up => self.history.older(&prefix, pos).or(pos),
                    _ => pos.and_then(|p| self.history.newer(&prefix, p)),
                };
                *input = LineEdit::new(next.map_or(prefix.as_str(), |i| self.history.get(i)));
                if let Some(c) = completer.as_mut() {
                    c.items.clear();
                    c.selected = None;
                }
                if next.is_some() {
                    *browsing = Some((next, prefix));
                }
            }
            KeyCode::Tab | KeyCode::BackTab | KeyCode::Down | KeyCode::Up
                if completer.is_some() && (has_items || matches!(key.code, KeyCode::Tab | KeyCode::BackTab)) =>
            {
                // Completion works on the word before the cursor and keeps the rest.
                // Tab completes shell-style; arrows just move the highlight.
                let forward = matches!(key.code, KeyCode::Tab | KeyCode::Down);
                if let Some(c) = completer.as_mut() {
                    let mut head = input.head().to_string();
                    match key.code {
                        KeyCode::Up | KeyCode::Down => c.step(&mut head, forward),
                        _ => c.cycle(&mut head, forward),
                    }
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
                let edit = input.key(key);
                if edit == Edit::Changed {
                    *browsing = None;
                }
                if edit != Edit::Ignored
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
            PromptKind::SaveOutput => {
                let path = self.resolve(&input);
                let out = &self.tabs[self.tab].output;
                let res = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&path)
                    .and_then(|mut f| std::io::Write::write_all(&mut f, out.text().as_bytes()));
                match res {
                    Ok(()) => {
                        let msg = format!("saved {} lines → {}", out.lines.len(), config::pretty_path(&path));
                        self.set_status(msg, Level::Ok);
                        self.browser.reload();
                        self.invalidate();
                    }
                    Err(e) => self.set_status(format!("can't save {}: {e}", config::pretty_path(&path)), Level::Err),
                }
            }
            PromptKind::Command => {
                self.history.add(&input);
                self.start_task(input.clone(), Plan { cmd: input, cwd });
            }
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
            PromptKind::RemoveWorktree => self.confirm_remove_worktree(input.trim()),
            PromptKind::PushBranch => match git::push_branch_cmd(&cwd, input.trim()) {
                Ok(cmd) => self.start_task(format!("git push {}", input.trim()), Plan { cmd, cwd }),
                Err(e) => self.set_status(e, Level::Err),
            },
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
