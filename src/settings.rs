//! Per-project settings in `$XDG_CONFIG_HOME/mainframe/projects/<project>.toml`, kept outside
//! the project like the notes, so host names never end up in a commit. They hold deploy
//! targets (see `deploy`) and project-only custom actions; deploy targets added or edited
//! in the TUI are written back here without touching the rest of the file.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use ratatui::crossterm::event::KeyEvent;
use serde::Deserialize;

use crate::{
    config::{self, Action, Config, path_key},
    deploy::Deploy,
};

pub const TEMPLATE: &str = r#"# Mainframe settings for {project}
#
# Deploy targets (D): build, copy the result to another machine over ssh and swap it in
# for the binary there. The old one is kept next to it as <remote_path>.bak-<date-time>.
#
# [[deploy]]
# name = "pi"
# host = "pi@raspberrypi.local"          # anything ssh accepts, incl. Host aliases from ~/.ssh/config
# remote_path = "/usr/local/bin/myapp"   # replaced in place, mode and owner kept
# build = "cross build --release --target aarch64-unknown-linux-gnu"   # runs in the project root; "" skips it
# from = "main"           # optional: build this branch (or commit) in a separate checkout, not the
#                         # folder as it is; keep_deploy_builds in the main config keeps that checkout
# artifact = "target/aarch64-unknown-linux-gnu/release/myapp"           # relative to the project root
# sudo = true             # replace it with sudo (needed for /usr/local/bin and the like)
# service = "myapp"       # optional systemd service: restarted after each deploy and checked that
#                         # it runs; a minimal unit is created if there's none (never overwritten)
# restart = "curl -fsS localhost:8080/health"   # optional, any command run on the remote afterwards
# keep = 3                # backups to keep on the remote; 0 keeps all
# ssh = ""                # extra ssh options, e.g. "-p 2222 -i ~/.ssh/pi"
# interactive = false     # true: runs in the real terminal, for ssh / sudo passwords
# key = "F9"              # optional: deploys with one key, no picker
#
# Custom actions only for this project, same as in the main config; they win over those.
#
# [[actions]]
# key = "F6"
# name = "logs on the pi"
# cmd = "ssh pi@raspberrypi.local journalctl -fu myapp"
"#;

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ProjectSettings {
    pub deploy: Vec<Deploy>,
    pub actions: Vec<Action>,
}

impl ProjectSettings {
    /// The settings of the project at `root`, plus an error when the file is broken.
    pub fn load(root: &Path) -> (Self, Option<String>) {
        let path = file_for(root);
        let Ok(text) = fs::read_to_string(&path) else {
            return (Self::default(), None);
        };
        match toml::from_str::<Self>(&text) {
            Ok(s) => match s.problem() {
                Some(p) => (s, Some(format!("{}: {p}", config::pretty_path(&path)))),
                None => (s, None),
            },
            Err(e) => (Self::default(), Some(format!("{}: {}", config::pretty_path(&path), e.message()))),
        }
    }

    fn problem(&self) -> Option<String> {
        self.deploy.iter().find_map(|d| d.problem().map(|p| format!("deploy {:?}: {p}", d.name)))
    }

    pub fn deploy_for_key(&self, key: &KeyEvent) -> Option<&Deploy> {
        self.deploy.iter().find(|d| !d.key.is_empty() && config::key_matches(&d.key, key))
    }
}

pub fn dir() -> PathBuf {
    Config::path().parent().map_or_else(|| PathBuf::from("."), Path::to_path_buf).join("projects")
}

/// `~/Projekte/Games/bash_emu` → `projects/Projekte%2FGames%2Fbash_emu.toml`.
pub fn file_for(root: &Path) -> PathBuf {
    dir().join(format!("{}.toml", path_key(root)))
}

/// Writes the commented template for `root` unless the file exists already.
pub fn ensure_file(root: &Path) -> io::Result<PathBuf> {
    let path = file_for(root);
    if !path.exists() {
        fs::create_dir_all(dir())?;
        fs::write(&path, template(root))?;
    }
    Ok(path)
}

fn template(root: &Path) -> String {
    TEMPLATE.replace("{project}", &config::pretty_path(root))
}

/// Appends `d` as a `[[deploy]]` table, creating the file (with the template) if needed.
/// Only non-default values are written, so the file stays short.
pub fn append_deploy(root: &Path, d: &Deploy) -> io::Result<PathBuf> {
    let path = ensure_file(root)?;
    let mut text = fs::read_to_string(&path)?;
    if !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(&render_deploy(d));
    fs::write(&path, text)?;
    Ok(path)
}

/// Puts `d` in place of the `idx`th target in the file; the rest of the file, comments
/// included, stays as it is.
pub fn replace_deploy(root: &Path, idx: usize, d: &Deploy) -> io::Result<PathBuf> {
    rewrite_deploy(root, idx, Some(d))
}

pub fn remove_deploy(root: &Path, idx: usize) -> io::Result<PathBuf> {
    rewrite_deploy(root, idx, None)
}

fn rewrite_deploy(root: &Path, idx: usize, d: Option<&Deploy>) -> io::Result<PathBuf> {
    let path = file_for(root);
    let text = fs::read_to_string(&path)?;
    let new = with_deploy(&text, idx, d).ok_or_else(|| {
        io::Error::other(format!("couldn't find that target in {} — E edits the file", config::pretty_path(&path)))
    })?;
    fs::write(&path, new)?;
    Ok(path)
}

