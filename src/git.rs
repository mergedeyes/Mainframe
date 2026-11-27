//! Git status for the header and per-file markers, plus the git menu's commands.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    process::Command,
};

use ratatui::style::Color;

use crate::{
    cmd::{Cmd, Step},
    form::{Field, Form, FormKind},
    proc,
    project::quote_str,
};

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

/// stdout of a successful `git` run in `cwd`.
pub fn git(cwd: &Path, args: &[&str]) -> Option<Vec<u8>> {
    proc::output(Command::new("git").args(args).current_dir(cwd))
}

/// [`git`] as text, trimmed.
pub fn git_text(cwd: &Path, args: &[&str]) -> Option<String> {
    proc::text(Command::new("git").args(args).current_dir(cwd))
}

/// [`git`] as lines; none when it fails.
pub fn git_lines(cwd: &Path, args: &[&str]) -> Vec<String> {
    git(cwd, args).map(|o| String::from_utf8_lossy(&o).lines().map(String::from).collect()).unwrap_or_default()
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

/// A path git printed. Git for Windows writes `C:/Users/…`; Windows' own separator
/// keeps paths built from it from mixing both.
pub fn path_from_git(s: &str) -> PathBuf {
    if cfg!(windows) { PathBuf::from(s.replace('/', "\\")) } else { PathBuf::from(s) }
}

pub fn status(cwd: &Path) -> Option<GitInfo> {
    let dirs = git(cwd, &["rev-parse", "--show-toplevel", "--absolute-git-dir", "--git-common-dir"])?;
    let dirs = String::from_utf8_lossy(&dirs);
    let mut dirs = dirs.lines().map(path_from_git);
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
pub struct Worktree {
    pub path: PathBuf,
    pub branch: Option<String>,
    /// Its folder was deleted by hand.
    pub gone: bool,
}

impl Worktree {
    pub fn name(&self) -> String {
        self.path.file_name().map_or_else(|| self.path.display().to_string(), |n| n.to_string_lossy().into_owned())
    }
}

/// The repo's worktrees, the main one first.
pub fn worktrees(cwd: &Path) -> Vec<Worktree> {
    let Some(raw) = git(cwd, &["worktree", "list", "--porcelain"]) else { return Vec::new() };
    let mut out: Vec<Worktree> = Vec::new();
    for line in String::from_utf8_lossy(&raw).lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            out.push(Worktree { path: path_from_git(p), branch: None, gone: false });
        } else if let Some(w) = out.last_mut() {
            if let Some(b) = line.strip_prefix("branch ") {
                w.branch = Some(b.strip_prefix("refs/heads/").unwrap_or(b).to_string());
            } else if line.starts_with("prunable") {
                w.gone = true;
            }
        }
    }
    out
}

/// Modified and untracked files in a worktree.
pub fn uncommitted(path: &Path) -> usize {
    git(path, &["status", "--porcelain"]).map_or(0, |o| String::from_utf8_lossy(&o).lines().count())
}

/// Local branches, most recently committed to first.
pub fn local_branches(cwd: &Path) -> Vec<String> {
    git_lines(cwd, &["for-each-ref", "--sort=-committerdate", "--format=%(refname:short)", "refs/heads"])
}

/// Turns whatever was typed into a branch name git and GitHub take: lowercase, words
/// joined by `-`, `/` (or a `:` as in `feat: thing`) kept as the folder separator.
/// Characters git refuses (space, `~^:?*[\`, `..`, `@{`, a leading `-` or `.`, a
/// trailing `.lock`) never come out. Empty when nothing usable was typed.
pub fn branch_name(input: &str) -> String {
    let mapped: String = input
        .to_lowercase()
        .chars()
        .map(|c| match c {
            ':' => '/',
            c if c.is_alphanumeric() || matches!(c, '/' | '.' | '_') => c,
            _ => '-',
        })
        .collect();
    let mut parts = Vec::new();
    for part in mapped.split('/') {
        let mut s = String::new();
        for c in part.chars() {
            // Runs of `-` or `.` become one, so no `--` or `..`.
            if !(matches!(c, '-' | '.') && s.ends_with(c)) {
                s.push(c);
            }
        }
        let mut s = s.trim_matches(['-', '.']);
        while let Some(rest) = s.strip_suffix(".lock") {
            s = rest.trim_end_matches(['-', '.']);
        }
        if !s.is_empty() {
            parts.push(s.to_string());
        }
    }
    parts.join("/")
}

