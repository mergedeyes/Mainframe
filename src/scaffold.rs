//! New projects (`J`, then `^G`): the folder, `git init`, and a CLAUDE.md from a template.
//!
//! The template is built in; `~/.config/mainframe/CLAUDE.md` replaces it when it exists.
//! Lines between `<!-- if rust -->` and `<!-- end -->` only stay for that stack, so one
//! file can carry the checks of every stack.

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use crate::{
    config::{self, Config},
    project::Kind,
    form::Form,
};

/// The stacks the form offers, as written in the template's `<!-- if … -->` markers.
pub const STACKS: &[&str] = &["none", "rust", "node", "python"];

pub const CLAUDE_MD: &str = r#"# {name}

<!-- One sentence: what this project is and who it's for. -->

## Layout

<!-- What lives where: main directories, entry points. -->

## Checks

After a change, run the checks below and report what they say. If one fails, fix the
cause instead of loosening a limit or silencing a lint; if a limit really has to move, say so.

<!-- if rust -->
- `cargo fmt --all`
- `cargo clippy --all-targets -- -D warnings`
- `cargo test`
<!-- end -->
<!-- if node -->
- `npx tsc --noEmit`
- `npx eslint .`
- `npm test` (once a test script exists)
<!-- end -->
<!-- if python -->
- `PYTHONDONTWRITEBYTECODE=1 python3 -m pytest`
- `ruff check .` (if configured)
<!-- end -->
<!-- if none -->
<!-- The commands to run after a change: lint, tests, build. -->
<!-- end -->

Tests can't show everything: where possible, try UI and behaviour changes in the real
program, and say plainly what wasn't verified.

## Conventions

<!-- Project-specific rules: error handling, naming, patterns to follow. -->

- Comments explain *why* in full sentences; match the comment density of the surrounding code.
- Make the smallest correct change, in the layer where the problem actually lives. No speculative refactors.
- Correct behaviour and functionality first, UX polish second, visual polish last.
- Don't invent a visual design direction: before significant UI work, ask what's wanted and for reference UIs.
- Update the README and docs in the same change as the feature or fix, not afterwards.
- After editing the README or docs, read back what you changed for the current feature or fix once,
  and fix any mistakes: wrong or outdated statements, keys or commands that don't match the code,
  broken links, typos. Only your own edits from this branch, and only one pass.
- A bug found while working on something else gets fixed right away, as its own commit, and mentioned.

## Git

- Every feature and fix gets its own branch. Before starting: `git fetch --prune`, fast-forward
  `main`, then branch from `main`. One PR per independent change. Never edit files on `main`.
- Check the branch in the same command as the commit (`git branch --show-current && git commit …`):
  the user switches branches in their own terminal while you work.
- Commit, push and open PRs only when asked. Never merge a PR unless the user explicitly allows it for that PR.
- No destructive git commands (reset --hard, force-push, …) unless asked; never revert unrelated work.
- Commit messages: `type(scope): Capitalised summary`, e.g. `fix(parser): Handle empty input`.

## Files and secrets

- Never commit secrets, and never put vulnerability details in tracked files. Local notes go in ignored files.
- This CLAUDE.md is local only (listed in `.git/info/exclude`).
- Run Python with `PYTHONDONTWRITEBYTECODE=1`, and delete any `__pycache__` folders before finishing.

## Environment

- The Bash tool runs zsh, not the user's fish: quote globs (`'*.rs'`), or zsh fails with "no matches found".
- git and other tools print their messages in German.
"#;

/// `~/.config/mainframe/CLAUDE.md`, next to the config.
pub fn template_path() -> PathBuf {
    Config::path().with_file_name("CLAUDE.md")
}

/// The user's template if there is one, else the built-in one.
pub fn template() -> String {
    fs::read_to_string(template_path()).unwrap_or_else(|_| CLAUDE_MD.to_string())
}

/// The template for one project: `{name}` filled in, other stacks' blocks dropped.
pub fn render(template: &str, name: &str, stack: &str) -> String {
    render_stacks(template, name, &[stack])
}

