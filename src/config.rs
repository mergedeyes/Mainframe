use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::Deserialize;

use crate::platform;

pub const TEMPLATE: &str = r#"# Mainframe config
#
# Settings can go anywhere in this file, also below a [section]: Mainframe finds them.
#
# Placeholders usable in commands (quoted for your shell automatically):
#   {file}  selected entry (absolute path)
#   {name}  selected entry's file name
#   {dir}   directory you're currently in
#   {root}  detected project root (falls back to {dir})

# Command used for `e` / Enter. Anything that blocks until you close it works.
editor = "nano"

# Command used for `o` (file) and `O` (folder).
vscode = "code"

# The shell for commands you write: `:`, [runners] and [[actions]]. A name on PATH
# or a path: sh, bash, zsh, fish, pwsh, powershell, cmd. Empty: sh, or PowerShell
# on Windows. Mainframe runs its own builds, tests and git commands without one.
shell = ""

# Where `i` copies standalone executables to.
install_dir = "~/.local/bin"

# The folder `J` lists projects from. Empty: the first of ~/Projekte, ~/Projects,
# ~/projects, ~/Code, ~/code, ~/dev, ~/src, ~/repos that exists.
projects_dir = ""

show_hidden = true
preview = true

# Use zoxide (if installed): `c` accepts keywords, `Z` opens its picker,
# and folders you browse here are added to its database.
zoxide = true

# Project .desktop entries when installing a binary with `i`:
# "ask" shows a diff first, "always" updates silently, "never" leaves them alone.
desktop_entries = "ask"

# Ring the terminal bell and send a desktop notification (notify-send) when a
# task that ran at least this many seconds finishes. 0 turns it off.
notify_after = 10

# Deploy targets with `from` (a branch to build, e.g. "main") build in a separate
# checkout under ~/.cache/mainframe/deploy/. false removes it, build output included,
# once the deploy is done; true keeps it, so the next deploy only rebuilds what changed.
keep_deploy_builds = false

# `g f` fetches and drops remote branches that were deleted (after a merged pull
# request, say). true also deletes the local branches that tracked them, but only
# those whose work is in the default branch (origin/main); the others are kept.
prune_gone_branches = false

# A Jekyll blog repo: inside it (or a worktree of it), `L` lists its drafts and posts,
# writes new ones, publishes and pushes them. Empty: off.
blog_repo = ""

# Override or add run commands per file extension (used by `r` / `R`).
[runners]
# py = "python3 {file}"
# lua = "luajit {file}"