/// `git push` for a local branch other than (or the same as) the checked-out one, without
/// switching to it: to the remote it tracks, or to origin with upstream set when it has none.
pub fn push_branch_cmd(cwd: &Path, branch: &str) -> Result<Cmd, String> {
    if git(cwd, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{branch}")]).is_none() {
        return Err(format!("no local branch {branch}"));
    }
    let config = |key: &str| git_text(cwd, &["config", &format!("branch.{branch}.{key}")]);
    let push = Step::git().arg("push");
    Ok(match (config("remote"), config("merge")) {
        // "." tracks a local branch; pushing there would only move that branch.
        (Some(remote), Some(merge)) if remote != "." => {
            let to = merge.strip_prefix("refs/heads/").unwrap_or(&merge);
            push.arg(remote).arg(format!("{branch}:{to}")).into()
        }
        _ => push.args(["-u", "origin", branch]).into(),
    })
}

/// How a local branch stands with the remote branch it tracks.
#[derive(Debug, PartialEq)]
pub enum Upstream {
    /// It tracks nothing: never pushed.
    None,
    /// It tracked a remote branch that has been deleted since, usually after its pull request was merged.
    Gone,
    Track {
        ahead: u32,
        behind: u32,
    },
}

/// A branch in the `g o` picker.
pub struct Branch {
    pub name: String,
    /// Only on a remote: switching to it makes a local branch that tracks it.
    pub remote_only: bool,
    pub current: bool,
    /// The branch the others are measured against (`origin/main`, or `main` without a remote).
    pub base: bool,
    /// Commits it has that the base doesn't.
    pub ahead: usize,
    /// When the work on it started: the author date of the oldest of those commits.
    pub started: Option<(i64, String)>,
    /// Its newest commit.
    pub last: (i64, String),
    pub upstream: Upstream,
    /// The order to merge in: 1 for the branch whose work started first. Only for
    /// branches with commits the base doesn't have.
    pub number: Option<usize>,
}

const DATE: &str = "format-local:%Y-%m-%d %H:%M";

/// What branches are measured against: the remote's default branch, else a local main or master.
pub fn base_branch(cwd: &Path) -> Option<String> {
    let exists = |r: &str| git(cwd, &["rev-parse", "--verify", "--quiet", r]).is_some();
    git_text(cwd, &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"]).filter(|b| !b.is_empty()).or_else(|| {
        ["origin/main", "origin/master", "main", "master"].into_iter().find(|b| exists(b)).map(String::from)
    })
}