/// `render` for a project with several stacks (a Tauri app is rust and node).
pub fn render_stacks(template: &str, name: &str, stacks: &[&str]) -> String {
    let mut out = String::new();
    // The stack of the block we're in; outside any block every line stays.
    let mut block: Option<&str> = None;
    for line in template.lines() {
        let marker = line.trim().strip_prefix("<!--").and_then(|l| l.strip_suffix("-->")).map(str::trim);
        match marker {
            Some(m) if m.starts_with("if ") => block = Some(m[3..].trim()),
            Some("end") if block.is_some() => block = None,
            _ if block.is_none_or(|b| stacks.contains(&b)) => {
                out.push_str(line);
                out.push('\n');
            }
            _ => {}
        }
    }
    out.replace("{name}", name)
}

/// Where a preset waits for Claude to merge it: every `.md` in `.claude/rules/` is loaded
/// when Claude Code starts in the project.
pub const PENDING: &str = ".claude/rules/mainframe-preset.md";

/// The files Claude Code reads a project's instructions from, those `dir` has.
pub fn instruction_files(dir: &Path) -> Vec<&'static str> {
    ["CLAUDE.md", ".claude/CLAUDE.md", "CLAUDE.local.md", "AGENTS.md"].into_iter().filter(|f| dir.join(f).is_file()).collect()
}

/// The template's stacks for the build tools at `dir`; "none" when it has none of them.
pub fn stacks(dir: &Path) -> Vec<&'static str> {
    let mut out: Vec<&str> = crate::project::kinds_at(dir)
        .into_iter()
        .filter_map(|k| match k {
            Kind::Cargo => Some("rust"),
            Kind::Npm => Some("node"),
            Kind::Python => Some("python"),
            _ => None,
        })
        .collect();
    out.dedup();
    if out.is_empty() {
        out.push("none");
    }
    out
}

pub enum Preset {
    /// There were no instructions yet: the preset is the project's CLAUDE.md now.
    Written,
    /// Claude merges it at its next start (true: an earlier one was still waiting).
    Pending(bool),
}

/// Brings the preset into an existing project: as its CLAUDE.md when it has no
/// instructions, else as a note Claude reads at its next start and merges into them.
/// Either way git ignores what's written.
pub fn apply_preset(dir: &Path, preset: &str) -> Result<Preset, String> {
    let existing = instruction_files(dir);
    if existing.is_empty() {
        fs::write(dir.join("CLAUDE.md"), preset).map_err(|e| format!("can't write CLAUDE.md: {e}"))?;
        exclude_here(dir, "CLAUDE.md").map_err(|e| format!("wrote CLAUDE.md, but can't exclude it from git: {e}"))?;
        return Ok(Preset::Written);
    }
    let path = dir.join(PENDING);
    let again = path.exists();
    let written = path.parent().map_or(Ok(()), fs::create_dir_all).and_then(|_| fs::write(&path, merge_note(preset, &existing)));
    written.map_err(|e| format!("can't write {PENDING}: {e}"))?;
    exclude_here(dir, PENDING).map_err(|e| format!("wrote {PENDING}, but can't exclude it from git: {e}"))?;
    Ok(Preset::Pending(again))
}

/// The note in `.claude/rules/`: what Claude should do with the preset, then the preset.
fn merge_note(preset: &str, existing: &[&str]) -> String {
    let files = existing.iter().map(|f| format!("`{f}`")).collect::<Vec<_>>().join(", ");
    format!(
        r#"# Pending: merge the Mainframe CLAUDE.md preset

Mainframe (the user's terminal file manager) put this here so you'd see it at the start of
the session. Do this once, before other work, or right after the user's first request if
that's urgent:

1. Merge the preset below into this project's instructions ({files}). If there are
   several, use the one that holds the project's main instructions, usually `CLAUDE.md`.
   - What's already there is project-specific and wins where the two disagree; mention
     each such conflict to the user instead of silently dropping either side.
   - Add the preset's rules that are missing. Don't repeat ones that are already there
     in other words; where the preset says something more precisely, you may tighten
     the existing wording.
   - Fill the preset's `<!-- … -->` placeholders from what you know of the project, or
     leave them out. Drop `<!-- if … -->` / `<!-- end -->` markers if any remain.
   - Keep the file's structure; put new rules under its matching headings.
2. Delete this file (`{PENDING}`).
3. Tell the user in a few lines what you added, changed and left out, and why.

Don't commit anything for this unless the user asks.

## The preset

~~~~markdown
{preset}~~~~
"#
    )
}

/// Adds `rel` (relative to `dir`) to the repo's `info/exclude`, so it never shows up in
/// `git status`. Works from a subfolder of the repo and in linked worktrees; outside a
/// repo there's nothing to do.
fn exclude_here(dir: &Path, rel: &str) -> std::io::Result<()> {
    let git = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .current_dir(dir)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    };
    let (Some(file), Some(prefix)) = (git(&["rev-parse", "--path-format=absolute", "--git-path", "info/exclude"]), git(&["rev-parse", "--show-prefix"])) else {
        return Ok(());
    };
    append_line(Path::new(&file), &format!("/{prefix}{rel}"))
}

