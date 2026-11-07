//! The projects folder (`~/Projekte`, `~/Projects`, … or `projects_dir` in the config):
//! which folders in it are projects, for the `J` picker and for whose notes `M` opens.

use std::{
    fs,
    path::{Path, PathBuf},
    time::SystemTime,
};

use crate::{
    config::{self, Config},
    git::GitInfo,
    project::{self, Project},
    watch,
};

/// Tried in order when `projects_dir` isn't set.
const CANDIDATES: &[&str] = &["Projekte", "Projects", "projects", "Code", "code", "dev", "src", "repos"];
/// `Games/bash_emu` is still found, `Games/2d/bash_emu` too; deeper is a project's own layout.
const MAX_DEPTH: usize = 3;
/// Files that don't stop a folder from being a group of projects.
const WORKSPACE: &[&str] = &[".sublime-project", ".sublime-workspace", ".code-workspace"];

#[derive(Debug, Clone)]
pub struct Entry {
    pub path: PathBuf,
    /// Relative to the projects folder, e.g. `Games/bash_emu`.
    pub name: String,
    /// `cargo`, `npm`, `godot`, `git`, … or empty for a plain folder.
    pub kind: &'static str,
    /// Newest change among its top-level files and the git index / reflog.
    pub active: Option<SystemTime>,
}

pub fn dir(cfg: &Config) -> Option<PathBuf> {
    if !cfg.projects_dir.trim().is_empty() {
        return Some(config::expand_tilde(cfg.projects_dir.trim())).filter(|d| d.is_dir());
    }
    let home = config::home();
    CANDIDATES.iter().map(|c| home.join(c)).find(|d| d.is_dir())
}

/// Every project under `root`, most recently active first.
pub fn scan(root: &Path) -> Vec<Entry> {
    let mut out = Vec::new();
    for child in subdirs(root) {
        collect(root, &child, 1, &mut out);
    }
    out.sort_by(|a, b| b.active.cmp(&a.active).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    out
}

/// A folder is a project if it has a marker (`.git`, `Cargo.toml`, …). A folder without one
/// that holds nothing but folders (and editor workspace files) is a group of projects when
/// a child has a marker (`Games/`, `Websites/`), or at the top when it has no workspace file
/// of its own (`LLMs/`). Anything else is a project too, just one without a build tool:
/// files of its own (`Memory-Desktop/ROADMAP.md`) make a vendored child part of it.
fn collect(root: &Path, dir: &Path, depth: usize, out: &mut Vec<Entry>) {
    let children = subdirs(dir);
    let group = kind(dir).is_none()
        && depth < MAX_DEPTH
        && !children.is_empty()
        && match only_dirs(dir) {
            Some(workspace) => children.iter().any(|c| kind(c).is_some()) || (depth == 1 && !workspace),
            None => false,
        };
    if group {
        for c in &children {
            collect(root, c, depth + 1, out);
        }
    } else {
        out.push(Entry {
            name: dir.strip_prefix(root).unwrap_or(dir).to_string_lossy().into_owned(),
            kind: kind(dir).unwrap_or(""),
            active: activity(dir),
            path: dir.to_path_buf(),
        });
    }
}

fn kind(dir: &Path) -> Option<&'static str> {
    if let Some(k) = project::kind_at(dir) {
        return Some(k.label());
    }
    if dir.join("project.godot").is_file() {
        return Some("godot");
    }
    dir.join(".git").exists().then_some("git")
}

fn subdirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = fs::read_dir(dir) else { return Vec::new() };
    let mut dirs: Vec<PathBuf> = rd
        .flatten()
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            !name.starts_with('.') && !watch::SKIP.contains(&name.as_ref()) && e.path().is_dir()
        })
        .map(|e| e.path())
        .collect();
    dirs.sort();
    dirs
}

/// `Some(has a workspace file)` when `dir` holds only folders, dotfiles and editor
/// workspace files (`*.sublime-project`, `*.code-workspace`), else None.
fn only_dirs(dir: &Path) -> Option<bool> {
    let mut workspace = false;
    for e in fs::read_dir(dir).ok()?.flatten() {
        let name = e.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || e.path().is_dir() {
            continue;
        }
        if WORKSPACE.iter().any(|ext| name.ends_with(ext)) {
            workspace = true;
        } else {
            return None;
        }
    }
    Some(workspace)
}