/// Every branch, local and on remotes, in merge order: the numbered ones (work that isn't
/// in the base yet, oldest first), then the base, then the rest, newest first.
pub fn branch_list(cwd: &Path) -> (Option<String>, Vec<Branch>) {
    let base = base_branch(cwd);
    let format = format!(
        "--format=%(refname)%09%(HEAD)%09%(committerdate:unix)%09%(committerdate:{DATE})%09%(upstream)%09%(upstream:track,nobracket)"
    );
    let Some(raw) = git(cwd, &["for-each-ref", &format, "refs/heads", "refs/remotes"]) else {
        return (base, Vec::new());
    };
    let raw = String::from_utf8_lossy(&raw);
    // The base's own name without the remote: a local `main` is the base, too.
    let base_name = base.as_deref().map(|b| b.split_once('/').map_or(b, |(_, n)| n).to_string());
    let mut out: Vec<Branch> = Vec::new();
    for line in raw.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        let [refname, head, unix, date, upstream, track] = f[..] else { continue };
        let (name, remote_only) = if let Some(b) = refname.strip_prefix("refs/heads/") {
            (b.to_string(), false)
        } else if let Some(rest) = refname.strip_prefix("refs/remotes/") {
            match rest.split_once('/') {
                Some((_, "HEAD")) | None => continue,
                Some((_, b)) => (b.to_string(), true),
            }
        } else {
            continue;
        };
        // A remote branch that's also local shows up once, as the local one (local refs come first).
        if remote_only && out.iter().any(|o| o.name == name) {
            continue;
        }
        let is_base = base_name.as_deref() == Some(name.as_str());
        let (ahead, started) = match base.as_deref().filter(|_| !is_base) {
            Some(b) => {
                let short = refname.trim_start_matches("refs/heads/").trim_start_matches("refs/remotes/");
                own_commits(cwd, b, short)
            }
            None => (0, None),
        };
        let upstream = match (upstream, track) {
            ("", _) => Upstream::None,
            (_, "gone") => Upstream::Gone,
            (_, t) => Upstream::Track { ahead: count(t, "ahead"), behind: count(t, "behind") },
        };
        out.push(Branch {
            name,
            remote_only,
            current: head == "*",
            base: is_base,
            ahead,
            started,
            last: (unix.parse().unwrap_or(0), date.to_string()),
            upstream,
            number: None,
        });
    }
    order(&mut out);
    (base, out)
}

/// Sorts into merge order and numbers the branches that have work to merge.
fn order(list: &mut [Branch]) {
    let rank = |b: &Branch| {
        if b.base {
            1
        } else if b.ahead > 0 {
            0
        } else {
            2
        }
    };
    list.sort_by(|a, b| {
        let started = |x: &Branch| x.started.as_ref().map_or(i64::MAX, |s| s.0);
        rank(a).cmp(&rank(b)).then_with(|| match rank(a) {
            // A branch stacked on another started with the same commit: the one below goes first.
            0 => started(a).cmp(&started(b)).then(a.ahead.cmp(&b.ahead)).then_with(|| a.last.0.cmp(&b.last.0)),
            _ => b.last.0.cmp(&a.last.0),
        })
    });
    for (i, b) in list.iter_mut().filter(|b| rank(b) == 0).enumerate() {
        b.number = Some(i + 1);
    }
}

/// How many commits `branch` has that `base` doesn't, and the author date of the oldest.
fn own_commits(cwd: &Path, base: &str, branch: &str) -> (usize, Option<(i64, String)>) {
    let range = format!("{base}..{branch}");
    let date = format!("--date={DATE}");
    let Some(raw) = git(cwd, &["log", "--format=%at%x09%ad", &date, &range, "--"]) else { return (0, None) };
    let raw = String::from_utf8_lossy(&raw);
    let lines: Vec<&str> = raw.lines().collect();
    let oldest = lines.last().and_then(|l| l.split_once('\t')).map(|(t, d)| (t.parse().unwrap_or(0), d.to_string()));
    (lines.len(), oldest)
}

/// "ahead 2, behind 1" → 2 for "ahead".
fn count(track: &str, what: &str) -> u32 {
    track.split(", ").find_map(|part| part.strip_prefix(what).map(|n| n.trim().parse().unwrap_or(0))).unwrap_or(0)
}

/// The checked-out branch, the remote branch it tracks and how many of its commits it
/// doesn't have yet, when there are any. As of the last fetch: git doesn't know newer ones.
pub fn behind_upstream(dir: &Path) -> Option<(String, String, usize)> {
    let out = |args: &[&str]| git_text(dir, args);
    let upstream = out(&["rev-parse", "--abbrev-ref", "@{upstream}"])?;
    let behind: usize = out(&["rev-list", "--count", "HEAD..@{upstream}"])?.parse().ok()?;
    let branch = out(&["branch", "--show-current"])?;
    (behind > 0).then_some((branch, upstream, behind))
}

