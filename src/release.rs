//! `g r`: a release in one go. Bumps the version in the manifest, commits it (on main or
//! through a merged PR), tags it, pushes the tag and follows the release workflow the tag
//! starts, or publishes with `gh release create` when there is none. `g R` lists releases
//! and CI runs.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use crate::{
    cmd::{Builtin, Cmd, Job, Step},
    config::dir_name,
    form::{Field, Form, FormKind},
    git::{git, git_lines as lines},
    proc,
    project::which,
    run::Ctx as Run,
};

/// Bump on a branch, open a PR and merge it right away.
const MERGE_PR: &str = "merge PR";
/// Bump on a branch and open a PR for the user to merge on GitHub once CI passes; the
/// next `g r` (as "already done") tags it.
const OPEN_PR: &str = "open PR";
const COMMIT: &str = "commit";
const DONE: &str = "already done";
const WATCH: &str = "watch CI";
const GH_RELEASE: &str = "gh release";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Cargo,
    Npm,
    Python,
}

struct Manifest {
    kind: Kind,
    name: String,
    version: String,
    /// Committed with the bump: the manifest and its lock file, when there is one.
    files: Vec<&'static str>,
}

impl Kind {
    /// The manifest, and the lock file committed with it when there is one.
    fn files(self) -> [&'static str; 2] {
        match self {
            Kind::Cargo => ["Cargo.toml", "Cargo.lock"],
            Kind::Npm => ["package.json", "package-lock.json"],
            Kind::Python => ["pyproject.toml", "uv.lock"],
        }
    }
}

impl Manifest {
    fn file(&self) -> &'static str {
        self.files[0]
    }

    fn bump(&self, v: &str) -> Vec<Job> {
        let lock = self.files.len() > 1;
        let set = || Builtin::SetVersion { file: self.file().into(), version: v.into() }.into();
        match self.kind {
            Kind::Cargo if lock => vec![set(), Step::words("cargo update --workspace --offline").into()],
            Kind::Npm => vec![Step::new("npm").args(["version", v, "--no-git-tag-version"]).into()],
            Kind::Python if lock => vec![set(), Step::words("uv lock").into()],
            Kind::Cargo | Kind::Python => vec![set()],
        }
    }

    /// Stops the release unless the (just pulled) manifest is at `v`: the bump PR is merged.
    fn guard(&self, v: &str, main: &str) -> Job {
        Builtin::RequireVersion {
            file: self.file().into(),
            version: v.into(),
            why: format!("{} on {main} isn't at {v} yet — is the release PR merged?", self.file()),
        }
        .into()
    }
}

/// The first `version = "…"` line set to `version`, the rest of the file as it was;
/// None when there's no such line.
pub fn set_version_line(text: &str, version: &str) -> Option<String> {
    const KEY: &str = "version = \"";
    let mut start = 0;
    for line in text.split_inclusive('\n') {
        if line.starts_with(KEY) {
            let value = start + KEY.len();
            let end = value + text[value..].find('"')?;
            return Some(format!("{}{version}{}", &text[..value], &text[end..]));
        }
        start += line.len();
    }
    None
}

/// Whether the manifest is at `version`: package.json's `"version"`, else the first
/// `version = "…"` line.
pub fn manifest_has_version(file: &str, text: &str, version: &str) -> bool {
    if file.ends_with(".json") {
        text.contains(&format!("\"version\": \"{version}\""))
    } else {
        let set = format!("version = \"{version}\"");
        text.lines().find(|l| l.starts_with("version = \"")).is_some_and(|l| l.starts_with(&set))
    }
}

/// What the release form needs beyond its fields; read once when it opens.
pub struct Ctx {
    pub root: PathBuf,
    manifest: Option<Manifest>,
    /// Newest version tag, and whether tags start with `v` (they do when there are none yet).
    last_tag: Option<String>,
    prefix: &'static str,
    main: String,
    /// Remote name → GitHub `owner/repo`, if it is on GitHub.
    remotes: Vec<(String, Option<String>)>,
    /// A workflow that runs when a tag is pushed.
    workflow: Option<String>,
    /// Why GitHub won't take a merge into main right away (required checks, reviews).
    required: Option<String>,
    /// Checks run on pull requests (a workflow, or required status checks), so wait for them.
    pr_checks: bool,
    /// Local `release/<version>` branches, left over from an `open PR` release.
    release_branches: Vec<String>,
}

