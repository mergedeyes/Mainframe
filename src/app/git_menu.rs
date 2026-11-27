//! The `g` menu and the `gh repo create` form it opens.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{
    App, Confirmed, Level, Mode, PromptKind,
    files::{describe, run_quiet},
};
use crate::{
    cmd::{Cmd, Step},
    config,
    form::{FormEvent, FormKind},
    git,
    project::{self, Plan},
    release,
};

impl App {
    /// Asks before `git worktree remove`, saying what stays and what would be lost.
    pub(super) fn confirm_remove_worktree(&mut self, which: &str) {
        let all = git::worktrees(&self.browser.cwd);
        let Some(main) = all.first().map(|w| w.path.clone()) else {
            return self.set_status("not a git repository", Level::Err);
        };
        let wanted = config::expand_tilde(which);
        let Some(w) = all.iter().skip(1).find(|w| w.name() == which || w.path == wanted) else {
            return self.set_status(format!("no worktree {which} — Tab lists them"), Level::Err);
        };
        let dirty = if w.gone { 0 } else { git::uncommitted(&w.path) };
        let mut msg = format!("Remove worktree {}", config::pretty_path(&w.path));
        if w.gone {
            msg.push_str(" (its folder is already gone)");
        }
        if let Some(b) = &w.branch {
            msg.push_str(&format!("; branch {b} stays"));
        }
        if dirty > 0 {
            msg.push_str(&format!("; its {dirty} uncommitted change{} are lost", if dirty == 1 { "" } else { "s" }));
        }
        self.mode = Mode::Confirm {
            msg: format!("{msg}? [y/N]"),
            action: Confirmed::RemoveWorktree { path: w.path.clone(), main, force: dirty > 0 },
        };
    }

    /// Leaves the worktree first when you're in it, so the browser isn't left in a deleted folder.
    pub(super) fn remove_worktree(&mut self, path: &std::path::Path, main: std::path::PathBuf, force: bool) {
        if self.browser.cwd.starts_with(path) {
            self.browser.cd(main.clone());
            self.invalidate();
        }
        let mut step = Step::git().args(["worktree", "remove"]);
        if force {
            step = step.arg("--force");
        }
        self.start_task("git worktree remove".into(), Plan { cmd: step.path(path).into(), cwd: main });
    }

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
        let files = paths.iter().map(|p| p.to_string_lossy().into_owned());
        let res = if add {
            run_quiet(&Step::words("git add -A --").args(files))
        } else {
            // Before the first commit there's no HEAD to restore from.
            run_quiet(&Step::words("git restore --staged --").args(files.clone()))
                .or_else(|_| run_quiet(&Step::words("git rm -r -q --cached --").args(files)))
        };
        match res {
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
        let many: Vec<String> = targets.iter().map(|p| p.to_string_lossy().into_owned()).collect();
        let sel = self.browser.selected().map(|e| e.path.to_string_lossy().into_owned());
        let needs_sel = matches!(c, 'L' | 'w' | 'a' | 'u' | 'x');
        let Some(f) = sel.or_else(|| (!needs_sel).then(String::new)) else {
            self.set_status("select a file first", Level::Err);
            return;
        };

        let git = |args: &str| Step::git().args(args.split_whitespace());
        let status = || git("status --short");
        let cmd: Cmd = match c {
            's' => git("status").into(),
            'd' => git("diff").into(),
            'D' => git("diff --staged").into(),
            'l' => git("log --graph --oneline --decorate --all -n 500").into(),
            'L' => git("log --follow --stat -n 100 --").arg(&f).into(),
            'w' => git("blame --date=short --").arg(&f).into(),
            'a' => git("add --").args(&many).then(status()),
            'A' => git("add -A").then(status()),
            'u' => git("restore --staged --").args(&many).then(status()),
            'f' => git::fetch_cmd(self.config.prune_gone_branches, git::base_branch(&cwd).as_deref()),
            'p' => git("pull").into(),
            'P' if self.git.as_ref().is_some_and(|g| !g.upstream) => git("push -u origin HEAD").into(),
            'P' => git("push").into(),
            'b' => git("branch -a -vv").into(),
            'z' => git("stash push -u").into(),
            'Z' => git("stash pop").into(),
            'i' => git("init").into(),
            'c' => return self.prompt(PromptKind::Commit { all: false }, ""),
            'C' => return self.prompt(PromptKind::Commit { all: true }, ""),
            'o' => return self.open_branch_picker(),
            'n' => return self.prompt(PromptKind::NewBranch, ""),
            'B' => return self.prompt(PromptKind::PushBranch, ""),
            'W' => {
                if git::worktrees(&cwd).len() < 2 {
                    return self.set_status("this repo has no other worktrees", Level::Info);
                }
                return self.prompt(PromptKind::RemoveWorktree, "");
            }
            'e' => {
                // git runs it through its own sh, so it's the setting as written.
                let cmd = Step::words("git commit --amend").env("GIT_EDITOR", self.config.editor());
                return self.foreground(Plan { cmd: cmd.into(), cwd }, true);
            }
            'x' => {
                self.mode = Mode::Confirm {
                    msg: format!("Discard all uncommitted changes to {}? [y/N]", describe(&targets)),
                    action: Confirmed::Task(Plan { cmd: git("restore --").args(&many).into(), cwd }),
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
            'r' | 'R' => {
                let Some(g) = &self.git else { return };
                let root = g.root.clone();
                if c == 'R' {
                    match release::overview(&root) {
                        Ok(cmd) => self.start_task("releases".into(), Plan { cmd, cwd: root }),
                        Err(e) => self.set_status(e, Level::Err),
                    }
                    return;
                }
                match release::form(&root, &g.branch, g.dirty()) {
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
                return self.foreground(Plan { cmd: Step::new(tool).into(), cwd }, false);
            }
            _ => return,
        };
        self.start_task(cmd.display(&self.config.shell()), Plan { cmd, cwd });
    }

    pub(super) fn key_form(&mut self, key: KeyEvent) {
        let Mode::Form(form) = &mut self.mode else { return };
        if matches!(form.kind, FormKind::NewProject)
            && key.code == KeyCode::Char('t')
            && key.modifiers.contains(KeyModifiers::CONTROL)
        {
            return self.edit_claude_template();
        }
        match form.key(key) {
            FormEvent::None => {}
            FormEvent::Cancel => {
                self.mode = Mode::Normal;
                self.set_status("cancelled", Level::Info);
            }
            FormEvent::Submit => {
                let Mode::Form(form) = std::mem::replace(&mut self.mode, Mode::Normal) else { return };
                match &form.kind {
                    FormKind::Release(ctx) => {
                        if let Err(e) = release::check(&form, ctx) {
                            self.set_status(e, Level::Err);
                            self.mode = Mode::Form(form);
                            return;
                        }
                        let cmd = Cmd::Steps(release::steps(&form, ctx));
                        let label = format!("release {}", form.get("version").trim());
                        let cwd = ctx.root.clone();
                        self.start_task(label, Plan { cmd, cwd });
                    }
                    FormKind::Deploy { .. } => {
                        if !self.submit_deploy_form(&form) {
                            self.mode = Mode::Form(form);
                        }
                    }
                    FormKind::NewProject => {
                        if !self.submit_new_project(&form) {
                            self.mode = Mode::Form(form);
                        }
                    }
                    FormKind::BlogPost { .. } => {
                        if !self.submit_blog_form(&form) {
                            self.mode = Mode::Form(form);
                        }
                    }
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
