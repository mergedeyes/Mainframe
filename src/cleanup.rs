//! What `C` can clear in a project: build output, installed dependencies and caches that the
//! build tool brings back by itself. Anything git tracks is never offered.

use std::{
    collections::{BTreeMap, HashSet},
    fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use crate::{
    project::{self, Kind, Project, quote, quote_str, which},
    targets,
};

pub struct Item {
    pub label: String,
    /// What it is and what brings it back.
    pub note: &'static str,
    /// What gets removed (and measured).
    pub paths: Vec<PathBuf>,
    /// Runs instead of `rm -rf` on `paths` (`cargo clean`, `make clean`).
    pub cmd: Option<String>,
    /// Disk space the paths take; None for a command whose effect isn't known up front.
    pub size: Option<u64>,
}

impl Item {
    fn dirs(label: impl Into<String>, note: &'static str, paths: Vec<PathBuf>) -> Self {
        Item { label: label.into(), note, paths, cmd: None, size: None }
    }

    pub fn command(&self, root: &Path) -> String {
        match &self.cmd {
            Some(c) => c.clone(),
            None => format!("rm -rf -- {}", self.paths.iter().map(|p| show(p, root)).collect::<Vec<_>>().join(" ")),
        }
    }

    /// The paths, relative to `root` when they're inside it.
    pub fn location(&self, root: &Path) -> String {
        match self.paths.len() {
            0 => String::new(),
            1..=3 => self.paths.iter().map(|p| show(p, root)).collect::<Vec<_>>().join(" "),
            n => format!("{n} folders"),
        }
    }
}

fn show(p: &Path, root: &Path) -> String {
    match p.strip_prefix(root) {
        Ok(rel) if !rel.as_os_str().is_empty() => quote(rel),
        _ => quote(p),
    }
}

/// Disk space of the union of the items' paths: debug builds plus the incremental cache
/// inside them count once.
pub fn total(items: &[&Item]) -> u64 {
    let mut all: Vec<&PathBuf> = items.iter().flat_map(|i| &i.paths).collect();
    all.sort();
    all.dedup();
    let mut seen = HashSet::new();
    all.iter()
        .filter(|p| !all.iter().any(|q| q != *p && p.starts_with(q)))
        .map(|p| disk_usage(p, &mut seen))
        .sum()
}

const NODE: &[(&str, &str)] = &[
    ("node_modules", "installed packages — the package manager's install brings them back"),
    (".next", "Next.js build output and cache"),
    (".nuxt", "Nuxt build output"),
    (".output", "Nuxt / Nitro build output"),
    (".svelte-kit", "SvelteKit build output"),
    (".turbo", "Turborepo cache"),
    (".parcel-cache", "Parcel cache"),
    (".angular", "Angular cache"),
    ("dist", "build output"),
    ("build", "build output"),
    ("out", "build output"),
    ("coverage", "test coverage report"),
];

const PYTHON: &[(&str, &str)] = &[
    (".venv", "virtualenv — uv sync / pip install brings it back"),
    ("venv", "virtualenv — pip install brings it back"),
    (".pytest_cache", "pytest cache"),
    (".mypy_cache", "mypy cache"),
    (".ruff_cache", "ruff cache"),
    (".tox", "tox environments"),
    (".nox", "nox environments"),
    ("htmlcov", "test coverage report"),
    (".coverage", "test coverage data"),
    ("build", "build output"),
    ("dist", "built wheels and sdists"),
];

const ZIG: &[(&str, &str)] = &[
    ("zig-out", "build output"),
    (".zig-cache", "build cache"),
    ("zig-cache", "build cache"),
];

const NATIVE: &[(&str, &str)] = &[
    ("build", "debug build tree — b configures it again"),
    ("build-release", "release build tree — B configures it again"),
    ("builddir", "build tree"),
    ("cmake-build-debug", "IDE build tree"),
    ("cmake-build-release", "IDE build tree"),
];

const GO: &[(&str, &str)] = &[("build", "build output"), ("bin", "build output")];

/// Everything there is to clean in the project at `root`, measured. Slow on big trees,
/// so it runs off the UI thread.
pub fn scan(project: &Project) -> Vec<Item> {
    let root = &project.root;
    let mut items = Vec::new();
    // A Tauri app is cargo and npm at once, so every build tool with a marker here counts.
    for kind in project::kinds_at(root) {
        match kind {
            Kind::Cargo => cargo(root, &mut items),
            Kind::Npm => named(root, NODE, &mut items),
            Kind::Python => {
                named(root, PYTHON, &mut items);
                python_extra(root, &mut items);
            }
            Kind::Zig => named(root, ZIG, &mut items),
            Kind::CMake | Kind::Meson => named(root, NATIVE, &mut items),
            Kind::Go => {
                named(root, GO, &mut items);
                go_cache(&mut items);
            }
            Kind::Make | Kind::Just => {}
        }
    }

    let mut seen = HashSet::new();
    items.retain(|i| i.cmd.is_some() || i.paths.iter().all(|p| seen.insert(p.clone())));
    drop_tracked(root, &mut items);

    // The project's own clean target last: it may do anything, so it isn't measured.
    for t in targets::list(project).into_iter().filter(|t| t.name == "clean") {
        items.push(Item {
            label: format!("{} clean", t.group),
            note: "the project's own clean target",
            paths: Vec::new(),
            cmd: Some(t.cmd),
            size: None,
        });
    }

    for item in &mut items {
        if !item.paths.is_empty() {
            item.size = Some(total(&[item]));
        }
    }
    items
}

fn named(root: &Path, list: &[(&str, &'static str)], items: &mut Vec<Item>) {
    for &(name, note) in list {
        let p = root.join(name);
        if fs::symlink_metadata(&p).is_ok() {
            items.push(Item::dirs(name, note, vec![p]));
        }
    }
}

/// `__pycache__` anywhere in the tree and `*.egg-info` next to the sources.
fn python_extra(root: &Path, items: &mut Vec<Item>) {
    let mut found = Vec::new();
    let mut budget = 50_000;
    find_pycache(root, &mut found, &mut budget);
    if !found.is_empty() {
        items.push(Item::dirs("__pycache__", "compiled bytecode — Python writes it again on import", found));
    }
    let egg: Vec<PathBuf> = [root.to_path_buf(), root.join("src")]
        .iter()
        .filter_map(|d| fs::read_dir(d).ok())
        .flat_map(|rd| rd.flatten().map(|e| e.path()))
        .filter(|p| p.is_dir() && p.extension().is_some_and(|x| x == "egg-info"))
        .collect();
    if !egg.is_empty() {
        items.push(Item::dirs("*.egg-info", "packaging metadata from setuptools", egg));
    }
}

fn find_pycache(dir: &Path, out: &mut Vec<PathBuf>, budget: &mut usize) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        if *budget == 0 {
            return;
        }
        *budget -= 1;
        let Ok(ft) = e.file_type() else { continue };
        if !ft.is_dir() {
            continue;
        }
        let name = e.file_name();
        let name = name.to_string_lossy();
        if name == "__pycache__" {
            out.push(e.path());
        } else if !name.starts_with('.') && !matches!(name.as_ref(), "node_modules" | "venv" | "target") {
            find_pycache(&e.path(), out, budget);
        }
    }
}

/// Go keeps no build output in the project; its compile cache is shared by every project.
fn go_cache(items: &mut Vec<Item>) {
    let Some(dir) = command_output("go", &["env", "GOCACHE"], Path::new("/")).map(PathBuf::from) else { return };
    if dir.is_dir() {
        items.push(Item {
            label: "go build cache".into(),
            note: "shared by all Go projects — rebuilt as needed",
            paths: vec![dir],
            cmd: Some("go clean -cache".into()),
            size: None,
        });
    }
}

/// One row per profile (debug, release, custom ones), each covering the host build and
/// every cross target's, then the incremental cache, docs and `cargo clean`.
fn cargo(root: &Path, items: &mut Vec<Item>) {
    let target = cargo_target_dir(root);
    let Ok(rd) = fs::read_dir(&target) else { return };
    let mut profiles: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
    for e in rd.flatten().filter(|e| e.path().is_dir()) {
        let path = e.path();
        if is_profile_dir(&path) {
            profiles.entry(e.file_name().to_string_lossy().into_owned()).or_default().push(path);
        } else if let Ok(sub) = fs::read_dir(&path) {
            // target/<triple>/<profile>
            for s in sub.flatten().filter(|s| is_profile_dir(&s.path())) {
                profiles.entry(s.file_name().to_string_lossy().into_owned()).or_default().push(s.path());
            }
        }
    }

    let incremental: Vec<PathBuf> =
        profiles.values().flatten().map(|p| p.join("incremental")).filter(|p| p.is_dir()).collect();
    let mut order: Vec<String> = profiles.keys().cloned().collect();
    order.sort_by_key(|p| match p.as_str() {
        "debug" => 0,
        "release" => 2,
        _ => 3,
    });
    for name in order {
        let paths = profiles.remove(&name).unwrap_or_default();
        let (label, note) = match name.as_str() {
            "debug" => ("debug builds".to_string(), "cargo build / run / test / check output — the next build compiles everything again"),
            "release" => ("release builds".to_string(), "cargo build --release output — installed copies stay"),
            _ => (format!("{name} builds"), "output of a custom profile"),
        };
        items.push(Item::dirs(label, note, paths));
        if name == "debug" && !incremental.is_empty() {
            items.push(Item::dirs(
                "incremental cache",
                "incremental compilation cache, grows with every edit — dependencies stay built",
                incremental.clone(),
            ));
        }
    }
    let doc = target.join("doc");
    if doc.is_dir() {
        items.push(Item::dirs("docs", "cargo doc output", vec![doc]));
    }
    items.push(Item {
        label: "all of target".into(),
        note: "cargo clean — everything cargo built for this project",
        paths: vec![target],
        cmd: Some("cargo clean".into()),
        size: None,
    });
}

fn is_profile_dir(p: &Path) -> bool {
    p.join(".fingerprint").is_dir() || p.join("deps").is_dir()
}

/// Workspaces, `CARGO_TARGET_DIR` and `build.target-dir` all move it; cargo knows where.
fn cargo_target_dir(root: &Path) -> PathBuf {
    command_output("cargo", &["metadata", "--no-deps", "--format-version", "1", "--offline"], root)
        .and_then(|out| serde_json::from_str::<serde_json::Value>(&out).ok())
        .and_then(|v| v.get("target_directory")?.as_str().map(PathBuf::from))
        .unwrap_or_else(|| root.join("target"))
}

fn command_output(program: &str, args: &[&str], cwd: &Path) -> Option<String> {
    which(program)?;
    let out = Command::new(program).args(args).current_dir(cwd).stderr(Stdio::null()).output().ok()?;
    let s = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (out.status.success() && !s.is_empty()).then_some(s)
}

/// Removes every path git tracks something in: a `build/` with sources in it isn't output.
fn drop_tracked(root: &Path, items: &mut Vec<Item>) {
    let inside: Vec<String> = items
        .iter()
        .filter(|i| i.cmd.is_none())
        .flat_map(|i| &i.paths)
        .filter_map(|p| p.strip_prefix(root).ok())
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    if inside.is_empty() {
        return;
    }
    let Ok(out) = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["ls-files", "-z", "--"])
        .args(&inside)
        .stderr(Stdio::null())
        .output()
    else {
        return;
    };
    if !out.status.success() {
        return;
    }
    let tracked: Vec<PathBuf> = out.stdout.split(|&b| b == 0).filter(|s| !s.is_empty()).map(|s| root.join(String::from_utf8_lossy(s).as_ref())).collect();
    for item in items.iter_mut().filter(|i| i.cmd.is_none()) {
        item.paths.retain(|p| !tracked.iter().any(|t| t.starts_with(p)));
    }
    items.retain(|i| i.cmd.is_some() || !i.paths.is_empty());
}