/// The folder for a new project named `name` in `location`, or why there can't be one.
pub fn target(location: &Path, name: &str) -> Result<PathBuf, String> {
    if name.is_empty() || name == "." || name == ".." || name.contains('/') {
        return Err("give the project a folder name (no /)".into());
    }
    if !location.is_dir() {
        return Err(format!("{} is not a directory", location.display()));
    }
    let dir = location.join(name);
    if dir.exists() {
        return Err(format!("{} exists already", dir.display()));
    }
    Ok(dir)
}

/// What submitting the new-project form will do, one line per step.
pub fn steps(form: &Form) -> Vec<String> {
    let name = form.get("name").trim();
    let location = config::expand_tilde(form.get("location").trim());
    let shown = if name.is_empty() { "…".into() } else { config::pretty_path(&location.join(name)) };
    let mut steps = vec![format!("mkdir {shown}"), "git init -b main".into()];
    if form.get("CLAUDE.md") == "yes" {
        steps.push(format!("write CLAUDE.md ({} checks), excluded in .git/info/exclude", form.get("stack")));
    }
    steps.push("go there".into());
    steps
}

/// Makes `dir`, runs `git init` there on `main`, and writes `claude_md` (if any) as a
/// CLAUDE.md that git ignores through `.git/info/exclude`, so it never gets committed.
pub fn create(dir: &Path, claude_md: Option<&str>) -> Result<(), String> {
    fs::create_dir(dir).map_err(|e| format!("can't create {}: {e}", dir.display()))?;
    // `-b main`: without init.defaultBranch git would pick `master`, and the template's
    // git rules talk about `main`.
    let out = Command::new("git")
        .args(["init", "--quiet", "-b", "main"])
        .current_dir(dir)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("created the folder, but can't run git: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!("created the folder, but git init failed: {}", err.trim()));
    }
    let Some(text) = claude_md else { return Ok(()) };
    fs::write(dir.join("CLAUDE.md"), text).map_err(|e| format!("can't write CLAUDE.md: {e}"))?;
    exclude(dir, "/CLAUDE.md").map_err(|e| format!("wrote CLAUDE.md, but can't add it to .git/info/exclude: {e}"))
}

/// Adds `pattern` to the repo's `.git/info/exclude` unless a line says it already.
fn exclude(repo: &Path, pattern: &str) -> std::io::Result<()> {
    append_line(&repo.join(".git/info/exclude"), pattern)
}