impl Ctx {
    fn tag(&self, v: &str) -> String {
        format!("{}{v}", self.prefix)
    }

    fn repo(&self, remote: &str) -> Option<&str> {
        self.remotes.iter().find(|(r, _)| r == remote)?.1.as_deref()
    }
}

fn first_line(root: &Path, args: &[&str]) -> Option<String> {
    lines(root, args).into_iter().next().filter(|l| !l.is_empty())
}

/// `[major, minor, patch]` of `1.2.3`, `v1.2.3` or `1.2.3-rc1`.
fn triple(v: &str) -> Option<[u64; 3]> {
    let v = v.strip_prefix('v').unwrap_or(v);
    let core = v.split(['-', '+']).next()?;
    let mut parts = core.split('.').map(|p| p.parse().ok());
    let t = [parts.next()??, parts.next()??, parts.next()??];
    parts.next().is_none().then_some(t)
}

fn valid_version(v: &str) -> bool {
    triple(v).is_some() && !v.starts_with('v') && v.chars().all(|c| c.is_ascii_alphanumeric() || ".-+".contains(c))
}

fn has_suffix(v: &str) -> bool {
    v.contains(['-', '+'])
}

/// `owner/repo` of a GitHub remote URL (ssh or https).
pub fn github_repo(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("git@github.com:")
        .or_else(|| url.strip_prefix("ssh://git@github.com/"))
        .or_else(|| url.split_once("github.com/").map(|(_, r)| r))?;
    let repo = rest.trim_end_matches('/').trim_end_matches(".git");
    (repo.split('/').count() == 2).then(|| repo.to_string())
}

/// True when the first line starting `version = "` sets exactly `version`, so the bump
/// changes the right line.
fn first_version_line_is(text: &str, version: &str) -> bool {
    text.lines().find(|l| l.starts_with("version = \"")) == Some(&format!("version = \"{version}\""))
}

fn detect_manifest(root: &Path) -> Option<Manifest> {
    let (kind, name, version) = read_manifest(root)?;
    let name = name.unwrap_or_else(|| dir_name(root));
    let [manifest, lock] = kind.files();
    let mut files = vec![manifest];
    if root.join(lock).is_file() {
        files.push(lock);
    }
    Some(Manifest { kind, name, version, files })
}

/// The first manifest found: its kind, package name and version.
fn read_manifest(root: &Path) -> Option<(Kind, Option<String>, String)> {
    let name_of = |n: Option<&toml::Value>| n.and_then(|n| n.as_str()).map(str::to_string);
    if let Ok(text) = fs::read_to_string(root.join("Cargo.toml")) {
        let t: toml::Table = text.parse().ok()?;
        let pkg = t.get("package");
        let version = pkg
            .and_then(|p| p.get("version")?.as_str())
            .or_else(|| t.get("workspace")?.get("package")?.get("version")?.as_str())?;
        if !first_version_line_is(&text, version) {
            return None;
        }
        return Some((Kind::Cargo, name_of(pkg.and_then(|p| p.get("name"))), version.into()));
    }
    if let Ok(text) = fs::read_to_string(root.join("package.json")) {
        let j: serde_json::Value = serde_json::from_str(&text).ok()?;
        let version = j.get("version")?.as_str()?.to_string();
        return Some((Kind::Npm, j.get("name").and_then(|n| n.as_str()).map(str::to_string), version));
    }
    if let Ok(text) = fs::read_to_string(root.join("pyproject.toml")) {
        let t: toml::Table = text.parse().ok()?;
        let table = t.get("project").or_else(|| t.get("tool")?.get("poetry"))?;
        let version = table.get("version")?.as_str()?;
        if !first_version_line_is(&text, version) {
            return None;
        }
        return Some((Kind::Python, name_of(table.get("name")), version.into()));
    }
    None
}

