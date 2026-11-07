//! `g r`: a release in one go. Bumps the version in the manifest, commits it (on main or
//! through a merged PR), tags it, pushes the tag and follows the release workflow the tag
//! starts, or publishes with `gh release create` when there is none. `g R` lists releases
//! and CI runs.

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use crate::{
    form::{Field, Form, FormKind},
    git::git,
    project::{quote_str, which},
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

impl Manifest {
    fn file(&self) -> &'static str {
        self.files[0]
    }

    fn bump(&self, v: &str) -> String {
        let lock = self.files.len() > 1;
        // The first `version = "…"` line, which `detect` checked is the package's.
        let sed = |file: &str| format!("sed -i '0,/^version = \".*\"/s//version = \"{v}\"/' {file}");
        match self.kind {
            Kind::Cargo if lock => format!("{} && cargo update --workspace --offline", sed("Cargo.toml")),
            Kind::Cargo => sed("Cargo.toml"),
            Kind::Npm => format!("npm version {v} --no-git-tag-version"),
            Kind::Python if lock => format!("{} && uv lock", sed("pyproject.toml")),
            Kind::Python => sed("pyproject.toml"),
        }
    }

    /// Stops the release unless the (just pulled) manifest is at `v`: the bump PR is merged.
    fn guard(&self, v: &str, main: &str) -> String {
        let pattern = match self.kind {
            Kind::Npm => format!("\"version\": \"{v}\""),
            _ => format!("^version = \"{v}\""),
        };
        let why = quote_str(&format!("{} on {main} isn't at {v} yet — is the release PR merged?", self.file()));
        format!("{{ grep -q '{pattern}' {} || {{ echo {why} >&2; false; }}; }}", self.file())
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

fn lines(root: &Path, args: &[&str]) -> Vec<String> {
    git(root, args).map(|o| String::from_utf8_lossy(&o).lines().map(str::to_string).collect()).unwrap_or_default()
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
fn github_repo(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("git@github.com:")
        .or_else(|| url.strip_prefix("ssh://git@github.com/"))
        .or_else(|| url.split_once("github.com/").map(|(_, r)| r))?;
    let repo = rest.trim_end_matches('/').trim_end_matches(".git");
    (repo.split('/').count() == 2).then(|| repo.to_string())
}

/// True when the first line starting `version = "` sets exactly `version`, so the sed in
/// `Manifest::bump` changes the right line.
fn first_version_line_is(text: &str, version: &str) -> bool {
    text.lines().find(|l| l.starts_with("version = \"")) == Some(&format!("version = \"{version}\""))
}

fn detect_manifest(root: &Path) -> Option<Manifest> {
    let exists = |f: &str| root.join(f).is_file();
    let dir_name = || root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();

    if let Ok(text) = fs::read_to_string(root.join("Cargo.toml")) {
        let t: toml::Table = text.parse().ok()?;
        let pkg = t.get("package");
        let version = pkg
            .and_then(|p| p.get("version")?.as_str())
            .or_else(|| t.get("workspace")?.get("package")?.get("version")?.as_str())?;
        if !first_version_line_is(&text, version) {
            return None;
        }
        let name = pkg.and_then(|p| p.get("name")?.as_str()).map_or_else(dir_name, str::to_string);
        let mut files = vec!["Cargo.toml"];
        if exists("Cargo.lock") {
            files.push("Cargo.lock");
        }
        return Some(Manifest { kind: Kind::Cargo, name, version: version.into(), files });
    }
    if let Ok(text) = fs::read_to_string(root.join("package.json")) {
        let j: serde_json::Value = serde_json::from_str(&text).ok()?;
        let version = j.get("version")?.as_str()?.to_string();
        let name = j.get("name").and_then(|n| n.as_str()).map_or_else(dir_name, str::to_string);
        let mut files = vec!["package.json"];
        if exists("package-lock.json") {
            files.push("package-lock.json");
        }
        return Some(Manifest { kind: Kind::Npm, name, version, files });
    }
    if let Ok(text) = fs::read_to_string(root.join("pyproject.toml")) {
        let t: toml::Table = text.parse().ok()?;
        let table = t.get("project").or_else(|| t.get("tool")?.get("poetry"))?;
        let version = table.get("version")?.as_str()?;
        if !first_version_line_is(&text, version) {
            return None;
        }
        let name = table.get("name").and_then(|n| n.as_str()).map_or_else(dir_name, str::to_string);
        let mut files = vec!["pyproject.toml"];
        if exists("uv.lock") {
            files.push("uv.lock");
        }
        return Some(Manifest { kind: Kind::Python, name, version: version.into(), files });
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

/// A workflow under `.github/workflows` started by pushing a tag; one named "release" first.
fn tag_workflow(root: &Path) -> Option<String> {
    let mut found: Vec<String> = fs::read_dir(root.join(".github/workflows"))
        .ok()?
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if !(name.ends_with(".yml") || name.ends_with(".yaml")) {
                return None;
            }
            let text = fs::read_to_string(e.path()).ok()?;
            text.lines().any(|l| l.trim_start().starts_with("tags:")).then_some(name)
        })
        .collect();
    found.sort_by_key(|n| !n.contains("release"));
    found.into_iter().next()
}

/// A workflow under `.github/workflows` that runs on pull requests.
fn pr_workflow(root: &Path) -> Option<String> {
    let mut found: Vec<String> = fs::read_dir(root.join(".github/workflows"))
        .ok()?
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let text = fs::read_to_string(e.path()).ok()?;
            text.lines().any(|l| !l.trim_start().starts_with('#') && l.contains("pull_request")).then_some(name)
        })
        .collect();
    found.sort();
    found.into_iter().next()
}

fn gh(args: &[&str]) -> Option<String> {
    let out = Command::new("gh").args(args).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
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
        None => Field::choice("then", &[GH_RELEASE, "tag only"], 0, "no tag workflow — gh release create --generate-notes"),
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
        && let Some(open) = gh(&["pr", "view", &br, "-R", repo, "--json", "state,url", "--jq", "select(.state == \"OPEN\") | .url"])
        && !open.is_empty()
    {
        return Err(format!("the release PR isn't merged yet — wait for CI, merge it on GitHub: {open}"));
    }
    Ok(())
}

/// The release as shell steps, shown one per line in the form and run joined with `&&`.
pub fn steps(form: &Form, ctx: &Ctx) -> Vec<String> {
    let v = form.get("version").trim();
    let tag = ctx.tag(v);
    let remote = quote_str(form.get("remote"));
    let main = quote_str(&ctx.main);
    let r = ctx.repo(form.get("remote")).map(|repo| format!(" -R {repo}")).unwrap_or_default();
    let msg = quote_str(&format!("Release {v}"));
    let bump = form.get("bump");

    let br = format!("release/{v}");
    // Left over from `open PR`, or made by `merge PR`; GitHub may have deleted it already.
    let cleanup = format!("{{ git branch -d {br} 2>/dev/null; git push {remote} --delete {br} 2>/dev/null; true; }}");

    let mut s = vec![format!("git switch {main} && git pull --ff-only {remote} {main}")];
    if bump == DONE
        && let Some(m) = &ctx.manifest
    {
        s.push(m.guard(v, &ctx.main));
    }
    let bumping = ctx.manifest.as_ref().filter(|_| [MERGE_PR, OPEN_PR, COMMIT].contains(&bump));
    if let Some(m) = bumping {
        s.push(m.bump(v));
    }
    let checks = form.get("checks").trim();
    if !checks.is_empty() {
        s.push(checks.to_string());
    }
    if let Some(m) = bumping {
        let files = m.files.join(" ");
        if bump == COMMIT {
            s.push(format!("git commit -m {msg} -- {files} && git push {remote} {main}"));
        } else {
            s.push(format!("git switch -c {br} && git commit -m {msg} -- {files} && git push {remote} {br}"));
            let body = quote_str(&format!("Sets the version to {v}."));
            s.push(format!("gh pr create{r} -B {main} -H {br} -t {msg} -b {body}"));
        }
        if bump == OPEN_PR {
            // Merging is the user's, on GitHub, once CI is happy; tagging waits for the next g r.
            if ctx.pr_checks {
                s.push(format!("{{ sleep 10 && gh pr checks {br}{r} --watch -i 20; true; }}"));
            }
            let next = if ctx.pr_checks { "Next: once the checks pass, merge" } else { "Next: merge" };
            s.push(format!(
                "echo && echo {}",
                quote_str(&format!("{next} the PR on GitHub (link above), then g r again: it tags {tag} and releases."))
            ));
            return s;
        }
        if bump == MERGE_PR {
            s.push(format!("gh pr merge {br}{r} --merge && git switch {main} && git pull --ff-only {remote} {main}"));
        }
    }
    s.push(format!("git tag {tag} && git push {remote} {tag}"));
    if bump == MERGE_PR || ctx.release_branches.contains(&br) {
        s.push(cleanup);
    }

    match form.get("then") {
        WATCH => {
            let wf = ctx.workflow.as_deref().unwrap_or_default();
            // The run shows up a few seconds after the push.
            s.push(format!(
                "id= && for i in $(seq 20); do id=$(gh run list{r} -w {wf} -b {tag} -c $(git rev-parse HEAD) -L 1 \
                 --json databaseId -q '.[0].databaseId'); [ -n \"$id\" ] && break; sleep 3; done"
            ));
            s.push(format!("gh run watch \"$id\"{r} -i 20 --exit-status && gh release view {tag}{r}"));
        }
        GH_RELEASE => {
            let name = ctx.manifest.as_ref().map_or_else(
                || ctx.root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                |m| m.name.clone(),
            );
            let mut title = name.chars();
            let title = title.next().map(|c| c.to_uppercase().chain(title).collect::<String>()).unwrap_or_default();
            s.push(format!(
                "gh release create {tag}{r} --title {} --generate-notes --verify-tag",
                quote_str(&format!("{title} {v}"))
            ));
        }
        _ => {}
    }
    s
}

/// `g R`: the latest releases and CI runs.
pub fn overview(root: &Path) -> Result<String, String> {
    if which("gh").is_none() {
        return Err("gh (GitHub CLI) is not installed".into());
    }
    let names = lines(root, &["remote"]);
    let r = default_remote(root, &names)
        .and_then(|rem| first_line(root, &["remote", "get-url", &rem]))
        .and_then(|u| github_repo(&u))
        .map(|repo| format!(" -R {repo}"))
        .unwrap_or_default();
    Ok(format!("gh release list{r} -L 10 && echo && gh run list{r} -L 15"))
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
