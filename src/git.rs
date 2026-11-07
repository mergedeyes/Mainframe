//! Git status for the header and per-file markers, plus the git menu's commands.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use ratatui::style::Color;

use crate::form::{Field, Form, FormKind};

pub struct GitInfo {
    pub root: PathBuf,
    /// This worktree's git dir (HEAD, index) and the one shared by all worktrees (refs).
    git_dir: PathBuf,
    common_dir: PathBuf,
    pub branch: String,
    pub upstream: bool,
    pub ahead: u32,
    pub behind: u32,
    /// Absolute path → porcelain XY code.
    files: HashMap<PathBuf, [u8; 2]>,
    /// Directories that contain changes somewhere below them.
    dirty_dirs: HashSet<PathBuf>,
}

impl GitInfo {
    /// Files git rewrites when HEAD, the index or a branch moves (commit, checkout,
    /// reset, fetch, push, pull). Watched together with `refs_dir`.
    pub fn state_files(&self) -> Vec<PathBuf> {
        let (g, c) = (&self.git_dir, &self.common_dir);
        vec![g.join("HEAD"), g.join("index"), g.join("logs/HEAD"), g.join("FETCH_HEAD"), c.join("packed-refs")]
    }

    pub fn refs_dir(&self) -> PathBuf {
        self.common_dir.join("refs")
    }

    pub fn dirty(&self) -> bool {
        !self.files.is_empty()
    }

    /// Tracked and changed, so `git diff` has something to show.
    pub fn modified(&self, path: &Path) -> bool {
        self.files.get(path).is_some_and(|xy| xy != b"??")
    }

    /// Marker shown next to an entry in the file list.
    pub fn mark(&self, path: &Path, is_dir: bool) -> Option<(&'static str, Color)> {
        if let Some(&[x, y]) = self.files.get(path) {
            return Some(match (x, y) {
                (b'?', b'?') => ("?", Color::Magenta),
                (b'U', _) | (_, b'U') | (b'A', b'A') | (b'D', b'D') => ("!", Color::Red),
                (_, b'D') | (b'D', b' ') => ("D", Color::Red),
                // Staged and clean in the worktree.
                (b'A', b' ') => ("A", Color::Green),
                (b'R', b' ') => ("R", Color::Green),
                (_, b' ') => ("M", Color::Green),
                _ => ("M", Color::Yellow),
            });
        }
        (is_dir && self.dirty_dirs.contains(path)).then_some(("•", Color::Yellow))
    }
}

fn git(cwd: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status.success().then_some(out.stdout)
}

/// Staged and unstaged changes to one file, without the `diff --git` header.
pub fn diff(path: &Path) -> Option<Vec<String>> {
    let dir = path.parent()?;
    let p = path.to_str()?;
    // A repo without commits has no HEAD to diff against.
    let raw = git(dir, &["diff", "--no-color", "--no-ext-diff", "HEAD", "--", p])
        .or_else(|| git(dir, &["diff", "--no-color", "--no-ext-diff", "--cached", "--", p]))?;
    let text = String::from_utf8_lossy(&raw);
    let lines: Vec<String> = text
        .lines()
        .skip_while(|l| !l.starts_with("@@") && !l.starts_with("Binary"))
        .map(|l| l.replace('\t', "    "))
        .collect();
    (!lines.is_empty()).then_some(lines)
}

pub fn status(cwd: &Path) -> Option<GitInfo> {
    let dirs = git(cwd, &["rev-parse", "--show-toplevel", "--absolute-git-dir", "--git-common-dir"])?;
    let dirs = String::from_utf8_lossy(&dirs);
    let mut dirs = dirs.lines().map(PathBuf::from);
    let (root, git_dir) = (dirs.next()?, dirs.next()?);
    // Printed relative to `cwd` unless it's elsewhere (worktrees).
    let common_dir = cwd.join(dirs.next()?);
    let raw = git(cwd, &["status", "--porcelain=v1", "-b", "-z", "-unormal"])?;
    let raw = String::from_utf8_lossy(&raw);

    let mut info = GitInfo {
        root,
        git_dir,
        common_dir,
        branch: String::new(),
        upstream: false,
        ahead: 0,
        behind: 0,
        files: HashMap::new(),
        dirty_dirs: HashSet::new(),
    };

    let mut records = raw.split('\0').filter(|r| !r.is_empty());
    while let Some(rec) = records.next() {
        if let Some(head) = rec.strip_prefix("## ") {
            parse_branch(&mut info, head);
            continue;
        }
        let bytes = rec.as_bytes();
        if bytes.len() < 4 {
            continue;
        }
        let xy = [bytes[0], bytes[1]];
        if matches!(xy[0], b'R' | b'C') {
            records.next(); // original path of a rename/copy
        }
        let path = info.root.join(rec[3..].trim_end_matches('/'));
        for dir in path.ancestors().skip(1) {
            if !dir.starts_with(&info.root) || dir == info.root {
                break;
            }
            info.dirty_dirs.insert(dir.to_path_buf());
        }
        info.files.insert(path, xy);
    }
    Some(info)
}

fn parse_branch(info: &mut GitInfo, head: &str) {
    // "main...origin/main [ahead 1, behind 2]" | "main" | "No commits yet on main" | "HEAD (no branch)"
    let head = head.strip_prefix("No commits yet on ").unwrap_or(head);
    let (names, counts) = match head.split_once(" [") {
        Some((n, c)) => (n, c.trim_end_matches(']')),
        None => (head, ""),
    };
    let (branch, upstream) = match names.split_once("...") {
        Some((b, _)) => (b, true),
        None => (names, false),
    };
    info.branch = if branch.starts_with("HEAD") { "detached".into() } else { branch.into() };
    info.upstream = upstream;
    for part in counts.split(", ") {
        if let Some(n) = part.strip_prefix("ahead ") {
            info.ahead = n.parse().unwrap_or(0);
        } else if let Some(n) = part.strip_prefix("behind ") {
            info.behind = n.parse().unwrap_or(0);
        }
    }
}