/// What to run before committing the bump, as the project usually would.
fn default_checks(root: &Path, m: Option<&Manifest>) -> String {
    match m.map(|m| m.kind) {
        Some(Kind::Cargo) => "cargo build && cargo test && cargo clippy --all-targets".into(),
        Some(Kind::Npm) => {
            let scripts = fs::read_to_string(root.join("package.json"))
                .ok()
                .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
                .and_then(|j| j.get("scripts").cloned());
            let has = |s: &str| scripts.as_ref().is_some_and(|sc| sc.get(s).is_some());
            let mut cmds = Vec::new();
            if has("build") {
                cmds.push("npm run build");
            }
            if has("test") {
                cmds.push("npm test");
            }
            cmds.join(" && ")
        }
        _ => String::new(),
    }
}

/// The workflow files under `.github/workflows` (name, text), sorted by name.
fn workflows(root: &Path) -> Vec<(String, String)> {
    let Ok(rd) = fs::read_dir(root.join(".github/workflows")) else { return Vec::new() };
    let mut found: Vec<(String, String)> = rd
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if !(name.ends_with(".yml") || name.ends_with(".yaml")) {
                return None;
            }
            Some((name, fs::read_to_string(e.path()).ok()?))
        })
        .collect();
    found.sort();
    found
}

/// A workflow started by pushing a tag; one named "release" first.
fn tag_workflow(root: &Path) -> Option<String> {
    let mut found: Vec<String> = workflows(root)
        .into_iter()
        .filter(|(_, text)| text.lines().any(|l| l.trim_start().starts_with("tags:")))
        .map(|(name, _)| name)
        .collect();
    found.sort_by_key(|n| !n.contains("release"));
    found.into_iter().next()
}

/// A workflow that runs on pull requests.
fn pr_workflow(root: &Path) -> Option<String> {
    workflows(root)
        .into_iter()
        .find(|(_, text)| text.lines().any(|l| !l.trim_start().starts_with('#') && l.contains("pull_request")))
        .map(|(name, _)| name)
}

fn gh(args: &[&str]) -> Option<String> {
    proc::text(Command::new("gh").args(args))
}

/// What GitHub requires before a merge into `main` (branch protection and rulesets), as
/// "requires …" phrases, and whether that includes status checks.
fn merge_requirements(repo: &str, main: &str) -> (Vec<&'static str>, bool) {
    let (protection, rules) = std::thread::scope(|s| {
        let protection = s.spawn(|| {
            gh(&[
                "api",
                &format!("repos/{repo}/branches/{main}"),
                "--jq",
                "[.protected, (.protection.required_status_checks.contexts | length)] | @tsv",
            ])
        });
        let rules = gh(&["api", &format!("repos/{repo}/rules/branches/{main}"), "--jq", ".[].type"]);
        (protection.join().ok().flatten(), rules)
    });
    let mut reasons = Vec::new();
    let mut checks = false;
    let rules: Vec<&str> = rules.as_deref().unwrap_or_default().lines().collect();
    let protection = protection.unwrap_or_default();
    let (protected, contexts) = protection.split_once('\t').unwrap_or(("false", "0"));
    if rules.contains(&"required_status_checks") || contexts.parse().unwrap_or(0) > 0 {
        reasons.push("requires passing checks");
        checks = true;
    }
    if rules.contains(&"pull_request") {
        reasons.push("requires a reviewed PR");
    }
    if reasons.is_empty() && protected == "true" {
        reasons.push("is protected");
    }
    (reasons, checks)
}

/// The remote main's upstream lives on, else `origin`, else the first one.
fn default_remote(root: &Path, remotes: &[String]) -> Option<String> {
    for b in ["main", "master", "HEAD"] {
        if let Some(up) = first_line(root, &["rev-parse", "--abbrev-ref", &format!("{b}@{{upstream}}")])
            && let Some(r) = remotes.iter().find(|r| up.starts_with(&format!("{r}/")))
        {
            return Some(r.clone());
        }
    }
    remotes.iter().find(|r| *r == "origin").or(remotes.first()).cloned()
}

fn main_branch(root: &Path, remote: &str, current: &str) -> String {
    if let Some(head) = first_line(root, &["symbolic-ref", "--short", &format!("refs/remotes/{remote}/HEAD")])
        && let Some(b) = head.strip_prefix(&format!("{remote}/"))
    {
        return b.to_string();
    }
    ["main", "master"]
        .into_iter()
        .find(|b| git(root, &["rev-parse", "-q", "--verify", &format!("refs/heads/{b}")]).is_some())
        .map_or_else(|| current.to_string(), str::to_string)
}