# Custom key-bound actions. Keys: single chars ("Z"), "F1".."F12",
# or with modifiers like "ctrl-g", "alt-x". They take priority over built-ins.
# interactive = true suspends the UI and runs the command in the real terminal.
#
# [[actions]]
# key = "F5"
# name = "git status"
# cmd = "git status"
#
# [[actions]]
# key = "ctrl-g"
# name = "lazygit"
# cmd = "lazygit"
# interactive = true
#
# [[actions]]
# key = "F6"
# name = "flash firmware"
# cmd = "cargo run --release -- --flash {file}"
#
# Settings per project (deploy targets for D, actions only for that project) live in
# ~/.config/mainframe/projects/: press D in a project, then e.
#
# New projects (J, then ^G) get their CLAUDE.md from ~/.config/mainframe/CLAUDE.md
# when it exists, else from the built-in template; ^T in that form edits it.
"#;

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Config {
    pub editor: String,
    pub vscode: String,
    pub shell: String,
    pub install_dir: String,
    pub projects_dir: String,
    pub show_hidden: bool,
    pub preview: bool,
    pub zoxide: bool,
    pub desktop_entries: DesktopMode,
    pub notify_after: u64,
    pub keep_deploy_builds: bool,
    pub prune_gone_branches: bool,
    pub blog_repo: String,
    pub runners: HashMap<String, String>,
    pub actions: Vec<Action>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            editor: "nano".into(),
            vscode: "code".into(),
            shell: String::new(),
            install_dir: "~/.local/bin".into(),
            projects_dir: String::new(),
            show_hidden: true,
            preview: true,
            zoxide: true,
            desktop_entries: DesktopMode::Ask,
            notify_after: 10,
            keep_deploy_builds: false,
            prune_gone_branches: false,
            blog_repo: String::new(),
            runners: HashMap::new(),
            actions: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DesktopMode {
    Ask,
    Always,
    Never,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Action {
    pub key: String,
    pub name: String,
    pub cmd: String,
    #[serde(default)]
    pub interactive: bool,
}

impl Action {
    pub fn matches(&self, key: &KeyEvent) -> bool {
        key_matches(&self.key, key)
    }
}

/// Whether `key` is the key written as `spec` ("F5", "ctrl-g", "Z", …).
pub fn key_matches(spec: &str, key: &KeyEvent) -> bool {
    parse_key(spec).is_some_and(|(code, mods)| {
        // Shift is implied by the character itself for letters/symbols.
        let pressed = key.modifiers - KeyModifiers::SHIFT;
        key.code == code && pressed == mods
    })
}

impl Config {
    pub fn path() -> PathBuf {
        platform::config_dir().join("config.toml")
    }

    /// Returns the config plus an error message if the file exists but is broken.
    pub fn load() -> (Self, Option<String>) {
        let path = Self::path();
        let Ok(text) = fs::read_to_string(&path) else {
            return (Self::default(), None);
        };
        match parse(&text) {
            Ok(cfg) => (cfg, None),
            Err(e) => (Self::default(), Some(format!("config error, {e}"))),
        }
    }
}

/// The settings at the top of config.toml. TOML puts every key below a `[section]`
/// header into that section, and a section can't be closed. None of these names means
/// anything inside a section, so wherever one ends up, it's moved back to the top.
const TOP_LEVEL: [&str; 13] = [
    "editor",
    "vscode",
    "shell",
    "install_dir",
    "projects_dir",
    "show_hidden",
    "preview",
    "zoxide",
    "desktop_entries",
    "notify_after",
    "keep_deploy_builds",
    "prune_gone_branches",
    "blog_repo",
];

/// Reads config.toml with the settings wherever they are. Errors start with the line.
fn parse(text: &str) -> Result<Config, String> {
    let mut table: toml::Table = text.parse().map_err(|e: toml::de::Error| {
        let line = e.span().map_or(String::new(), |s| format!("line {}: ", line_of(text, s.start)));
        format!("{line}{}", e.message().trim())
    })?;
    hoist(&mut table, text)?;
    Config::deserialize(table.clone()).map_err(|e| {
        // Deserializing a table forgets where things were: try each setting alone to find the one.
        let culprit = table.iter().find(|(k, v)| {
            let alone: toml::Table = [((*k).clone(), (*v).clone())].into_iter().collect();
            Config::deserialize(alone).is_err()
        });
        match culprit.and_then(|(k, _)| key_lines(text, k).first().map(|l| (k, *l))) {
            Some((k, line)) => format!("line {line}, {k}: {}", e.message().trim()),
            None => e.message().trim().to_string(),
        }
    })
}

/// Moves top-level settings that landed in a section (`[runners]`, an `[[actions]]` entry)
/// back to the top.
fn hoist(table: &mut toml::Table, text: &str) -> Result<(), String> {
    let mut moved = Vec::new();
    for (section, value) in table.iter_mut() {
        let inner: Vec<&mut toml::Table> = match value {
            toml::Value::Table(t) => vec![t],
            toml::Value::Array(items) => items.iter_mut().filter_map(toml::Value::as_table_mut).collect(),
            _ => continue,
        };
        for t in inner {
            for key in TOP_LEVEL {
                if let Some(v) = t.remove(key) {
                    moved.push((key, v, section.clone()));
                }
            }
        }
    }
    for (key, value, section) in moved {
        if table.contains_key(key) {
            let lines: Vec<String> = key_lines(text, key).iter().map(usize::to_string).collect();
            return Err(format!("{key} is set twice (lines {}), once under [{section}]", lines.join(" and ")));
        }
        table.insert(key.to_string(), value);
    }
    Ok(())
}

/// The 1-based line of byte `pos`.
fn line_of(text: &str, pos: usize) -> usize {
    text[..pos.min(text.len())].matches('\n').count() + 1
}

/// The lines that set `key`, or open it as a `[key]` / `[[key]]` section.
fn key_lines(text: &str, key: &str) -> Vec<usize> {
    let sets = |l: &str| {
        let l = l.trim_start();
        l.strip_prefix(key).is_some_and(|rest| rest.trim_start().starts_with('='))
            || l.starts_with(&format!("[{key}]"))
            || l.starts_with(&format!("[[{key}]]"))
    };
    text.lines().enumerate().filter(|(_, l)| sets(l)).map(|(i, _)| i + 1).collect()
}

impl Config {
    /// The shell the `shell` setting names.
    pub fn shell(&self) -> crate::shell::Shell {
        crate::shell::Shell::new(&self.shell)
    }

    pub fn install_dir(&self) -> PathBuf {
        expand_tilde(&self.install_dir)
    }
}

pub use crate::platform::{cache_dir, home, state_dir};

/// A file name for things kept per project, e.g. `~/Projekte/Games/bash_emu` →
/// `Projekte%2FGames%2Fbash_emu`: readable, and no two folders share one. On Windows
/// `\` is a separator too (also written `%2F`), and `D:` outside the profile becomes `D%3A`.
pub fn path_key(root: &Path) -> String {
    let home = home();
    let rel = root.strip_prefix(&home).map_or_else(|_| root.to_string_lossy(), |r| r.to_string_lossy());
    let rel = rel.trim_matches(is_separator);
    if rel.is_empty() {
        return "~".to_string();
    }
    let mut key = String::with_capacity(rel.len());
    for c in rel.chars() {
        match c {
            '%' => key.push_str("%25"),
            c if is_separator(c) => key.push_str("%2F"),
            // Not allowed in Windows file names.
            ':' | '<' | '>' | '"' | '|' | '?' | '*' if cfg!(windows) => key.push_str(&format!("%{:02X}", c as u32)),
            c => key.push(c),
        }
    }
    key
}

fn is_separator(c: char) -> bool {
    c == '/' || (cfg!(windows) && c == '\\')
}

pub fn expand_tilde(s: &str) -> PathBuf {
    match s.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with(is_separator) => {
            home().join(rest.trim_start_matches(is_separator))
        }
        _ => PathBuf::from(s),
    }
}

/// The last part of `p` (a folder's name), or "" for `/`.
pub fn dir_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Shows a path with $HOME abbreviated to `~`.
pub fn pretty_path(p: &Path) -> String {
    let home = home();
    match p.strip_prefix(&home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".into(),
        Ok(rest) => format!("~{}{}", std::path::MAIN_SEPARATOR, rest.display()),
        Err(_) => p.display().to_string(),
    }
}

/// Whether `s` is a key spec like `F9` or `ctrl-d`.
pub fn valid_key(s: &str) -> bool {
    parse_key(s).is_some()
}

fn parse_key(s: &str) -> Option<(KeyCode, KeyModifiers)> {
    let mut mods = KeyModifiers::NONE;
    let mut rest = s;
    loop {
        let lower = rest.to_ascii_lowercase();
        if let Some(r) = lower.strip_prefix("ctrl-").or(lower.strip_prefix("c-")) {
            mods |= KeyModifiers::CONTROL;
            rest = &rest[rest.len() - r.len()..];
        } else if let Some(r) = lower.strip_prefix("alt-").or(lower.strip_prefix("a-")) {
            mods |= KeyModifiers::ALT;
            rest = &rest[rest.len() - r.len()..];
        } else {
            break;
        }
    }
    let code = match rest {
        _ if rest.chars().count() == 1 => KeyCode::Char(rest.chars().next()?),
        _ => {
            let lower = rest.to_ascii_lowercase();
            match lower.as_str() {
                "enter" => KeyCode::Enter,
                "tab" => KeyCode::Tab,
                "space" => KeyCode::Char(' '),
                "esc" => KeyCode::Esc,
                "delete" | "del" => KeyCode::Delete,
                "insert" | "ins" => KeyCode::Insert,
                f if f.starts_with('f') => KeyCode::F(f[1..].parse().ok()?),
                _ => return None,
            }
        }
    };
    // Ctrl+letter arrives lowercase from the terminal.
    let code = match code {
        KeyCode::Char(c) if mods.contains(KeyModifiers::CONTROL) => KeyCode::Char(c.to_ascii_lowercase()),
        c => c,
    };
    Some((code, mods))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every setting in the template is one Mainframe moves back to the top, and the other way round.
    #[cfg(windows)]
    #[test]
    fn path_keys_are_valid_windows_file_names() {
        let h = home();
        assert_eq!(path_key(&h.join(r"Projekte\Mainframe")), "Projekte%2FMainframe");
        assert_eq!(path_key(Path::new(r"D:\code\app")), "D%3A%2Fcode%2Fapp");
        assert_eq!(path_key(&h), "~");
        assert_eq!(expand_tilde(r"~\bin"), h.join("bin"));
    }

    #[test]
    fn top_level_matches_the_template() {
        let t: toml::Table = TEMPLATE.parse().unwrap();
        let mut in_template: Vec<&str> = t.iter().filter(|(_, v)| !v.is_table()).map(|(k, _)| k.as_str()).collect();
        let mut known = TOP_LEVEL.to_vec();
        in_template.sort();
        known.sort();
        assert_eq!(in_template, known);
        assert!(parse(TEMPLATE).is_ok());
    }

    /// A setting below [runners] or in an [[actions]] entry still counts, and doesn't break them.
    #[test]
    fn settings_work_below_a_section() {
        let text = "editor = \"vim\"\n[runners]\npy = \"python3 {file}\"\nprune_gone_branches = true\n\n\
                    [[actions]]\nkey = \"F5\"\nname = \"x\"\ncmd = \"x\"\nnotify_after = 3\n";
        let cfg = parse(text).unwrap();
        assert!(cfg.prune_gone_branches);
        assert_eq!(cfg.notify_after, 3);
        assert_eq!(cfg.editor, "vim");
        assert_eq!(cfg.runners.len(), 1);
        assert_eq!(cfg.actions.len(), 1);
    }

    #[test]
    fn errors_say_the_line() {
        let wrong_type = "editor = \"vim\"\n\nnotify_after = \"soon\"\n";
        assert!(parse(wrong_type).unwrap_err().starts_with("line 3, notify_after: invalid type"));
        let moved_wrong = "[runners]\npy = \"x\"\nshow_hidden = \"yes\"\n";
        assert!(parse(moved_wrong).unwrap_err().starts_with("line 3, show_hidden:"));
        let broken = "editor = \"vim\"\nlist = [\n";
        assert!(parse(broken).unwrap_err().starts_with("line 2"), "{}", parse(broken).unwrap_err());
        let twice = "zoxide = true\n[runners]\nzoxide = false\n";
        assert_eq!(parse(twice).unwrap_err(), "zoxide is set twice (lines 1 and 3), once under [runners]");
        let bad_runner = "[runners]\npy = 3\n";
        assert!(parse(bad_runner).unwrap_err().starts_with("line 1, runners:"));
    }
}
