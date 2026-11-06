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

    pub(super) fn confirm_delete(&mut self) {
        let Some(e) = self.browser.selected() else { return };
        let verb = if trash_cmd().is_some() { "Trash" } else { "Permanently delete" };
        let what = if e.is_dir { format!("{}/ and everything in it", e.name) } else { e.name.clone() };
        self.mode = Mode::Confirm { msg: format!("{verb} {what}? [y/N]"), action: Confirmed::Delete(e.path.clone()) };
    }

    pub(super) fn delete(&mut self, path: &Path) {
        let res = match trash_cmd() {
            Some(cmd) => run_quiet(&format!("{cmd} {}", quote(path))),
            None if path.is_dir() && !path.is_symlink() => fs::remove_dir_all(path),
            None => fs::remove_file(path),
        };
        match res {
            Ok(()) => self.set_status(format!("deleted {}", config::pretty_path(path)), Level::Ok),
            Err(e) => self.set_status(format!("delete failed: {e}"), Level::Err),
        }
        self.browser.reload();
        self.invalidate();
    }

    pub(super) fn paste(&mut self, mv: bool) {
        let Some(src) = self.yank.clone() else {
            self.set_status("nothing yanked — y on a file first", Level::Info);
            return;
        };
        let Some(name) = src.file_name() else { return };
        let mut dst = self.browser.cwd.join(name);
        if mv && dst == src {
            self.set_status("already here", Level::Info);
            return;
        }
        if dst.exists() {
            if mv {
                self.set_status(format!("{} already exists here", name.to_string_lossy()), Level::Err);
                return;
            }
            dst = unique_name(&dst);
        }
        let res = if mv {
            move_path(&src, &dst)
        } else {
            run_quiet(&format!("cp -a -- {} {}", quote(&src), quote(&dst)))
        };
        match res {
            Ok(()) => {
                if mv {
                    self.yank = None;
                }
                let verb = if mv { "moved" } else { "copied" };
                self.set_status(format!("{verb} → {}", config::pretty_path(&dst)), Level::Ok);
                self.browser.reload();
                if let Some(n) = dst.file_name() {
                    self.browser.select_name(&n.to_string_lossy());
                }
            }
            Err(e) => self.set_status(format!("paste failed: {e}"), Level::Err),
        }
        self.invalidate();
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

fn run_quiet(cmd: &str) -> std::io::Result<()> {
    let out = Command::new("sh").arg("-c").arg(cmd).stdin(Stdio::null()).output()?;
    if out.status.success() {
        Ok(())
    } else {
        let msg = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(std::io::Error::other(if msg.is_empty() { format!("{cmd} failed") } else { msg }))
    }
}
