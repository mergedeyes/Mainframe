use std::{
    collections::HashMap,
    env, fs,
    path::{Path, PathBuf},
};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::Deserialize;

pub const TEMPLATE: &str = r#"# Mainframe config
#
# Placeholders usable in commands (all shell-quoted automatically):
#   {file}  selected entry (absolute path)
#   {name}  selected entry's file name
#   {dir}   directory you're currently in
#   {root}  detected project root (falls back to {dir})

# Command used for `e` / Enter. Anything that blocks until you close it works.
editor = "nano"

# Command used for `o` (file) and `O` (folder).
vscode = "code"

# Where `i` copies standalone executables to.
install_dir = "~/.local/bin"

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
"#;

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Config {
    pub editor: String,
    pub vscode: String,
    pub install_dir: String,
    pub show_hidden: bool,
    pub preview: bool,
    pub zoxide: bool,
    pub desktop_entries: DesktopMode,
    pub notify_after: u64,
    pub runners: HashMap<String, String>,
    pub actions: Vec<Action>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            editor: "nano".into(),
            vscode: "code".into(),
            install_dir: "~/.local/bin".into(),
            show_hidden: true,
            preview: true,
            zoxide: true,
            desktop_entries: DesktopMode::Ask,
            notify_after: 10,
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
        parse_key(&self.key).is_some_and(|(code, mods)| {
            // Shift is implied by the character itself for letters/symbols.
            let pressed = key.modifiers - KeyModifiers::SHIFT;
            key.code == code && pressed == mods
        })
    }
}

impl Config {
    pub fn path() -> PathBuf {
        let base = env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home().join(".config"));
        base.join("mainframe").join("config.toml")
    }

    /// Returns the config plus an error message if the file exists but is broken.
    pub fn load() -> (Self, Option<String>) {
        let path = Self::path();
        let Ok(text) = fs::read_to_string(&path) else {
            return (Self::default(), None);
        };
        match toml::from_str(&text) {
            Ok(cfg) => (cfg, None),
            Err(e) => (Self::default(), Some(format!("config error: {}", e.message()))),
        }
    }

    pub fn install_dir(&self) -> PathBuf {
        expand_tilde(&self.install_dir)
    }
}

pub fn home() -> PathBuf {
    env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| "/".into())
}

pub fn expand_tilde(s: &str) -> PathBuf {
    match s.strip_prefix('~') {
        Some(rest) => home().join(rest.trim_start_matches('/')),
        None => PathBuf::from(s),
    }
}

/// Shows a path with $HOME abbreviated to `~`.
pub fn pretty_path(p: &Path) -> String {
    let home = home();
    match p.strip_prefix(&home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".into(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => p.display().to_string(),
    }
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