/// Builds the release form for the repo at `root`, with the next version suggested.
pub fn form(root: &Path, branch: &str, dirty: bool) -> Result<Form, String> {
    let names = lines(root, &["remote"]);
    let Some(remote) = default_remote(root, &names) else {
        return Err("no git remote to push the tag to — g h creates a GitHub repo".into());
    };
    let remotes: Vec<(String, Option<String>)> = names
        .iter()
        .map(|r| (r.clone(), first_line(root, &["remote", "get-url", r]).and_then(|u| github_repo(&u))))
        .collect();
    let main = main_branch(root, &remote, branch);
    let manifest = detect_manifest(root);
    let last_tag = lines(root, &["tag", "-l", "--sort=-v:refname"]).into_iter().find(|t| triple(t).is_some());
    let prefix = if last_tag.as_ref().is_none_or(|t| t.starts_with('v')) { "v" } else { "" };
    let workflow = tag_workflow(root);
    let release_branches = lines(root, &["for-each-ref", "--format=%(refname:short)", "refs/heads/release/"]);
    let repo = remotes.iter().find(|(r, _)| *r == remote).and_then(|(_, repo)| repo.clone());
    let (reasons, required_checks) = match repo.as_deref().filter(|_| which("gh").is_some()) {
        Some(repo) => merge_requirements(repo, &main),
        None => (Vec::new(), false),
    };
    let required = (!reasons.is_empty()).then(|| format!("{main} {}", reasons.join(" and ")));
    let pr_ci = pr_workflow(root);
    let pr_checks = required_checks || pr_ci.is_some();

    // Suggest the manifest's version when it is ahead of the last tag (bumped by hand, not
    // released yet), else the next patch.
    let last = last_tag.as_deref().and_then(triple);
    let cur = manifest.as_ref().and_then(|m| triple(&m.version));
    let ahead = matches!((cur, last), (Some(c), Some(l)) if c > l) || (cur.is_some() && last.is_none());
    let base = cur.max(last);
    let next = |t: [u64; 3]| format!("{}.{}.{}", t[0], t[1], t[2]);
    // A release branch from `open PR` that wasn't tagged yet: this is round two.
    let pending = release_branches
        .iter()
        .filter_map(|b| b.strip_prefix("release/"))
        .filter(|v| valid_version(v) && last.is_none_or(|l| triple(v).is_some_and(|t| t > l)))
        .max_by_key(|v| triple(v))
        .map(str::to_string);
    let suggested = match (&pending, ahead, base) {
        (Some(p), _, _) => p.clone(),
        (None, true, _) => manifest.as_ref().map(|m| m.version.clone()).unwrap_or_default(),
        (None, false, Some([a, b, c])) => next([a, b, c + 1]),
        (None, false, None) => "0.1.0".into(),
    };

    let mut info = Vec::new();
    if let Some(t) = &last_tag {
        info.push(format!("last tag {t}"));
    }
    if let Some(m) = &manifest {
        info.push(format!("{} {}", m.file(), m.version));
    }
    if let Some([a, b, _]) = base {
        info.push(format!("minor {} · major {}", next([a, b + 1, 0]), next([a + 1, 0, 0])));
    }

    let mut fields = vec![Field::text("version", suggested.clone(), info.join(" · "))];
    if let Some(m) = &manifest {
        let merges_prs = lines(root, &["log", "-n", "50", "--merges", "--format=%s", &main])
            .iter()
            .any(|s| s.starts_with("Merge pull request"));
        let how = if m.version == suggested || pending.is_some() {
            3
        } else if required.is_some() || (merges_prs && pr_checks) {
            1
        } else if merges_prs {
            0
        } else {
            2
        };
        fields.push(Field::choice(
            "bump",
            &[MERGE_PR, OPEN_PR, COMMIT, DONE],
            how,
            format!("bumps {}", m.files.join(" + ")),
        ));
    }
    // Round two: the checks ran before the PR, and CI on it.
    let checks = if pending.is_some() { String::new() } else { default_checks(root, manifest.as_ref()) };
    fields.push(Field::text("checks", checks, "run before tagging · empty skips"));
    let remote_names: Vec<&str> = names.iter().map(String::as_str).collect();
    let remote_i = names.iter().position(|r| *r == remote).unwrap_or(0);
    fields.push(Field::choice("remote", &remote_names, remote_i, "gets the commit and the tag"));
    fields.push(match &workflow {
        Some(wf) => Field::choice("then", &[WATCH, "stop"], 0, format!("the tag starts {wf}")),
        None => {
            Field::choice("then", &[GH_RELEASE, "tag only"], 0, "no tag workflow — gh release create --generate-notes")
        }
    });

    let note = if let Some(p) = &pending {
        Some(format!("release/{p} is waiting — once its PR is merged on GitHub, this tags {prefix}{p}"))
    } else if let Some(why) = required.as_ref().filter(|_| fields.iter().any(|f| f.value() == OPEN_PR)) {
        Some(format!("{why} — opens the PR; merge it on GitHub when CI passes, then g r again"))
    } else if let Some(wf) = pr_ci.as_ref().filter(|_| fields.iter().any(|f| f.value() == OPEN_PR)) {
        Some(format!("{wf} runs on PRs — opens the PR; merge it on GitHub when it passes, then g r again"))
    } else if dirty {
        Some("uncommitted changes — they stay out of the release, but may block switching branches".into())
    } else if branch != main {
        Some(format!("on {branch} — switches to {main} first"))
    } else if manifest.is_none() {
        Some("no version in Cargo.toml / package.json / pyproject.toml — only tags".into())
    } else if remotes.iter().all(|(_, repo)| repo.is_none()) {
        Some(format!("{remote} isn't on GitHub — only the tag gets pushed"))
    } else {
        None
    };

    let ctx = Ctx {
        root: root.to_path_buf(),
        manifest,
        last_tag,
        prefix,
        main,
        remotes,
        workflow,
        required,
        pr_checks,
        release_branches,
    };
    Ok(Form {
        kind: FormKind::Release(Box::new(ctx)),
        title: " release ".into(),
        fields,
        step: 0,
        note,
        completer: None,
    })
}

