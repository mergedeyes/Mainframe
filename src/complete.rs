//! Shell-style completion for every text input: paths, $PATH programs, zoxide, git branches.

use std::{
    collections::BTreeSet,
    env, fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::{Command, Stdio},
};

use crate::config::{expand_tilde, pretty_path};

const MAX_ITEMS: usize = 500;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Files and directories.
    Path,
    /// Directories only.
    Dir,
    /// Shell command line: programs in command position, escaped paths elsewhere.
    Shell,
    /// Paths, falling back to zoxide when the input looks like keywords.
    Cd,
    /// A fixed word list (e.g. branch names), matched anywhere in the word.
    List,
}

pub struct Item {
    /// Replaces the input from `Completer::start` onwards.
    pub insert: String,
    pub display: String,
    pub is_dir: bool,
}

pub struct Completer {
    kind: Kind,
    /// Branch names for `List`, programs on $PATH for `Shell`.
    words: Vec<String>,
    zoxide: bool,
    pub items: Vec<Item>,
    /// Set while cycling through `items` with Tab / arrows.
    pub selected: Option<usize>,
    /// Byte offset in the input where the completed token starts.
    start: usize,
}

impl Completer {
    pub fn new(kind: Kind, zoxide: bool) -> Self {
        let words = if kind == Kind::Shell { path_programs() } else { Vec::new() };
        Self { kind, words, zoxide, items: Vec::new(), selected: None, start: 0 }
    }

    pub fn list(words: Vec<String>) -> Self {
        Self { kind: Kind::List, words, zoxide: false, items: Vec::new(), selected: None, start: 0 }
    }

    /// Recomputes suggestions for `input`. With `force`, an empty shell token
    /// lists the current directory instead of showing nothing.
    pub fn update(&mut self, input: &str, force: bool) {
        self.selected = None;
        let (start, items) = match self.kind {
            Kind::Path => (0, paths(input, false, false)),
            Kind::Dir => (0, paths(input, true, false)),
            Kind::List => (0, words(&self.words, input)),
            Kind::Cd => {
                let looks_like_path = input.starts_with(['/', '~', '.']) || input.contains('/');
                let found = paths(input, false, false);
                if self.zoxide && !input.trim().is_empty() && !looks_like_path && (found.is_empty() || input.contains(' ')) {
                    (0, zoxide(input))
                } else {
                    (0, found)
                }
            }
            Kind::Shell => {
                let start = shell_token_start(input);
                let token = &input[start..];
                let items = if token.is_empty() && !force {
                    Vec::new()
                } else if is_command_position(&input[..start]) && !token.contains('/') {
                    if token.is_empty() { Vec::new() } else { programs(&self.words, token) }
                } else {
                    paths(token, false, true)
                };
                (start, items)
            }
        };
        self.start = start;
        self.items = items;
    }

    /// Tab / Shift-Tab / arrows. Completes a unique match or the common prefix,
    /// otherwise cycles through the matches.
    pub fn cycle(&mut self, input: &mut String, forward: bool) {
        if self.items.is_empty() && self.selected.is_none() {
            self.update(input, true);
        }
        let n = self.items.len();
        if n == 0 {
            return;
        }
        if let Some(sel) = self.selected {
            let next = if forward { (sel + 1) % n } else { (sel + n - 1) % n };
            self.selected = Some(next);
            self.replace(input, next);
            return;
        }
        let token_len = input[self.start..].chars().count();
        if n == 1 {
            let dir = self.items[0].is_dir;
            self.replace(input, 0);
            if self.kind == Kind::Shell && !dir {
                input.push(' ');
            }
            // Descend: a completed directory immediately lists its contents.
            self.update(input, dir);
            return;
        }
        let lcp = common_prefix(self.items.iter().map(|i| i.insert.as_str()));
        if lcp.chars().count() > token_len {
            input.truncate(self.start);
            input.push_str(&lcp);
            self.update(input, false);
            return;
        }
        let first = if forward { 0 } else { n - 1 };
        self.selected = Some(first);
        self.replace(input, first);
    }

    fn replace(&self, input: &mut String, i: usize) {
        input.truncate(self.start);
        input.push_str(&self.items[i].insert);
    }
}