/// `g f`: fetch from every remote and drop remote branches that are gone. With `prune_local`,
/// also delete local branches whose remote branch is gone, but only those whose work is in
/// `base`, so nothing that isn't merged is lost; the others are listed and kept.
///
/// Work counts as in `base` when the branch's commits are, or when merging the branch into
/// `base` wouldn't change a file: that's a branch rebased or squashed on the way in, whose
/// commits `base` has under other ids. A merge with conflicts keeps the branch.
pub fn fetch_cmd(prune_local: bool, base: Option<&str>) -> Cmd {
    if !prune_local {
        return Step::git().args(["fetch", "--all", "--prune"]).into();
    }
    let fetch = format!("{GIT} fetch --all --prune");
    Cmd::Sh(format!(
        "{fetch} && base={} && \
         in_base() {{ t=$(git merge-tree --write-tree --no-messages \"$base\" \"$1\" 2>/dev/null) && [ \"$t\" = \"$(git rev-parse \"$base^{{tree}}\")\" ]; }} && \
         git for-each-ref --format='%(if:equals=gone)%(upstream:track,nobracket)%(then)%(refname:short)%(end)' refs/heads \
         | while read -r b; do [ -n \"$b\" ] || continue; \
         if git merge-base --is-ancestor \"$b\" \"$base\"; then git branch -D \"$b\"; \
         elif in_base \"$b\"; then git branch -D \"$b\" && echo \"  (rebased or squashed: its changes are in $base)\"; \
         else echo \"kept $b: its remote branch is gone, but it isn't merged into $base\"; fi; done",
        quote_str(base.unwrap_or("HEAD"))
    ))
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
    ("sync", &[item('f', "fetch"), item('p', "pull"), item('P', "push"), item('B', "push another branch…")]),
    ("release (gh)", &[item('r', "release… bump, tag, CI"), item('R', "releases & CI runs")]),
    (
        "branches",
        &[
            item('b', "list branches"),
            item('o', "switch branch, merge order…"),
            item('n', "new branch… (name tidied)"),
            item('W', "remove a worktree…"),
            item('z', "stash"),
            item('Z', "stash pop"),
        ],
    ),
    (
        "other",
        &[item('i', "init repo here"), item('h', "create GitHub repo… (gh)"), item('g', "lazygit / gitui / tig")],
    ),
];

// ── gh repo create ────────────────────────────────────────────────────────

