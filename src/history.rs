//! Commands run with `:`, kept across sessions in `$XDG_STATE_HOME/mainframe/history`.

use std::{env, fs, path::PathBuf};

use crate::config::home;

const MAX: usize = 1000;

pub struct History {
    /// Oldest first, no duplicates.
    entries: Vec<String>,
    path: PathBuf,
}

impl History {
    pub fn load() -> Self {
        let base = env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home().join(".local/state"));
        let path = base.join("mainframe").join("history");
        let entries = fs::read_to_string(&path)
            .map(|t| t.lines().filter(|l| !l.is_empty()).map(String::from).collect())
            .unwrap_or_default();
        Self { entries, path }
    }

    /// Records `cmd` as the newest entry and saves. Failing to save only costs the history.
    pub fn add(&mut self, cmd: &str) {
        if cmd.contains('\n') {
            return;
        }
        self.entries.retain(|e| e != cmd);
        self.entries.push(cmd.to_string());
        if self.entries.len() > MAX {
            self.entries.drain(..self.entries.len() - MAX);
        }
        if let Some(dir) = self.path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let _ = fs::write(&self.path, self.entries.join("\n") + "\n");
    }

    pub fn get(&self, i: usize) -> &str {
        &self.entries[i]
    }

    /// The closest entry before `from` (or the newest one) that starts with `prefix`.
    pub fn older(&self, prefix: &str, from: Option<usize>) -> Option<usize> {
        let end = from.unwrap_or(self.entries.len());
        (0..end).rev().find(|&i| self.entries[i].starts_with(prefix))
    }

    /// The closest entry after `from` that starts with `prefix`.
    pub fn newer(&self, prefix: &str, from: usize) -> Option<usize> {
        (from + 1..self.entries.len()).find(|&i| self.entries[i].starts_with(prefix))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_navigation() {
        let mut h = History { entries: Vec::new(), path: env::temp_dir().join("mainframe-history-test") };
        for c in ["cargo build", "ls", "cargo test", "ls"] {
            h.add(c);
        }
        assert_eq!(h.entries, ["cargo build", "cargo test", "ls"]);
        let a = h.older("cargo", None).unwrap();
        assert_eq!(h.get(a), "cargo test");
        let b = h.older("cargo", Some(a)).unwrap();
        assert_eq!(h.get(b), "cargo build");
        assert_eq!(h.older("cargo", Some(b)), None);
        assert_eq!(h.newer("cargo", b), Some(a));
        assert_eq!(h.newer("cargo", a), None);
        let _ = fs::remove_file(&h.path);
    }
}