fn paths(token: &str, dirs_only: bool, shell: bool) -> Vec<Item> {
    let raw = if shell { unescape(token) } else { token.to_string() };
    if raw == "~" {
        return vec![Item { insert: "~/".into(), display: "~/".into(), is_dir: true }];
    }
    let (dir_part, prefix) = match raw.rfind('/') {
        Some(i) => (&raw[..=i], &raw[i + 1..]),
        None => ("", raw.as_str()),
    };
    let dir = if dir_part.is_empty() { PathBuf::from(".") } else { expand_tilde(dir_part) };
    let Ok(rd) = fs::read_dir(&dir) else { return Vec::new() };

    let lower = prefix.to_lowercase();
    let show_hidden = prefix.starts_with('.');
    let (mut starts, mut contains) = (Vec::new(), Vec::new());
    for de in rd.flatten() {
        let name = de.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') && !show_hidden {
            continue;
        }
        let is_dir = de.path().is_dir();
        if dirs_only && !is_dir {
            continue;
        }
        let l = name.to_lowercase();
        if l.starts_with(&lower) {
            starts.push((name, is_dir));
        } else if !lower.is_empty() && l.contains(&lower) {
            contains.push((name, is_dir));
        }
    }
    let mut found = if starts.is_empty() { contains } else { starts };
    found.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase())));
    found.truncate(MAX_ITEMS);

    // Keep the directory part exactly as typed (escaped, `~`, relative…).
    let typed_dir = &token[..token.rfind('/').map_or(0, |i| i + 1)];
    found
        .into_iter()
        .map(|(name, is_dir)| {
            let slash = if is_dir { "/" } else { "" };
            let shown = if shell { escape(&name) } else { name.clone() };
            Item { insert: format!("{typed_dir}{shown}{slash}"), display: format!("{name}{slash}"), is_dir }
        })
        .collect()
}

fn words(list: &[String], input: &str) -> Vec<Item> {
    let lower = input.to_lowercase();
    let mut starts: Vec<&String> = list.iter().filter(|w| w.to_lowercase().starts_with(&lower)).collect();
    if starts.is_empty() {
        starts = list.iter().filter(|w| w.to_lowercase().contains(&lower)).collect();
    }
    starts
        .into_iter()
        .take(MAX_ITEMS)
        .map(|w| Item { insert: w.clone(), display: w.clone(), is_dir: false })
        .collect()
}

fn programs(list: &[String], prefix: &str) -> Vec<Item> {
    list.iter()
        .filter(|p| p.starts_with(prefix))
        .take(MAX_ITEMS)
        .map(|p| Item { insert: p.clone(), display: p.clone(), is_dir: false })
        .collect()
}

fn zoxide(input: &str) -> Vec<Item> {
    let Ok(out) = Command::new("zoxide")
        .args(["query", "--list", "--"])
        .args(input.split_whitespace())
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
    else {
        return Vec::new();
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .take(30)
        .map(|l| {
            let p = PathBuf::from(l);
            Item { insert: l.to_string(), display: pretty_path(&p), is_dir: true }
        })
        .collect()
}

/// Executable names on $PATH, sorted and deduplicated.
fn path_programs() -> Vec<String> {
    let mut set = BTreeSet::new();
    for dir in env::var_os("PATH").map(|p| env::split_paths(&p).collect::<Vec<_>>()).unwrap_or_default() {
        let Ok(rd) = fs::read_dir(dir) else { continue };
        for de in rd.flatten() {
            // fs::metadata follows symlinks (rustup's cargo, etc. are links).
            let exec = fs::metadata(de.path()).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0);
            if exec {
                set.insert(de.file_name().to_string_lossy().into_owned());
            }
        }
    }
    set.into_iter().collect()
}

/// Start of the last whitespace-separated token, honoring backslash escapes.
fn shell_token_start(input: &str) -> usize {
    let bytes = input.as_bytes();
    let (mut start, mut i) = (0, 0);
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 1,
            b' ' | b'\t' | b';' | b'|' | b'&' | b'(' => start = i + 1,
            _ => {}
        }
        i += 1;
    }
    start.min(input.len())
}

/// True when the next word is a command name (start of line or after ; | & ().
fn is_command_position(before: &str) -> bool {
    let t = before.trim_end();
    t.is_empty() || t.ends_with([';', '|', '&', '('])
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if !(c.is_alphanumeric() || "/._-+=:,@%~".contains(c)) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(n) = chars.next() {
                out.push(n);
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn common_prefix<'a>(mut items: impl Iterator<Item = &'a str>) -> String {
    let Some(first) = items.next() else { return String::new() };
    let mut len = first.len();
    for s in items {
        len = first
            .char_indices()
            .zip(s.chars())
            .take_while(|((_, a), b)| a == b)
            .last()
            .map_or(0, |((i, a), _)| i + a.len_utf8())
            .min(len);
    }
    first[..len].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_start_respects_escapes_and_separators() {
        assert_eq!(shell_token_start("ls my\\ dir/fi"), 3);
        assert_eq!(shell_token_start("make && carg"), 8);
        assert_eq!(shell_token_start("echo"), 0);
    }

    #[test]
    fn command_position() {
        assert!(is_command_position(""));
        assert!(is_command_position("ls | "));
        assert!(!is_command_position("ls "));
    }

    #[test]
    fn prefix_and_escaping() {
        assert_eq!(common_prefix(["src/app.rs", "src/ansi.rs"].into_iter()), "src/a");
        assert_eq!(escape("my file (1).txt"), "my\\ file\\ \\(1\\).txt");
        assert_eq!(unescape("my\\ file"), "my file");
    }
}