/// Catches what would otherwise fail halfway through, after something was pushed.
pub fn check(form: &Form, ctx: &Ctx) -> Result<(), String> {
    let v = form.get("version").trim();
    if !valid_version(v) {
        return Err(format!("'{v}' isn't a version like 1.2.3"));
    }
    let tag = ctx.tag(v);
    if git(&ctx.root, &["rev-parse", "-q", "--verify", &format!("refs/tags/{tag}")]).is_some() {
        return Err(format!("tag {tag} exists already"));
    }
    if let (Some(last), Some(new)) = (ctx.last_tag.as_deref(), triple(v)) {
        let l = triple(last).unwrap_or_default();
        if new < l || (new == l && !has_suffix(last)) {
            return Err(format!("{v} isn't higher than the last tag {last}"));
        }
    }
    let bump = form.get("bump");
    let br = format!("release/{v}");
    let pending = ctx.release_branches.contains(&br);
    if let Some(m) = &ctx.manifest {
        // A pending release branch is checked against main after the pull instead.
        if bump == DONE && m.version != v && !pending {
            return Err(format!("{} says {}, not {v} — pick a bump", m.file(), m.version));
        }
        if bump != DONE && m.version == v {
            return Err(format!("{} is at {v} already — pick 'already done'", m.file()));
        }
    }
    if bump == MERGE_PR
        && let Some(why) = &ctx.required
    {
        return Err(format!("{why} — pick 'open PR' and merge it on GitHub"));
    }
    let then = form.get("then");
    let remote = form.get("remote");
    let uses_gh = bump == MERGE_PR || bump == OPEN_PR || then == WATCH || then == GH_RELEASE;
    if uses_gh {
        if which("gh").is_none() {
            return Err("gh (GitHub CLI) is not installed".into());
        }
        if ctx.repo(remote).is_none() {
            return Err(format!("{remote} isn't a GitHub remote — commit + tag only, without gh"));
        }
    }
    // Round two of `open PR`: don't tag before the PR is in.
    if bump == DONE
        && let Some(repo) = ctx.repo(remote)
        && let Some(open) =
            gh(&["pr", "view", &br, "-R", repo, "--json", "state,url", "--jq", "select(.state == \"OPEN\") | .url"])
        && !open.is_empty()
    {
        return Err(format!("the release PR isn't merged yet — wait for CI, merge it on GitHub: {open}"));
    }
    Ok(())
}