fn activity(dir: &Path) -> Option<SystemTime> {
    let mtime = |p: &Path| fs::metadata(p).and_then(|m| m.modified()).ok();
    let top = fs::read_dir(dir).into_iter().flatten().flatten().filter_map(|e| e.metadata().ok()?.modified().ok());
    let git = [".git/index", ".git/logs/HEAD"].into_iter().filter_map(|f| mtime(&dir.join(f)));
    top.chain(git).chain(mtime(dir)).max()
}

/// The project `cwd` belongs to (whose notes `M` opens, whose output tabs are shown):
/// the listed project it's in, else the git repo, else the build tool's project.
/// None outside of any, e.g. in `~`, `~/Downloads` or a group folder like `~/Projekte/Games`.
pub fn owner(cwd: &Path, projects: Option<&Path>, git: Option<&GitInfo>, project: Option<&Project>) -> Option<PathBuf> {
    if let Some(root) = projects
        && cwd.starts_with(root)
        && cwd != root
        && let Some(e) = scan_owner(root, cwd)
    {
        return Some(e);
    }
    // A dotfiles repo at $HOME doesn't make everything below it one project.
    let home = config::home();
    git.map(|g| g.root.clone()).filter(|r| *r != home).or_else(|| project.map(|p| p.root.clone()))
}

/// Like `scan`, but only walks down the branch that holds `cwd`.
fn scan_owner(root: &Path, cwd: &Path) -> Option<PathBuf> {
    let rel = cwd.strip_prefix(root).ok()?;
    let mut dir = root.to_path_buf();
    for (depth, part) in rel.components().enumerate() {
        dir.push(part);
        let mut found = Vec::new();
        collect(root, &dir, depth + 1, &mut found);
        // Not a group: this folder is the project.
        if found.len() != 1 || found[0].path != dir {
            if found.iter().any(|e| cwd.starts_with(&e.path)) {
                continue;
            }
            return None;
        }
        return Some(dir);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(name: &str, files: &[&str]) -> PathBuf {
        let root = std::env::temp_dir().join(format!("mainframe-projects-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for f in files {
            let p = root.join(f);
            if f.ends_with('/') {
                fs::create_dir_all(&p).unwrap();
            } else {
                fs::create_dir_all(p.parent().unwrap()).unwrap();
                fs::write(&p, "").unwrap();
            }
        }
        root
    }

    #[test]
    fn groups_open_up_into_their_projects() {
        let root = tree(
            "scan",
            &[
                "Mainframe/Cargo.toml",
                "Mainframe/src/main.rs",
                "Games/bash_emu/Cargo.toml",
                "Games/Sim/project.godot",
                "Games/maps_only/maps/",
                "LLMs/seafind/train.py",
                "Notes/README.md",
                "Notes/old/",
                "Desktop/ROADMAP.md",
                "Desktop/vendored/Cargo.toml",
                "Sites/sites.sublime-project",
                "Sites/blog/.git/",
                "Clicker/clicker.sublime-project",
                "Clicker/win/",
            ],
        );
        let mut names: Vec<_> = scan(&root).into_iter().map(|e| (e.name, e.kind)).collect();
        names.sort();
        assert_eq!(
            names,
            [
                ("Clicker".into(), ""),
                ("Desktop".into(), ""),
                ("Games/Sim".into(), "godot"),
                ("Games/bash_emu".into(), "cargo"),
                ("Games/maps_only".into(), ""),
                ("LLMs/seafind".into(), ""),
                ("Mainframe".into(), "cargo"),
                ("Notes".into(), ""),
                ("Sites/blog".into(), "git"),
            ]
        );

        let owner = |p: &str| scan_owner(&root, &root.join(p));
        assert_eq!(owner("Mainframe/src"), Some(root.join("Mainframe")));
        assert_eq!(owner("Games/bash_emu"), Some(root.join("Games/bash_emu")));
        assert_eq!(owner("Games/maps_only/maps"), Some(root.join("Games/maps_only")));
        assert_eq!(owner("Notes/old"), Some(root.join("Notes")));
        assert_eq!(owner("Desktop/vendored"), Some(root.join("Desktop")));
        assert_eq!(owner("Games"), None);
        let _ = fs::remove_dir_all(&root);
    }
}