/// `text` with its `idx`th `[[deploy]]` table replaced by `d`, or removed. None when the
/// tables can't be told apart safely, or the result wouldn't read back as intended.
fn with_deploy(text: &str, idx: usize, d: Option<&Deploy>) -> Option<String> {
    let before: ProjectSettings = toml::from_str(text).ok()?;
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let header = |l: &str| l.trim_start().starts_with('[');
    let starts: Vec<usize> = (0..lines.len())
        .filter(|&i| {
            let t = lines[i].trim();
            t == "[[deploy]]" || t.strip_prefix("[[deploy]]").is_some_and(|r| r.trim_start().starts_with('#'))
        })
        .collect();
    if starts.len() != before.deploy.len() {
        return None;
    }
    let start = *starts.get(idx)?;
    let mut end = (start + 1..lines.len()).find(|&i| header(lines[i])).unwrap_or(lines.len());
    // Comments and blank lines just above the next table belong to that one.
    while end > start + 1 && {
        let t = lines[end - 1].trim();
        t.is_empty() || t.starts_with('#')
    } {
        end -= 1;
    }
    let mut out: String = lines[..start].concat();
    if let Some(d) = d {
        out += render_deploy(d).trim_start_matches('\n');
    } else {
        // Drop the blank line that separated it, too.
        while out.ends_with("\n\n") {
            out.pop();
        }
    }
    out += &lines[end..].concat();

    let after: ProjectSettings = toml::from_str(&out).ok()?;
    let mut want = before.deploy;
    match d {
        Some(d) => want[idx] = d.clone(),
        None => {
            want.remove(idx);
        }
    }
    (after.deploy == want && after.actions.len() == before.actions.len()).then_some(out)
}

fn render_deploy(d: &Deploy) -> String {
    let s = |v: &str| toml::Value::String(v.to_string()).to_string();
    let def = Deploy::default();
    let mut out = String::from("\n[[deploy]]\n");
    out += &format!("name = {}\n", s(&d.name));
    out += &format!("host = {}\n", s(&d.host));
    out += &format!("remote_path = {}\n", s(&d.remote_path));
    out += &format!("build = {}\n", s(&d.build));
    if !d.from.is_empty() {
        out += &format!("from = {}\n", s(&d.from));
    }
    out += &format!("artifact = {}\n", s(&d.artifact));
    out += &format!("sudo = {}\n", d.sudo);
    for (k, v, dv) in [
        ("service", &d.service, &def.service),
        ("restart", &d.restart, &def.restart),
        ("ssh", &d.ssh, &def.ssh),
        ("key", &d.key, &def.key),
    ] {
        if v != dv {
            out += &format!("{k} = {}\n", s(v));
        }
    }
    if d.keep != def.keep {
        out += &format!("keep = {}\n", d.keep);
    }
    if d.interactive {
        out += "interactive = true\n";
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pi() -> Deploy {
        Deploy {
            name: "pi".into(),
            host: "pi@raspberrypi.local".into(),
            remote_path: "/usr/local/bin/my app".into(),
            build: "cargo build --release && echo built".into(),
            artifact: "target/aarch64-unknown-linux-gnu/release/myapp".into(),
            sudo: true,
            restart: "sudo systemctl restart myapp".into(),
            ..Deploy::default()
        }
    }

    #[test]
    fn appended_tables_parse_back() {
        let d = Deploy { key: "F9".into(), keep: 5, interactive: true, ..pi() };
        let text =
            template(Path::new("/x")) + &render_deploy(&d) + &render_deploy(&Deploy { name: "zero".into(), ..pi() });
        let s: ProjectSettings = toml::from_str(&text).unwrap();
        assert_eq!(s.deploy, vec![d, Deploy { name: "zero".into(), ..pi() }]);
        assert!(s.problem().is_none());
    }

    #[test]
    fn edits_one_table_and_keeps_the_rest() {
        let a = Deploy { name: "a".into(), ..pi() };
        let b = Deploy { name: "b".into(), service: "myapp".into(), ..pi() };
        let text = template(Path::new("/x"))
            + &render_deploy(&a)
            + "# about b\n"
            + &render_deploy(&b)
            + "\n[[actions]]\nkey = \"F6\"\nname = \"logs\"\ncmd = \"true\"\n";
        let b2 = Deploy { host: "other".into(), keep: 0, key: "F9".into(), ..b.clone() };
        let edited = with_deploy(&text, 1, Some(&b2)).unwrap();
        assert!(
            edited.contains("# about b\n\n[[deploy]]\nname = \"b\"\nhost = \"other\"")
                && edited.contains("# Mainframe settings")
        );
        let s: ProjectSettings = toml::from_str(&edited).unwrap();
        assert_eq!(s.deploy, vec![a.clone(), b2]);
        assert_eq!(s.actions.len(), 1);

        let removed = with_deploy(&text, 0, None).unwrap();
        let s: ProjectSettings = toml::from_str(&removed).unwrap();
        assert_eq!(s.deploy, vec![b.clone()]);
        assert!(removed.contains("# about b"));
        assert!(with_deploy(&text, 2, None).is_none());
        // Tables written some other way aren't guessed at.
        assert!(with_deploy("deploy = [{ name = \"a\", host = \"h\" }]\n", 0, None).is_none());
    }

    #[test]
    fn template_parses_empty() {
        let s: ProjectSettings = toml::from_str(&template(Path::new("/x"))).unwrap();
        assert!(s.deploy.is_empty() && s.actions.is_empty());
    }
}