/// The release, shown one step per line in the form and run one after another.
pub fn steps(form: &Form, ctx: &Ctx) -> Vec<Job> {
    let v = form.get("version").trim();
    let tag = ctx.tag(v);
    let remote = form.get("remote").to_string();
    let main = ctx.main.clone();
    let repo = ctx.repo(&remote).map(str::to_string);
    let gh = |args: &[&str]| {
        let step = Step::new("gh").args(args.iter().copied());
        match &repo {
            Some(r) => step.args(["-R", r]),
            None => step,
        }
    };
    let git = |args: &[&str]| Step::git().args(args.iter().copied());
    let msg = format!("Release {v}");
    let bump = form.get("bump");
    let br = format!("release/{v}");

    let mut s: Vec<Job> = vec![git(&["switch", &main]).into(), git(&["pull", "--ff-only", &remote, &main]).into()];
    if bump == DONE
        && let Some(m) = &ctx.manifest
    {
        s.push(m.guard(v, &ctx.main));
    }
    let bumping = ctx.manifest.as_ref().filter(|_| [MERGE_PR, OPEN_PR, COMMIT].contains(&bump));
    if let Some(m) = bumping {
        s.extend(m.bump(v));
    }
    let checks = form.get("checks").trim();
    if !checks.is_empty() {
        s.push(Builtin::RunShell(checks.to_string()).into());
    }
    if let Some(m) = bumping {
        let commit = git(&["commit", "-m", &msg, "--"]).args(m.files.iter().copied());
        if bump == COMMIT {
            s.push(commit.into());
            s.push(git(&["push", &remote, &main]).into());
        } else {
            s.push(git(&["switch", "-c", &br]).into());
            s.push(commit.into());
            s.push(git(&["push", &remote, &br]).into());
            let body = format!("Sets the version to {v}.");
            s.push(gh(&["pr", "create", "-B", &main, "-H", &br, "-t", &msg, "-b", &body]).into());
        }
        if bump == OPEN_PR {
            // Merging is the user's, on GitHub, once CI is happy; tagging waits for the next g r.
            if ctx.pr_checks {
                // The checks show up a few seconds after the PR.
                s.push(Builtin::Sleep(10).into());
                s.push(gh(&["pr", "checks", &br, "--watch", "-i", "20"]).may_fail().into());
            }
            let next = if ctx.pr_checks { "Next: once the checks pass, merge" } else { "Next: merge" };
            s.push(Builtin::Say(String::new()).into());
            s.push(
                Builtin::Say(format!(
                    "{next} the PR on GitHub (link above), then g r again: it tags {tag} and releases."
                ))
                .into(),
            );
            return s;
        }
        if bump == MERGE_PR {
            s.push(gh(&["pr", "merge", &br, "--merge"]).into());
            s.push(git(&["switch", &main]).into());
            s.push(git(&["pull", "--ff-only", &remote, &main]).into());
        }
    }
    s.push(git(&["tag", &tag]).into());
    s.push(git(&["push", &remote, &tag]).into());
    if bump == MERGE_PR || ctx.release_branches.contains(&br) {
        // Left over from `open PR`, or made by `merge PR`; GitHub may have deleted it already.
        s.push(Builtin::DeleteBranch { remote: remote.clone(), branch: br }.into());
    }

    match form.get("then") {
        WATCH => s.push(Builtin::WatchRelease { repo, workflow: ctx.workflow.clone().unwrap_or_default(), tag }.into()),
        GH_RELEASE => {
            let name = ctx.manifest.as_ref().map_or_else(|| dir_name(&ctx.root), |m| m.name.clone());
            let mut title = name.chars();
            let title = title.next().map(|c| c.to_uppercase().chain(title).collect::<String>()).unwrap_or_default();
            let title = format!("{title} {v}");
            s.push(gh(&["release", "create", &tag, "--title", &title, "--generate-notes", "--verify-tag"]).into());
        }
        _ => {}
    }
    s
}