/// Builds the "create GitHub repo" form with everything prefilled for `source`.
pub fn gh_repo_form(source: &Path) -> Result<Form, String> {
    if crate::project::which("gh").is_none() {
        return Err("gh (GitHub CLI) is not installed".into());
    }
    let Some(owner) = proc::text(Command::new("gh").args(["api", "user", "--jq", ".login"])) else {
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

/// What the form currently describes; runs from `source`.
pub fn gh_repo_create_cmd(form: &Form) -> Cmd {
    let source = crate::config::expand_tilde(form.get("source"));
    let owner = form.get("owner").trim();
    let name = form.get("name").trim();
    let repo = if owner.is_empty() { name.to_string() } else { format!("{owner}/{name}") };

    let mut steps = Vec::new();
    if git(&source, &["rev-parse", "--git-dir"]).is_none() {
        steps.push(Step::words("git init"));
        steps.push(Step::words("git add -A"));
        steps.push(Step::words("git commit -m").arg("Initial commit"));
    }
    let mut gh = Step::words("gh repo create").arg(repo).arg(format!("--{}", form.get("visibility")));
    gh = gh.arg("--source=.").arg(format!("--remote={}", form.get("remote")));
    let desc = form.get("description").trim();
    if !desc.is_empty() {
        gh = gh.args(["--description", desc]);
    }
    if form.get("push") == "yes" {
        gh = gh.arg("--push");
    }
    steps.push(gh);
    Cmd::Steps(steps)
}

#[cfg(test)]
mod branch_name_tests {
    use super::*;
    use crate::testutil::{git as test_git, temp_dir};

    #[test]
    fn formats_typed_text() {
        assert_eq!(branch_name("Fix login bug"), "fix-login-bug");
        assert_eq!(branch_name("feat: Add dark mode (WIP)"), "feat/add-dark-mode-wip");
        assert_eq!(branch_name("fix/ crash on  empty   list "), "fix/crash-on-empty-list");
        assert_eq!(branch_name("--hotfix--"), "hotfix");
        assert_eq!(branch_name("a..b ~^?*[\\ c"), "a.b-c");
        assert_eq!(branch_name("feat//.hidden/x.lock"), "feat/hidden/x");
        assert_eq!(branch_name("@{u}"), "u");
        assert_eq!(branch_name("Übersetzung für v1.2_beta"), "übersetzung-für-v1.2_beta");
        assert_eq!(branch_name("  / :: ?? "), "");
    }

    /// Whatever comes out, git itself agrees it's a valid branch name.
    #[test]
    fn git_accepts_the_result() {
        let dir = temp_dir("branchname");
        let inputs = ["feat: Add X", "..a/b.lock.lock/", "-x -", "a@{b}", "x/./y", "tab\there", "dots...end."];
        for name in inputs.map(branch_name) {
            test_git(&dir, &["check-ref-format", "--branch", &name]);
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

#[cfg(test)]
mod push_tests {
    #![cfg_attr(windows, allow(unused_imports))]
    use super::*;
    use crate::shell::Shell;
    use crate::testutil::{git as test_git, temp_dir};

    /// Pushing another branch leaves the checkout alone and lands on the remote.
    #[test]
    fn pushes_a_branch_that_is_not_checked_out() {
        let base = temp_dir("push");
        let (repo, remote) = (base.join("repo"), base.join("remote.git"));
        std::fs::create_dir_all(&repo).unwrap();
        let run = |dir: &Path, args: &[&str]| drop(test_git(dir, args));
        run(&base, &["init", "-q", "--bare", "remote.git"]);
        run(&repo, &["init", "-q", "-b", "main"]);
        run(&repo, &["remote", "add", "origin", remote.to_str().unwrap()]);
        run(&repo, &["commit", "-q", "--allow-empty", "-m", "one"]);
        run(&repo, &["switch", "-q", "-c", "feature"]);
        run(&repo, &["commit", "-q", "--allow-empty", "-m", "two"]);
        run(&repo, &["switch", "-q", "main"]);

        assert_eq!(local_branches(&repo).len(), 2);
        assert!(push_branch_cmd(&repo, "nope").is_err());
        let cmd = push_branch_cmd(&repo, "feature").unwrap();
        assert_eq!(cmd.display(&Shell::sh()), "git push -u origin feature");
        let (ok, _) = cmd.run_in(&repo);

        let head = git(&repo, &["branch", "--show-current"]).unwrap();
        let on_remote = git(&remote, &["rev-parse", "feature"]).unwrap();
        let local = git(&repo, &["rev-parse", "feature"]).unwrap();
        // Now it tracks origin/feature, so the next push goes there by name.
        let again = push_branch_cmd(&repo, "feature").unwrap().display(&Shell::sh());
        std::fs::remove_dir_all(&base).unwrap();

        assert!(ok);
        assert_eq!(String::from_utf8_lossy(&head).trim(), "main");
        assert_eq!(on_remote, local);
        assert_eq!(again, "git push origin feature:feature");
    }
}

#[cfg(test)]
mod worktree_tests {
    use super::*;
    use crate::testutil::{git, temp_dir};

    #[test]
    fn lists_worktrees_with_branch_gone_and_dirty_state() {
        let base = temp_dir("wt");
        let main = base.join("repo");
        std::fs::create_dir_all(&main).unwrap();
        let run = |args: &[&str]| drop(git(&main, args));
        run(&["init", "-q", "-b", "main"]);
        run(&["commit", "-q", "--allow-empty", "-m", "one"]);
        run(&["worktree", "add", "-q", "../locks", "-b", "edit-locks"]);
        run(&["worktree", "add", "-q", "../gone", "-b", "old"]);
        std::fs::write(base.join("locks/new.txt"), "x").unwrap();
        std::fs::remove_dir_all(base.join("gone")).unwrap();

        let wts = worktrees(&main);
        let names: Vec<String> = wts.iter().map(|w| w.name()).collect();
        let dirty = uncommitted(&base.join("locks"));
        std::fs::remove_dir_all(&base).unwrap();

        // The main one first; the others come sorted by path.
        assert_eq!(names, ["repo", "gone", "locks"]);
        let by = |n: &str| wts.iter().find(|w| w.name() == n).unwrap();
        assert_eq!(by("locks").branch.as_deref(), Some("edit-locks"));
        assert!(!by("locks").gone && by("gone").gone);
        assert_eq!(dirty, 1);
    }
}

#[cfg(test)]
mod branch_tests {
    use super::*;

    fn run(dir: &Path, args: &[&str]) {
        crate::testutil::git(dir, args);
    }

    fn commit(dir: &Path, msg: &str, date: &str) {
        run(dir, &["commit", "-q", "--allow-empty", "-m", msg, "--date", date]);
    }

    /// A repo with a bare remote: main pushed, and its HEAD set like a clone would have it.
    fn setup(name: &str) -> (PathBuf, PathBuf) {
        let base = crate::testutil::temp_dir(name);
        let repo = base.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        run(&base, &["init", "-q", "--bare", "-b", "main", "remote.git"]);
        run(&repo, &["init", "-q", "-b", "main"]);
        run(&repo, &["remote", "add", "origin", base.join("remote.git").to_str().unwrap()]);
        commit(&repo, "one", "2026-01-01T10:00:00");
        run(&repo, &["push", "-q", "-u", "origin", "main"]);
        run(&repo, &["remote", "set-head", "origin", "main"]);
        (base, repo)
    }

    /// Numbered by when their work started, not by when they last changed; branches with
    /// nothing new and the base come after, unnumbered.
    #[test]
    fn numbers_branches_in_merge_order() {
        let (base, repo) = setup("branches");
        run(&repo, &["switch", "-q", "-c", "old-work"]);
        commit(&repo, "a", "2026-02-01T10:00:00");
        run(&repo, &["switch", "-q", "-c", "new-work", "main"]);
        commit(&repo, "b", "2026-03-01T10:00:00");
        commit(&repo, "c", "2026-03-02T10:00:00");
        // old-work changes last, but its work started first.
        run(&repo, &["switch", "-q", "old-work"]);
        commit(&repo, "d", "2026-04-01T10:00:00");
        run(&repo, &["switch", "-q", "-c", "nothing-new", "main"]);
        // Only on the remote: pushed from elsewhere, say.
        run(&repo, &["push", "-q", "origin", "new-work:remote-work"]);
        run(&repo, &["fetch", "-q"]);
        run(&repo, &["switch", "-q", "old-work"]);

        let (b, list) = branch_list(&repo);
        std::fs::remove_dir_all(&base).unwrap();

        assert_eq!(b.as_deref(), Some("origin/main"));
        let rows: Vec<(&str, Option<usize>, usize)> =
            list.iter().map(|b| (b.name.as_str(), b.number, b.ahead)).collect();
        assert_eq!(
            rows,
            [
                ("old-work", Some(1), 2),
                ("new-work", Some(2), 2),
                ("remote-work", Some(3), 2),
                ("main", None, 0),
                ("nothing-new", None, 0),
            ]
        );
        let by = |n: &str| list.iter().find(|b| b.name == n).unwrap();
        assert!(by("old-work").current && by("main").base && by("remote-work").remote_only);
        assert_eq!(by("old-work").started.as_ref().unwrap().1, "2026-02-01 10:00");
        assert_eq!(by("main").upstream, Upstream::Track { ahead: 0, behind: 0 });
        assert_eq!(by("new-work").upstream, Upstream::None);
    }

    /// With pruning on, a merged branch whose remote branch is gone is deleted; one that
    /// isn't merged is kept, and so is everything with pruning off.
    // Runs a sh script; Windows gets these once the script is Rust.
    #[cfg(unix)]
    #[test]
    fn prunes_only_merged_branches_whose_remote_is_gone() {
        let (base, repo) = setup("prune");
        let start = git_text(&repo, &["rev-parse", "HEAD"]).unwrap();
        // Each branch changes its own file; "conflict" changes the one main changes later.
        for (b, file) in
            [("merged", "m"), ("rebased", "r"), ("squashed", "s"), ("unmerged", "u"), ("conflict", "shared")]
        {
            run(&repo, &["switch", "-q", "-c", b, "main"]);
            for n in 1..=2 {
                std::fs::write(repo.join(file), format!("{b} {n}\n")).unwrap();
                run(&repo, &["add", file]);
                commit(&repo, &format!("{b} {n}"), "2026-02-01T10:00:00");
            }
            run(&repo, &["push", "-q", "-u", "origin", b]);
        }
        // On the remote: "merged" merged, "rebased" rebased onto main (new commit ids), "squashed"
        // squash-merged, then main moves on; every branch is deleted there.
        run(&repo, &["switch", "-q", "main"]);
        run(&repo, &["merge", "-q", "--no-ff", "-m", "merge", "merged"]);
        run(&repo, &["cherry-pick", "rebased~1", "rebased"]);
        run(&repo, &["merge", "-q", "--squash", "squashed"]);
        commit(&repo, "squash", "2026-02-02T10:00:00");
        std::fs::write(repo.join("shared"), "main\n").unwrap();
        run(&repo, &["add", "shared"]);
        commit(&repo, "later", "2026-02-03T10:00:00");
        run(&repo, &["push", "-q", "origin", "main", ":merged", ":rebased", ":squashed", ":unmerged", ":conflict"]);
        run(&repo, &["reset", "-q", "--hard", &start]);
        let run = |cmd: Cmd| {
            let (ok, out) = cmd.run_in(&repo);
            assert!(ok, "{cmd:?}: {out}");
            out
        };

        run(fetch_cmd(false, Some("origin/main")));
        let after_plain = local_branches(&repo);
        let out = run(fetch_cmd(true, Some("origin/main")));
        let after_prune = local_branches(&repo);
        std::fs::remove_dir_all(&base).unwrap();

        assert_eq!(after_plain.len(), 6);
        let mut left = after_prune.clone();
        left.sort();
        assert_eq!(left, ["conflict", "main", "unmerged"], "{out}");
        assert!(out.contains("Deleted branch merged"), "{out}");
        assert!(out.contains("Deleted branch rebased") && out.contains("Deleted branch squashed"), "{out}");
        assert!(out.contains("kept unmerged") && out.contains("kept conflict"), "{out}");
    }

    /// After a fetch brought new commits that weren't pulled, the checkout is behind.
    #[test]
    fn sees_a_checkout_behind_its_remote() {
        let (base, repo) = setup("behind");
        let up_to_date = behind_upstream(&repo);
        commit(&repo, "two", "2026-02-01T10:00:00");
        commit(&repo, "three", "2026-02-02T10:00:00");
        run(&repo, &["push", "-q"]);
        // Like merging pull requests on GitHub, then fetching without pulling.
        run(&repo, &["reset", "-q", "--hard", "HEAD~2"]);
        let behind = behind_upstream(&repo);
        std::fs::remove_dir_all(&base).unwrap();

        assert_eq!(up_to_date, None);
        assert_eq!(behind, Some(("main".into(), "origin/main".into(), 2)));
    }

    #[test]
    fn reads_ahead_and_behind() {
        assert_eq!(count("ahead 2, behind 1", "ahead"), 2);
        assert_eq!(count("ahead 2, behind 1", "behind"), 1);
        assert_eq!(count("behind 3", "ahead"), 0);
    }
}
