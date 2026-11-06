//! Creating, renaming, deleting, copying and moving files.

use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use super::{App, Confirmed, Level, Mode};
use crate::{
    config,
    project::{self, quote},
};

impl App {
    pub(super) fn after_create(&mut self, path: &Path, res: std::io::Result<()>) {
        match res {
            Ok(()) => {
                self.set_status(format!("created {}", config::pretty_path(path)), Level::Ok);
                self.browser.reload();
                // Select the top-level component that appeared in this dir.
                if let Ok(rel) = path.strip_prefix(&self.browser.cwd)
                    && let Some(first) = rel.components().next() {
                        self.browser.select_name(&first.as_os_str().to_string_lossy());
                    }
            }
            Err(e) => self.set_status(format!("create failed: {e}"), Level::Err),
        }
        self.invalidate();
    }

    /// Marked entries, or else the selected one.
    pub(super) fn targets(&self) -> Vec<PathBuf> {
        if self.browser.marked.is_empty() {
            self.browser.selected().map(|e| vec![e.path.clone()]).unwrap_or_default()
        } else {
            self.browser.marked.iter().cloned().collect()
        }
    }

    pub(super) fn yank_targets(&mut self) {
        let paths = self.targets();
        if paths.is_empty() {
            return;
        }
        let msg = format!("yanked {} — p to copy here, P to move here", describe(&paths));
        self.yank = paths;
        self.browser.marked.clear();
        self.set_status(msg, Level::Info);
    }

    pub(super) fn confirm_delete(&mut self) {
        let paths = self.targets();
        if paths.is_empty() {
            return;
        }
        let verb = if trash_cmd().is_some() { "Trash" } else { "Permanently delete" };
        let what = match paths.as_slice() {
            [p] if p.is_dir() => format!("{} and everything in it", describe(&paths)),
            _ => describe(&paths),
        };
        self.mode = Mode::Confirm { msg: format!("{verb} {what}? [y/N]"), action: Confirmed::Delete(paths) };
    }

    pub(super) fn delete(&mut self, paths: &[PathBuf]) {
        let trash = trash_cmd();
        let mut failed = None;
        for path in paths {
            let res = match trash {
                Some(cmd) => run_quiet(&format!("{cmd} {}", quote(path))),
                None if path.is_dir() && !path.is_symlink() => fs::remove_dir_all(path),
                None => fs::remove_file(path),
            };
            match res {
                Ok(()) => drop(self.browser.marked.remove(path)),
                Err(e) => failed = failed.or(Some(format!("delete {} failed: {e}", config::pretty_path(path)))),
            }
        }
        match failed {
            Some(msg) => self.set_status(msg, Level::Err),
            None => self.set_status(format!("deleted {}", describe(paths)), Level::Ok),
        }
        self.browser.reload();
        self.invalidate();
    }

    pub(super) fn paste(&mut self, mv: bool) {
        if self.yank.is_empty() {
            self.set_status("nothing yanked — y on a file first (Space marks several)", Level::Info);
            return;
        }
        let mut done = Vec::new();
        let mut failed = None;
        for src in self.yank.clone() {
            let Some(name) = src.file_name() else { continue };
            let mut dst = self.browser.cwd.join(name);
            let res = if mv && dst == src {
                Err("already here".to_string())
            } else if mv && dst.exists() {
                Err(format!("{} already exists here", name.to_string_lossy()))
            } else {
                if dst.exists() {
                    dst = unique_name(&dst);
                }
                let res = if mv { move_path(&src, &dst) } else { run_quiet(&format!("cp -a -- {} {}", quote(&src), quote(&dst))) };
                res.map_err(|e| format!("paste failed: {e}"))
            };
            match res {
                Ok(()) => {
                    if mv {
                        self.yank.retain(|y| y != &src);
                    }
                    done.push(dst);
                }
                Err(e) => failed = failed.or(Some(e)),
            }
        }
        self.browser.reload();
        if let Some(n) = done.last().and_then(|d| d.file_name()) {
            self.browser.select_name(&n.to_string_lossy());
        }
        match failed {
            Some(msg) => self.set_status(msg, Level::Err),
            None => {
                let verb = if mv { "moved" } else { "copied" };
                let msg = match done.as_slice() {
                    [one] => format!("{verb} → {}", config::pretty_path(one)),
                    _ => format!("{verb} {} items → {}", done.len(), config::pretty_path(&self.browser.cwd)),
                };
                self.set_status(msg, Level::Ok);
            }
        }
        self.invalidate();
    }
}

/// `name`, `dir/`, or `3 items` for status lines and confirmations.
pub(super) fn describe(paths: &[PathBuf]) -> String {
    match paths {
        [p] => {
            let name = p.file_name().map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().into_owned());
            if p.is_dir() { format!("{name}/") } else { name }
        }
        _ => format!("{} items", paths.len()),
    }
}

pub(super) fn move_path(from: &Path, to: &Path) -> std::io::Result<()> {
    if to.exists() {
        return Err(std::io::Error::new(ErrorKind::AlreadyExists, "target already exists"));
    }
    match fs::rename(from, to) {
        Err(e) if e.kind() == ErrorKind::CrossesDevices => {
            run_quiet(&format!("mv -n -- {} {}", quote(from), quote(to)))
        }
        other => other,
    }
}

/// `foo.txt` → `foo copy.txt`, `foo copy 2.txt`, …
fn unique_name(path: &Path) -> PathBuf {
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let ext = path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    (1..)
        .map(|n| {
            let suffix = if n == 1 { " copy".to_string() } else { format!(" copy {n}") };
            path.with_file_name(format!("{stem}{suffix}{ext}"))
        })
        .find(|p| !p.exists())
        .unwrap()
}

fn trash_cmd() -> Option<&'static str> {
    if project::which("gio").is_some() {
        Some("gio trash --")
    } else if project::which("trash-put").is_some() {
        Some("trash-put --")
    } else {
        None
    }
}

pub(super) fn run_quiet(cmd: &str) -> std::io::Result<()> {
    let out = Command::new("sh").arg("-c").arg(cmd).stdin(Stdio::null()).output()?;
    if out.status.success() {
        Ok(())
    } else {
        let msg = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(std::io::Error::other(if msg.is_empty() { format!("{cmd} failed") } else { msg }))
    }
}