/// The release workflow the tag push started: found once GitHub lists it (a few seconds
/// after the push), followed to the end, then the release it made.
pub fn watch_release(run: &mut Run, repo: Option<&str>, workflow: &str, tag: &str) -> Result<(), String> {
    let gh = |args: &[&str]| {
        let step = Step::new("gh").args(args.iter().copied());
        match repo {
            Some(r) => step.args(["-R", r]),
            None => step,
        }
    };
    let head = run.ask(&Step::words("git rev-parse HEAD")).ok_or("git rev-parse HEAD failed")?;
    run.say(format!("waiting for the {workflow} run of {tag} …"));
    let list = gh(&[
        "run",
        "list",
        "-w",
        workflow,
        "-b",
        tag,
        "-c",
        &head,
        "-L",
        "1",
        "--json",
        "databaseId",
        "-q",
        ".[0].databaseId",
    ]);
    let mut id = None;
    for _ in 0..20 {
        if let Some(found) = run.ask(&list).filter(|s| !s.is_empty()) {
            id = Some(found);
            break;
        }
        if !run.sleep(std::time::Duration::from_secs(3)) {
            return Err("stopped".into());
        }
    }
    let id = id.ok_or_else(|| format!("no {workflow} run for {tag} showed up — gh run list shows what ran"))?;
    run.must(&gh(&["run", "watch", &id, "-i", "20", "--exit-status"]), || format!("the {workflow} run failed"))?;
    run.must(&gh(&["release", "view", tag]), || format!("no release {tag} — did the workflow make one?"))
}