/// Like `du`: allocated blocks, hard links counted once (cargo links each binary twice).
fn disk_usage(path: &Path, seen: &mut HashSet<(u64, u64)>) -> u64 {
    let Ok(m) = fs::symlink_metadata(path) else { return 0 };
    if !m.is_dir() && m.nlink() > 1 && !seen.insert((m.dev(), m.ino())) {
        return 0;
    }
    let mut n = m.blocks() * 512;
    if m.is_dir()
        && let Ok(rd) = fs::read_dir(path)
    {
        n += rd.flatten().map(|e| disk_usage(&e.path(), seen)).sum::<u64>();
    }
    n
}

/// `echo` line for the end of the run, so the output pane says what happened.
pub fn done_line(labels: &[&str], size: Option<u64>) -> String {
    let what = labels.join(", ");
    let msg = match size {
        Some(n) if n > 0 => format!("cleaned {what} — {} freed", crate::browser::human_size(n)),
        _ => format!("cleaned {what}"),
    };
    format!("echo {}", quote_str(&msg))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offers_output_but_not_tracked_folders() {
        let root = std::env::temp_dir().join(format!("mainframe-cleanup-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for d in ["node_modules/x", "dist", "build", "src/__pycache__"] {
            fs::create_dir_all(root.join(d)).unwrap();
        }
        fs::write(root.join("package.json"), r#"{"scripts":{"clean":"rm -rf dist"}}"#).unwrap();
        fs::write(root.join("pyproject.toml"), "[project]\nname = \"fx\"\n").unwrap();
        fs::write(root.join("node_modules/x/a.js"), vec![0u8; 50_000]).unwrap();
        fs::write(root.join("build/keep.sh"), "echo source\n").unwrap();
        let git = |args: &[&str]| Command::new("git").arg("-C").arg(&root).args(args).output().unwrap();
        git(&["init", "-q"]);
        git(&["add", "build/keep.sh"]);

        let items = scan(&project::detect(&root).unwrap());
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        fs::remove_dir_all(&root).unwrap();

        assert_eq!(labels, ["node_modules", "dist", "__pycache__", "npm clean"]);
        assert!(items[0].size.unwrap() >= 50_000);
        assert_eq!(items[0].command(&root), "rm -rf -- node_modules");
        assert_eq!(items[3].command(&root), "npm run clean");
        assert_eq!(items[3].size, None);
    }
}