/// Local branches, then remote ones not checked out locally (without the remote prefix).
pub fn branches(cwd: &Path) -> Vec<String> {
    let Some(raw) = git(cwd, &["for-each-ref", "--format=%(refname)", "refs/heads", "refs/remotes"]) else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    for r in String::from_utf8_lossy(&raw).lines() {
        let name = if let Some(b) = r.strip_prefix("refs/heads/") {
            b
        } else if let Some(rest) = r.strip_prefix("refs/remotes/") {
            match rest.split_once('/') {
                Some((_, "HEAD")) | None => continue,
                Some((_, b)) => b,
            }
        } else {
            continue;
        };
        if !out.iter().any(|o| o == name) {
            out.push(name.to_string());
        }
    }
    out
}

/// `git` with colors forced on, since the output pane isn't a tty.
pub const GIT: &str = "git -c color.ui=always";

pub struct MenuItem {
    pub key: char,
    pub label: &'static str,
}

const fn item(key: char, label: &'static str) -> MenuItem {
    MenuItem { key, label }
}

/// Grouped for display; the app dispatches on `key`.
pub const MENU: &[(&str, &[MenuItem])] = &[
    (
        "inspect",
        &[
            item('s', "status"),
            item('d', "diff"),
            item('D', "diff staged"),
            item('l', "log graph"),
            item('L', "log of selected file"),
            item('w', "blame selected"),
        ],
    ),
    (
        "stage & commit",
        &[
            item('a', "stage selected"),
            item('A', "stage all"),
            item('u', "unstage selected"),
            item('c', "commit staged…"),
            item('C', "stage all + commit…"),
            item('e', "amend (editor)"),
            item('x', "discard changes to selected"),
        ],
    ),
    (
        "sync",
        &[
            item('f', "fetch"),
            item('p', "pull"),
            item('P', "push"),
        ],
    ),
    (
        "branches",
        &[
            item('b', "list branches"),
            item('o', "switch to branch…"),
            item('n', "new branch…"),
            item('z', "stash"),
            item('Z', "stash pop"),
        ],
    ),
    (
        "other",
        &[
            item('i', "init repo here"),
            item('h', "create GitHub repo… (gh)"),
            item('g', "lazygit / gitui / tig"),
        ],
    ),
];

// ── gh repo create ────────────────────────────────────────────────────────

/// Builds the "create GitHub repo" form with everything prefilled for `source`.
pub fn gh_repo_form(source: &Path) -> Result<Form, String> {
    if crate::project::which("gh").is_none() {
        return Err("gh (GitHub CLI) is not installed".into());
    }
    let owner = Command::new("gh")
        .args(["api", "user", "--jq", ".login"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    let Some(owner) = owner else {
        return Err("gh isn't logged in — run `gh auth login` (e.g. via !)".into());
    };

    let is_repo = git(source, &["rev-parse", "--git-dir"]).is_some();
    let has_commits = is_repo && git(source, &["rev-parse", "--verify", "HEAD"]).is_some();
    let has_origin = is_repo && git(source, &["remote", "get-url", "origin"]).is_some();
    let name: String = source
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
        .chars()
        .map(|c| if c.is_alphanumeric() || "-_.".contains(c) { c } else { '-' })
        .collect();

    let note = if !is_repo {
        Some("not a git repo yet — will git init + make an initial commit first".to_string())
    } else if !has_commits {
        Some("no commits yet — nothing to push until you commit".to_string())
    } else if has_origin {
        Some("this repo already has an 'origin' remote — pick another remote name".to_string())
    } else {
        None
    };

    Ok(Form {
        kind: FormKind::GhRepoCreate,
        title: " create GitHub repo ".into(),
        fields: vec![
            Field::text("owner", owner, "your user or an org"),
            Field::text("name", name, "repository name"),
            Field::text("description", "", "optional"),
            Field::choice("visibility", &["private", "public", "internal"], 0, "←/→ to change"),
            Field::dir("source", source.to_string_lossy(), "local folder to link · Tab completes"),
            Field::text("remote", if has_origin { "github" } else { "origin" }, "remote name to add"),
            Field::choice("push", &["yes", "no"], if is_repo && !has_commits { 1 } else { 0 }, "push current branch"),
        ],
        step: 0,
        note,
        completer: None,
    })
}

/// The shell command the form currently describes; runs from `source`.
pub fn gh_repo_create_cmd(form: &Form) -> String {
    let q = crate::project::quote_str;
    let source = crate::config::expand_tilde(form.get("source"));
    let owner = form.get("owner").trim();
    let name = form.get("name").trim();
    let repo = if owner.is_empty() { name.to_string() } else { format!("{owner}/{name}") };

    let mut cmd = String::new();
    if git(&source, &["rev-parse", "--git-dir"]).is_none() {
        cmd.push_str("git init && git add -A && git commit -m 'Initial commit' && ");
    }
    cmd.push_str(&format!("gh repo create {} --{} --source=. --remote={}", q(&repo), form.get("visibility"), q(form.get("remote"))));
    let desc = form.get("description").trim();
    if !desc.is_empty() {
        cmd.push_str(&format!(" --description {}", q(desc)));
    }
    if form.get("push") == "yes" {
        cmd.push_str(" --push");
    }
    cmd
}