fn append_line(path: &Path, pattern: &str) -> std::io::Result<()> {
    let path = path.to_path_buf();
    let current = fs::read_to_string(&path).unwrap_or_default();
    if current.lines().any(|l| l.trim() == pattern) {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = fs::OpenOptions::new().create(true).append(true).open(&path)?;
    let sep = if current.is_empty() || current.ends_with('\n') { "" } else { "\n" };
    writeln!(file, "{sep}{pattern}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_only_the_chosen_stacks_checks() {
        let rust = render(CLAUDE_MD, "demo", "rust");
        assert!(rust.starts_with("# demo\n"));
        assert!(rust.contains("cargo clippy --all-targets -- -D warnings"));
        assert!(!rust.contains("npx tsc") && !rust.contains("pytest") && !rust.contains("lint, tests, build"));
        assert!(!rust.contains("<!-- if") && !rust.contains("<!-- end"));

        let none = render(CLAUDE_MD, "demo", "none");
        assert!(none.contains("<!-- The commands to run after a change: lint, tests, build. -->"));
        assert!(!none.contains("cargo") && !none.contains("npx"));

        // Every stack the form offers has a block in the built-in template.
        for stack in STACKS {
            assert!(CLAUDE_MD.contains(&format!("<!-- if {stack} -->")), "{stack}");
        }
    }

    #[test]
    fn a_lone_end_marker_is_just_text() {
        assert_eq!(render("a\n<!-- end -->\nb", "x", "rust"), "a\n<!-- end -->\nb\n");
    }

    #[test]
    fn creates_a_repo_with_an_ignored_claude_md() {
        let root = std::env::temp_dir().join(format!("mainframe-scaffold-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();

        assert!(target(&root, "").is_err());
        assert!(target(&root, "a/b").is_err());
        assert!(target(&root.join("missing"), "x").is_err());
        let dir = target(&root, "demo").unwrap();

        create(&dir, Some("# demo\n")).unwrap();
        assert_eq!(fs::read_to_string(dir.join("CLAUDE.md")).unwrap(), "# demo\n");
        let branch = crate::git::git(&dir, &["symbolic-ref", "--short", "HEAD"]).unwrap();
        assert_eq!(String::from_utf8_lossy(&branch).trim(), "main");
        // Ignored, so it never shows up for `git add -A`.
        assert!(crate::git::git(&dir, &["check-ignore", "-q", "CLAUDE.md"]).is_some());

        // A second exclude of the same pattern doesn't add a duplicate line.
        exclude(&dir, "/CLAUDE.md").unwrap();
        let excludes = fs::read_to_string(dir.join(".git/info/exclude")).unwrap();
        assert_eq!(excludes.lines().filter(|l| *l == "/CLAUDE.md").count(), 1);

        assert!(target(&root, "demo").is_err(), "exists now");
        let bare = root.join("bare");
        create(&bare, None).unwrap();
        assert!(bare.join(".git").is_dir() && !bare.join("CLAUDE.md").exists());
        let _ = fs::remove_dir_all(&root);
    }

    fn repo(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mainframe-preset-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Command::new("git").args(["init", "-q", "-b", "main"]).current_dir(&dir).output().unwrap();
        dir
    }

    fn status(dir: &Path) -> String {
        let out = Command::new("git").args(["status", "--porcelain", "--untracked-files=all"]).current_dir(dir).output().unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    #[test]
    fn preset_becomes_the_claude_md_when_there_is_none() {
        let dir = repo("none");
        let res = apply_preset(&dir, "# preset\n");
        let text = fs::read_to_string(dir.join("CLAUDE.md")).unwrap_or_default();
        let st = status(&dir);
        fs::remove_dir_all(&dir).unwrap();
        assert!(matches!(res, Ok(Preset::Written)));
        assert_eq!(text, "# preset\n");
        assert_eq!(st, "", "git should ignore it");
    }

    /// A project in a subfolder of its repo, with its own CLAUDE.md: the preset waits in
    /// .claude/rules/ for Claude, the CLAUDE.md stays as it is, git sees neither change.
    #[test]
    fn preset_waits_for_claude_when_there_are_instructions() {
        let top = repo("merge");
        let dir = top.join("app");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("CLAUDE.md"), "# app\nmine\n").unwrap();
        Command::new("git").args(["add", "."]).current_dir(&top).output().unwrap();
        Command::new("git").args(["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "x"]).current_dir(&top).output().unwrap();

        let first = apply_preset(&dir, "# preset\n- a rule\n");
        let second = apply_preset(&dir, "# preset\n- a rule\n");
        let note = fs::read_to_string(dir.join(PENDING)).unwrap_or_default();
        let mine = fs::read_to_string(dir.join("CLAUDE.md")).unwrap();
        let st = status(&top);
        fs::remove_dir_all(&top).unwrap();

        assert!(matches!(first, Ok(Preset::Pending(false))));
        assert!(matches!(second, Ok(Preset::Pending(true))));
        assert!(note.contains("`CLAUDE.md`") && note.contains("- a rule") && note.contains(PENDING), "{note}");
        assert_eq!(mine, "# app\nmine\n");
        assert_eq!(st, "", "git should ignore the note");
    }

    #[test]
    fn stacks_follow_the_build_tools() {
        let dir = std::env::temp_dir().join(format!("mainframe-preset-stacks-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let none = stacks(&dir);
        fs::write(dir.join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        fs::write(dir.join("package.json"), "{}").unwrap();
        let both = stacks(&dir);
        fs::remove_dir_all(&dir).unwrap();
        assert_eq!(none, ["none"]);
        assert_eq!(both, ["rust", "node"]);
        let text = render_stacks(CLAUDE_MD, "x", &both);
        assert!(text.contains("cargo clippy") && text.contains("npx tsc") && !text.contains("pytest"));
    }
}
