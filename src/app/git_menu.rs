//! The `g` menu and the `gh repo create` form it opens.

use ratatui::crossterm::event::{KeyCode, KeyEvent};

use super::{
    App, Confirmed, Level, Mode, PromptKind,
    files::{describe, run_quiet},
};
use crate::{
    config,
    form::{FormEvent, FormKind},
    git::{self, GIT},
    project::{self, Plan, quote, quote_str},
};

impl App {
    /// s / u: stages or unstages the marked entries (or the selection) in place.
    pub(super) fn stage(&mut self, add: bool) {
        self.await_git();
        if self.git.is_none() {
            return self.set_status("not a git repository", Level::Err);
        }
        let paths = self.targets();
        if paths.is_empty() {
            return;
        }
        let files = paths.iter().map(|p| quote(p)).collect::<Vec<_>>().join(" ");
        let cmd = if add {
            format!("git add -A -- {files}")
        } else {
            // Before the first commit there's no HEAD to restore from.
            format!("git restore --staged -- {files} 2>/dev/null || git rm -r -q --cached -- {files}")
        };
        match run_quiet(&cmd) {
            Ok(()) => {
                let verb = if add { "staged" } else { "unstaged" };
                self.set_status(format!("{verb} {}", describe(&paths)), Level::Ok);
                self.browser.marked.clear();
            }
            Err(e) => self.set_status(e.to_string(), Level::Err),
        }
        self.invalidate();
    }

    pub(super) fn key_git(&mut self, key: KeyEvent) {
        self.mode = Mode::Normal;
        let KeyCode::Char(c) = key.code else { return };
        if self.git.is_none() && !matches!(c, 'i' | 'g' | 'h') {
            self.set_status("not a git repository — g i to create one", Level::Err);
            return;
        }
        let cwd = self.browser.cwd.clone();
        let targets = self.targets();
        // Add, unstage and discard take every marked entry; log and blame want one file.
        let many = targets.iter().map(|p| quote(p)).collect::<Vec<_>>().join(" ");
        let sel = self.browser.selected().map(|e| quote(&e.path));
        let needs_sel = matches!(c, 'L' | 'w' | 'a' | 'u' | 'x');
        let Some(f) = sel.or_else(|| (!needs_sel).then(String::new)) else {
            self.set_status("select a file first", Level::Err);
            return;
        };

        let cmd = match c {
            's' => format!("{GIT} status"),
            'd' => format!("{GIT} diff"),
            'D' => format!("{GIT} diff --staged"),
            'l' => format!("{GIT} log --graph --oneline --decorate --all -n 500"),
            'L' => format!("{GIT} log --follow --stat -n 100 -- {f}"),
            'w' => format!("{GIT} blame --date=short -- {f}"),
            'a' => format!("{GIT} add -- {many} && {GIT} status --short"),
            'A' => format!("{GIT} add -A && {GIT} status --short"),
            'u' => format!("{GIT} restore --staged -- {many} && {GIT} status --short"),
            'f' => format!("{GIT} fetch --all --prune"),
            'p' => format!("{GIT} pull"),
            'P' if self.git.as_ref().is_some_and(|g| !g.upstream) => format!("{GIT} push -u origin HEAD"),
            'P' => format!("{GIT} push"),
            'b' => format!("{GIT} branch -a -vv"),
            'z' => format!("{GIT} stash push -u"),
            'Z' => format!("{GIT} stash pop"),
            'i' => format!("{GIT} init"),
            'c' => return self.prompt(PromptKind::Commit { all: false }, ""),
            'C' => return self.prompt(PromptKind::Commit { all: true }, ""),
            'o' => return self.prompt(PromptKind::SwitchBranch, ""),
            'n' => return self.prompt(PromptKind::NewBranch, ""),
            'e' => {
                let cmd = format!("GIT_EDITOR={} git commit --amend", quote_str(&self.config.editor));
                return self.foreground(Plan { cmd, cwd }, true);
            }
            'x' => {
                self.mode = Mode::Confirm {
                    msg: format!("Discard all uncommitted changes to {}? [y/N]", describe(&targets)),
                    action: Confirmed::Task(Plan { cmd: format!("{GIT} restore -- {many}"), cwd }),
                };
                return;
            }
            'h' => {
                let source = match &self.git {
                    Some(g) => g.root.clone(),
                    None => self.project.as_ref().map_or_else(|| cwd.clone(), |p| p.root.clone()),
                };
                match git::gh_repo_form(&source) {
                    Ok(form) => self.mode = Mode::Form(form),
                    Err(e) => self.set_status(e, Level::Err),
                }
                return;
            }
            'g' => {
                let Some(tool) = ["lazygit", "gitui", "tig"].into_iter().find(|t| project::which(t).is_some()) else {
                    self.set_status("none of lazygit, gitui or tig is installed", Level::Err);
                    return;
                };
                return self.foreground(Plan { cmd: tool.into(), cwd }, false);
            }
            _ => return,
        };
        self.start_task(cmd.replace(GIT, "git"), Plan { cmd, cwd });
    }

    pub(super) fn key_form(&mut self, key: KeyEvent) {
        let Mode::Form(form) = &mut self.mode else { return };
        match form.key(key) {
            FormEvent::None => {}
            FormEvent::Cancel => {
                self.mode = Mode::Normal;
                self.set_status("cancelled", Level::Info);
            }
            FormEvent::Submit => {
                let Mode::Form(form) = std::mem::replace(&mut self.mode, Mode::Normal) else { return };
                match form.kind {
                    FormKind::GhRepoCreate => {
                        let source = config::expand_tilde(form.get("source"));
                        if !source.is_dir() {
                            self.set_status(format!("{} is not a directory", source.display()), Level::Err);
                            self.mode = Mode::Form(form);
                            return;
                        }
                        let cmd = git::gh_repo_create_cmd(&form);
                        self.start_task("gh repo create".into(), Plan { cmd, cwd: source });
                    }
                }
            }
        }
    }
}