/// `g R`: the latest releases and CI runs.
pub fn overview(root: &Path) -> Result<Cmd, String> {
    if which("gh").is_none() {
        return Err("gh (GitHub CLI) is not installed".into());
    }
    let names = lines(root, &["remote"]);
    let repo = default_remote(root, &names)
        .and_then(|rem| first_line(root, &["remote", "get-url", &rem]))
        .and_then(|u| github_repo(&u));
    let gh = |args: &str| {
        let step = Step::words(args);
        match &repo {
            Some(r) => step.args(["-R", r]),
            None => step,
        }
    };
    Ok(Cmd::Steps(vec![
        gh("gh release list -L 10").into(),
        Builtin::Say(String::new()).into(),
        gh("gh run list -L 15").into(),
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bumps_only_the_first_version_line() {
        let toml = "[package]\nname = \"x\"\nversion = \"1.2.3\" # keep me\r\n\n[dependencies]\nversion = \"9\"\n";
        let new = set_version_line(toml, "1.3.0").unwrap();
        assert_eq!(new, toml.replacen("1.2.3", "1.3.0", 1));
        assert!(manifest_has_version("Cargo.toml", &new, "1.3.0"));
        assert!(!manifest_has_version("Cargo.toml", toml, "1.3.0"));
        assert_eq!(set_version_line("[package]\nname = \"x\"\n", "1.0.0"), None);
        assert!(manifest_has_version("package.json", "{\n  \"version\": \"2.0.0\"\n}", "2.0.0"));
    }

    #[test]
    fn versions() {
        assert_eq!(triple("v0.10.2"), Some([0, 10, 2]));
        assert_eq!(triple("1.0.0-rc1"), Some([1, 0, 0]));
        assert_eq!(triple("1.0"), None);
        assert!(valid_version("0.3.2"));
        assert!(!valid_version("v0.3.2"));
        assert!(!valid_version("0.3.2'; rm"));
    }

    #[test]
    fn github_urls() {
        assert_eq!(github_repo("git@github.com:mergedeyes/terminaal.git").as_deref(), Some("mergedeyes/terminaal"));
        assert_eq!(github_repo("https://github.com/a/b").as_deref(), Some("a/b"));
        assert_eq!(github_repo("https://gitlab.com/a/b"), None);
    }

    #[test]
    fn version_line() {
        let cargo = "[package]\nname = \"x\"\nversion = \"0.3.1\"\n\n[dependencies.y]\nversion = \"1\"\n";
        assert!(first_version_line_is(cargo, "0.3.1"));
        assert!(!first_version_line_is("[package]\nversion=\"0.3.1\"\n", "0.3.1"));
    }

    #[test]
    fn manifests_bring_their_lock_file() {
        let dir = crate::testutil::temp_dir("release-manifest");
        let cargo = dir.join("crate");
        fs::create_dir_all(&cargo).unwrap();
        fs::write(cargo.join("Cargo.toml"), "[package]\nname = \"tool\"\nversion = \"0.2.0\"\n").unwrap();
        fs::write(cargo.join("Cargo.lock"), "").unwrap();
        let npm = dir.join("web-app");
        fs::create_dir_all(&npm).unwrap();
        fs::write(npm.join("package.json"), r#"{"version": "1.4.0"}"#).unwrap();
        let ws = dir.join("ws");
        fs::create_dir_all(&ws).unwrap();
        fs::write(ws.join("Cargo.toml"), "[workspace.package]\nversion = \"3.0.0\"\n").unwrap();

        let (c, n, w) = (detect_manifest(&cargo).unwrap(), detect_manifest(&npm).unwrap(), detect_manifest(&ws));
        let none = detect_manifest(&dir);
        fs::remove_dir_all(&dir).unwrap();
        assert!(c.kind == Kind::Cargo && c.name == "tool" && c.version == "0.2.0");
        assert_eq!(c.files, ["Cargo.toml", "Cargo.lock"]);
        // No name in package.json: the folder's; no lock file: just the manifest.
        assert!(n.kind == Kind::Npm && n.name == "web-app" && n.files == ["package.json"]);
        assert!(w.is_some_and(|w| w.name == "ws" && w.version == "3.0.0"));
        assert!(none.is_none());
    }

    #[test]
    fn finds_tag_and_pr_workflows() {
        let dir = crate::testutil::temp_dir("release-workflows");
        let wf = dir.join(".github/workflows");
        fs::create_dir_all(&wf).unwrap();
        fs::write(wf.join("ci.yml"), "on:\n  pull_request:\n  push:\n    tags: ['v*']\n").unwrap();
        fs::write(wf.join("release.yaml"), "on:\n  push:\n    tags: ['v*']\n").unwrap();
        fs::write(wf.join("README.md"), "runs on pull_request\n").unwrap();
        let (tag, pr) = (tag_workflow(&dir), pr_workflow(&dir));
        fs::remove_dir_all(&dir).unwrap();
        assert_eq!(tag.as_deref(), Some("release.yaml"));
        assert_eq!(pr.as_deref(), Some("ci.yml"));
    }

    /// A whole release without GitHub: bump, commit, push, tag, push the tag.
    #[test]
    fn commit_release_bumps_tags_and_pushes() {
        use crate::testutil::{git as tgit, temp_dir};
        let base = temp_dir("release-run");
        let (repo, remote) = (base.join("repo"), base.join("remote.git"));
        fs::create_dir_all(&repo).unwrap();
        tgit(&base, &["init", "-q", "--bare", "-b", "main", "remote.git"]);
        tgit(&repo, &["init", "-q", "-b", "main"]);
        tgit(&repo, &["config", "user.name", "t"]);
        tgit(&repo, &["config", "user.email", "t@t"]);
        fs::write(repo.join("Cargo.toml"), "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n").unwrap();
        tgit(&repo, &["add", "-A"]);
        tgit(&repo, &["commit", "-q", "-m", "one"]);
        tgit(&repo, &["remote", "add", "origin", &remote.to_string_lossy()]);
        tgit(&repo, &["push", "-q", "-u", "origin", "main"]);

        let mut form = form(&repo, "main", false).unwrap();
        for f in &mut form.fields {
            match f.label {
                "version" => f.default = "0.2.0".into(),
                "checks" => f.default = String::new(),
                "bump" => f.choice = f.options.iter().position(|o| o == COMMIT).unwrap(),
                "then" => f.choice = f.options.iter().position(|o| o == "tag only").unwrap(),
                _ => {}
            }
        }
        let FormKind::Release(ctx) = &form.kind else { panic!("not a release form") };
        check(&form, ctx).unwrap();
        let (ok, out) = crate::run::testing::run_in(&Cmd::Steps(steps(&form, ctx)), &repo);
        let manifest = fs::read_to_string(repo.join("Cargo.toml")).unwrap();
        let tagged = tgit(&remote, &["log", "-1", "--format=%s", "v0.2.0"]);
        let main = tgit(&remote, &["log", "-1", "--format=%s", "main"]);
        fs::remove_dir_all(&base).unwrap();

        assert!(ok, "{out}");
        assert!(manifest.contains("version = \"0.2.0\""), "{manifest}");
        assert_eq!(tagged.trim(), "Release 0.2.0");
        assert_eq!(main.trim(), "Release 0.2.0");
    }
}
