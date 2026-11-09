//! What `C` can clear in a project: build output, installed dependencies and caches that the
//! build tool brings back by itself. Anything git tracks is never offered.

use std::{
    collections::{BTreeMap, HashSet},
    fs::{self, File},
    os::{
        fd::AsRawFd,
        unix::fs::{FileExt, MetadataExt},
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use crate::{
    browser::human_size,
    project::{self, Kind, Project, quote, quote_str, which},
    targets,
};

pub struct Item {
    /// The other worktree this belongs to (its folder name); None for the project itself.
    pub place: Option<String>,
    /// The project root it belongs to (another worktree's for those).
    pub root: PathBuf,
    pub label: String,
    /// What it is and what brings it back.
    pub note: &'static str,
    /// What gets removed (and measured).
    pub paths: Vec<PathBuf>,
    /// Runs instead of `rm -rf` on `paths` (`cargo clean`, `make clean`).
    pub cmd: Option<String>,
    /// Room the paths take; None for a command whose effect isn't known up front.
    pub size: Option<Usage>,
}

impl Item {
    fn dirs(root: &Path, label: impl Into<String>, note: &'static str, paths: Vec<PathBuf>) -> Self {
        Item { place: None, root: root.to_path_buf(), label: label.into(), note, paths, cmd: None, size: None }
    }

    fn tool(root: &Path, label: impl Into<String>, note: &'static str, paths: Vec<PathBuf>, cmd: impl Into<String>) -> Self {
        Item { cmd: Some(cmd.into()), ..Item::dirs(root, label, note, paths) }
    }

    /// The label, with the worktree in front for other worktrees.
    pub fn title(&self) -> String {
        match &self.place {
            Some(w) => format!("{w} · {}", self.label),
            None => self.label.clone(),
        }
    }

    /// The shell command, for running in `cwd`.
    pub fn command(&self, cwd: &Path) -> String {
        match &self.cmd {
            Some(c) if self.root == cwd => c.clone(),
            Some(c) => format!("(cd {} && {c})", quote(&self.root)),
            None => format!("rm -rf -- {}", self.paths.iter().map(|p| show(p, cwd)).collect::<Vec<_>>().join(" ")),
        }
    }

    /// The paths, relative to the root they belong to.
    pub fn location(&self) -> String {
        match self.paths.len() {
            0 => String::new(),
            1..=3 => self.paths.iter().map(|p| show(p, &self.root)).collect::<Vec<_>>().join(" "),
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

/// Room some paths take: the files' size (what `du` and `dua` show), and where btrfs
/// stores part of it compressed, an estimate of what that is on disk.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    pub bytes: u64,
    pub disk: Option<u64>,
}

impl Usage {
    /// What deleting it should free.
    pub fn freed(&self) -> u64 {
        self.disk.unwrap_or(self.bytes)
    }

    /// For the size column: the on-disk estimate when there is one.
    pub fn short(&self) -> String {
        match self.disk {
            Some(d) => format!("~{}", human_size(d)),
            None => human_size(self.bytes),
        }
    }

    pub fn long(&self) -> String {
        match self.disk {
            Some(d) => format!("~{} on disk, {} of files", human_size(d), human_size(self.bytes)),
            None => human_size(self.bytes),
        }
    }

    fn add(self, o: Usage) -> Usage {
        let disk = (self.disk.is_some() || o.disk.is_some()).then(|| self.freed() + o.freed());
        Usage { bytes: self.bytes + o.bytes, disk }
    }
}

/// The items' sizes added up without touching the disk again; one inside another
/// (the incremental cache inside the debug build) counts once. Unmeasured ones add nothing.
pub fn total(items: &[&Item]) -> Usage {
    let inside = |a: &Item, b: &Item| {
        a.paths != b.paths && !a.paths.is_empty() && a.paths.iter().all(|p| b.paths.iter().any(|q| p.starts_with(q)))
    };
    items
        .iter()
        .filter(|a| !items.iter().any(|b| inside(a, b)))
        .filter_map(|i| i.size)
        .fold(Usage::default(), Usage::add)
}

/// Walks the paths once. Hard links count once (cargo links each binary twice).
pub fn measure(paths: &[PathBuf]) -> Usage {
    let mut m = Meter { rng: 0x9E37_79B9_7F4A_7C15, ..Meter::default() };
    for p in paths {
        let fiemap = is_btrfs(p);
        m.walk(p, fiemap);
    }
    m.usage()
}

/// Compressed extents sampled per measurement to estimate the ratio (each up to 128K).
const SAMPLES: usize = 32;
/// btrfs compresses in chunks of this, each stored in whole sectors.
const SECTOR: u64 = 4096;

#[derive(Default)]
struct Meter {
    seen: HashSet<(u64, u64)>,
    bytes: u64,
    /// Uncompressed length of the extents btrfs stores compressed.
    encoded: u64,
    encoded_extents: u64,
    /// (file, offset, length) of some compressed extents, picked evenly (reservoir sampling).
    samples: Vec<(PathBuf, u64, u64)>,
    rng: u64,
}

impl Meter {
    fn walk(&mut self, path: &Path, fiemap: bool) {
        let Ok(m) = fs::symlink_metadata(path) else { return };
        if !m.is_dir() && m.nlink() > 1 && !self.seen.insert((m.dev(), m.ino())) {
            return;
        }
        self.bytes += m.blocks() * 512;
        if m.is_dir() {
            if let Ok(rd) = fs::read_dir(path) {
                for e in rd.flatten() {
                    self.walk(&e.path(), fiemap);
                }
            }
        } else if fiemap && m.is_file() && m.len() > 0 {
            self.extents(path, m.len());
        }
    }

    /// Asks the kernel which parts of the file are stored compressed (FIEMAP works without
    /// root; their size on disk isn't part of the answer, hence the sampling).
    fn extents(&mut self, path: &Path, len: u64) {
        const FS_IOC_FIEMAP: u64 = 0xC020_660B;
        const LAST: u32 = 0x1;
        const ENCODED: u32 = 0x8;
        const INLINE: u32 = 0x200;
        const COUNT: usize = 128;
        let Ok(f) = File::open(path) else { return };
        let mut start = 0u64;
        while start < len {
            // struct fiemap (32 bytes) + COUNT struct fiemap_extent (56 bytes each).
            let mut buf = vec![0u8; 32 + 56 * COUNT];
            buf[0..8].copy_from_slice(&start.to_ne_bytes());
            buf[8..16].copy_from_slice(&(len - start).to_ne_bytes());
            buf[24..28].copy_from_slice(&(COUNT as u32).to_ne_bytes());
            // SAFETY: buf is a struct fiemap with room for fm_extent_count extents.
            if unsafe { libc::ioctl(f.as_raw_fd(), FS_IOC_FIEMAP as _, buf.as_mut_ptr()) } != 0 {
                return;
            }
            let u64_at = |o: usize| u64::from_ne_bytes(buf[o..o + 8].try_into().unwrap());
            let u32_at = |o: usize| u32::from_ne_bytes(buf[o..o + 4].try_into().unwrap());
            let mapped = u32_at(20) as usize;
            if mapped == 0 {
                return;
            }
            for i in 0..mapped.min(COUNT) {
                let o = 32 + 56 * i;
                let (logical, length, flags) = (u64_at(o), u64_at(o + 16), u32_at(o + 40));
                if flags & ENCODED != 0 && flags & INLINE == 0 {
                    let length = length.min((len.saturating_sub(logical)).div_ceil(SECTOR) * SECTOR);
                    self.encoded += length;
                    self.sample(path, logical, length);
                }
                start = logical + length.max(1);
                if flags & LAST != 0 {
                    return;
                }
            }
        }
    }

    fn sample(&mut self, path: &Path, offset: u64, length: u64) {
        self.encoded_extents += 1;
        if self.samples.len() < SAMPLES {
            self.samples.push((path.to_path_buf(), offset, length));
            return;
        }
        // xorshift: deterministic, and good enough to spread the picks.
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        let slot = self.rng % self.encoded_extents;
        if (slot as usize) < SAMPLES {
            self.samples[slot as usize] = (path.to_path_buf(), offset, length);
        }
    }

    /// Compresses the samples the way btrfs does (zstd level 3, the default, per chunk,
    /// rounded to sectors) and applies that ratio to everything stored compressed.
    fn usage(self) -> Usage {
        let (mut raw, mut packed) = (0u64, 0u64);
        for (path, offset, length) in &self.samples {
            let Ok(f) = File::open(path) else { continue };
            let mut data = vec![0u8; *length as usize];
            let Ok(n) = f.read_at(&mut data, *offset) else { continue };
            if n == 0 {
                continue;
            }
            data.truncate(n);
            let sectors = (n as u64).div_ceil(SECTOR) * SECTOR;
            let compressed = zstd::bulk::compress(&data, 3).map_or(sectors, |c| c.len() as u64);
            raw += sectors;
            packed += (compressed.div_ceil(SECTOR) * SECTOR).min(sectors);
        }
        if raw == 0 || self.encoded == 0 {
            return Usage { bytes: self.bytes, disk: None };
        }
        let encoded = self.encoded.min(self.bytes);
        let on_disk = (encoded as u128 * packed as u128 / raw as u128) as u64;
        Usage { bytes: self.bytes, disk: Some(self.bytes - encoded + on_disk) }
    }
}

fn is_btrfs(path: &Path) -> bool {
    const BTRFS_SUPER_MAGIC: i64 = 0x9123_683E;
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()) else { return false };
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    // SAFETY: c is a valid C string and st a statfs to fill.
    unsafe { libc::statfs(c.as_ptr(), &mut st) == 0 && st.f_type as i64 == BTRFS_SUPER_MAGIC }
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

/// Everything there is to clean in the folder (a project, or one holding projects) and
/// in the same folder in every other worktree of its repo, measured. Slow on big trees,
/// so it runs off the UI thread.
pub fn scan(root: &Path) -> Vec<Item> {
    let others = other_worktrees(root);
    let skip: HashSet<PathBuf> = others.iter().map(|(_, p)| p.clone()).collect();
    let mut items = scan_tree(root, None, true, &skip);
    for (name, root) in others {
        items.extend(scan_tree(&root, Some(&name), false, &skip));
    }
    // Workspace members share the workspace's target, which is listed once.
    let mut seen = HashSet::new();
    items.retain(|i| i.paths.is_empty() || seen.insert(i.paths.clone()));
    items
}

/// Folders never walked into for nested projects: huge, and nothing in them is ours.
const SKIP: &[&str] = &["node_modules", "target", "vendor", "venv", "site-packages", "__pycache__", "bower_components", "Pods"];

/// How far and how much the walk for nested projects goes, so a huge tree can't stall it.
const MAX_DEPTH: usize = 8;
const MAX_DIRS: usize = 50_000;

/// The project, then every project nested below it (a `frontend/` with its own
/// `package.json`, workspace members, examples) and stray cache folders (`CACHEDIR.TAG`,
/// e.g. a `target` whose `Cargo.toml` is gone). Their rows are prefixed with where they are.
fn scan_tree(root: &Path, place: Option<&str>, shared: bool, skip: &HashSet<PathBuf>) -> Vec<Item> {
    let at = |rel: &Path| {
        let rel = rel.to_string_lossy();
        match (place, rel.is_empty()) {
            (Some(p), true) => Some(p.to_string()),
            (Some(p), false) => Some(format!("{p}/{rel}")),
            (None, true) => None,
            (None, false) => Some(rel.into_owned()),
        }
    };
    let top = project::detect(root).filter(|p| p.root == root);
    let mut items: Vec<Item> = top
        .map(|p| scan_one(&p, shared).into_iter().map(|i| Item { place: at(Path::new("")), ..i }).collect())
        .unwrap_or_default();
    let mut covered: Vec<PathBuf> = items.iter().flat_map(|i| i.paths.clone()).collect();
    let mut budget = MAX_DIRS;
    let mut stack = vec![(root.to_path_buf(), 0)];
    while let Some((dir, depth)) = stack.pop() {
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        let mut subdirs: Vec<PathBuf> = rd
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .filter(|e| {
                let name = e.file_name();
                let name = name.to_string_lossy();
                !name.starts_with('.') && !SKIP.contains(&name.as_ref())
                    || e.path().join("CACHEDIR.TAG").is_file()
            })
            .map(|e| e.path())
            .collect();
        subdirs.sort_by_key(|p| p.to_string_lossy().to_lowercase());
        let mut descend = Vec::new();
        for sub in subdirs {
            if budget == 0 {
                break;
            }
            budget -= 1;
            if skip.contains(&sub) || covered.iter().any(|c| sub.starts_with(c)) {
                continue;
            }
            let rel = sub.strip_prefix(root).unwrap_or(&sub).to_path_buf();
            if sub.join("CACHEDIR.TAG").is_file() {
                let parent = rel.parent().unwrap_or(Path::new(""));
                let name = rel.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let mut item = Item::dirs(&sub, name, "marked as a cache folder — the tool that made it builds it again", vec![sub.clone()]);
                item.place = at(parent);
                item.root = root.join(parent);
                item.size = Some(measure(&item.paths));
                covered.push(sub);
                items.push(item);
                continue;
            }
            if !project::kinds_at(&sub).is_empty()
                && let Some(p) = project::detect(&sub)
            {
                let found: Vec<Item> = scan_one(&p, false).into_iter().map(|i| Item { place: at(&rel), ..i }).collect();
                covered.extend(found.iter().flat_map(|i| i.paths.clone()));
                items.extend(found);
                // A repo in here: its other worktrees too, wherever they live.
                if sub.join(".git").is_dir() {
                    let here = at(&rel).unwrap_or_default();
                    for (name, wt) in other_worktrees(&sub).into_iter().filter(|(_, w)| !skip.contains(w)) {
                        items.extend(scan_tree(&wt, Some(&format!("{here} ⎇ {name}")), false, skip));
                    }
                }
            }
            if depth + 1 < MAX_DEPTH {
                descend.push((sub, depth + 1));
            }
        }
        // Popped in name order.
        stack.extend(descend.into_iter().rev());
    }
    items
}

/// The project's folder in each other worktree of its repo (folder name, path), so a
/// project in a subfolder of the repo maps to the same subfolder there.
fn other_worktrees(root: &Path) -> Vec<(String, PathBuf)> {
    let Some(top) = command_output("git", &["rev-parse", "--show-toplevel"], root).map(PathBuf::from) else {
        return Vec::new();
    };
    let Ok(rel) = root.strip_prefix(&top) else { return Vec::new() };
    let list = command_output("git", &["worktree", "list", "--porcelain"], root).unwrap_or_default();
    list.lines()
        .filter_map(|l| l.strip_prefix("worktree "))
        .map(PathBuf::from)
        .filter(|w| *w != top)
        .map(|w| {
            let name = w.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            // join("") would add a trailing slash
            let dir = if rel.as_os_str().is_empty() { w } else { w.join(rel) };
            (name, dir)
        })
        .filter(|(_, p)| p.is_dir())
        .collect()
}

/// `shared`: also offer caches every project shares (only once, for the project itself).
fn scan_one(project: &Project, shared: bool) -> Vec<Item> {
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
                if shared {
                    go_cache(root, &mut items);
                }
            }
            Kind::Make | Kind::Just => {}
        }
    }

    let mut seen = HashSet::new();
    items.retain(|i| i.cmd.is_some() || i.paths.iter().all(|p| seen.insert(p.clone())));
    drop_tracked(root, &mut items);

    // The project's own clean target last: it may do anything, so it isn't measured.
    for t in targets::list(project).into_iter().filter(|t| t.name == "clean") {
        items.push(Item::tool(root, format!("{} clean", t.group), "the project's own clean target", Vec::new(), t.cmd));
    }

    for item in &mut items {
        if !item.paths.is_empty() {
            item.size = Some(measure(&item.paths));
        }
    }
    items
}

fn named(root: &Path, list: &[(&str, &'static str)], items: &mut Vec<Item>) {
    for &(name, note) in list {
        let p = root.join(name);
        if fs::symlink_metadata(&p).is_ok() {
            items.push(Item::dirs(root, name, note, vec![p]));
        }
    }
}

/// `__pycache__` anywhere in the tree and `*.egg-info` next to the sources.
fn python_extra(root: &Path, items: &mut Vec<Item>) {
    let mut found = Vec::new();
    let mut budget = 50_000;
    find_pycache(root, &mut found, &mut budget);
    if !found.is_empty() {
        items.push(Item::dirs(root, "__pycache__", "compiled bytecode — Python writes it again on import", found));
    }
    let egg: Vec<PathBuf> = [root.to_path_buf(), root.join("src")]
        .iter()
        .filter_map(|d| fs::read_dir(d).ok())
        .flat_map(|rd| rd.flatten().map(|e| e.path()))
        .filter(|p| p.is_dir() && p.extension().is_some_and(|x| x == "egg-info"))
        .collect();
    if !egg.is_empty() {
        items.push(Item::dirs(root, "*.egg-info", "packaging metadata from setuptools", egg));
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
fn go_cache(root: &Path, items: &mut Vec<Item>) {
    let Some(dir) = command_output("go", &["env", "GOCACHE"], Path::new("/")).map(PathBuf::from) else { return };
    if dir.is_dir() {
        items.push(Item::tool(root, "go build cache", "shared by all Go projects — rebuilt as needed", vec![dir], "go clean -cache"));
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
        items.push(Item::dirs(root, label, note, paths));
        if name == "debug" && !incremental.is_empty() {
            items.push(Item::dirs(
                root,
                "incremental cache",
                "incremental compilation cache, grows with every edit — dependencies stay built",
                incremental.clone(),
            ));
        }
    }
    let doc = target.join("doc");
    if doc.is_dir() {
        items.push(Item::dirs(root, "docs", "cargo doc output", vec![doc]));
    }
    items.push(Item::tool(root, "all of target", "cargo clean — everything cargo built for this project", vec![target], "cargo clean"));
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

/// What the items are, short enough for one line: their titles for a few, else counts
/// per kind ("debug builds ×28, incremental cache ×2").
pub fn summary(items: &[&Item]) -> String {
    if items.len() <= 3 {
        return items.iter().map(|i| i.title()).collect::<Vec<_>>().join(", ");
    }
    let mut kinds: Vec<(&str, usize)> = Vec::new();
    for i in items {
        match kinds.iter_mut().find(|(l, _)| *l == i.label) {
            Some((_, n)) => *n += 1,
            None => kinds.push((&i.label, 1)),
        }
    }
    kinds.iter().map(|(l, n)| if *n > 1 { format!("{l} ×{n}") } else { l.to_string() }).collect::<Vec<_>>().join(", ")
}

/// The shell script for a run: each step echoed, then what was freed on disk, measured
/// with `df` around it. That's the real number, after compression and snapshots.
pub fn script(steps: &[String], what: &str, expected: Option<Usage>) -> String {
    let mut out = String::from("#!/bin/sh\n# Written by Mainframe's cleanup (C).\n");
    out.push_str("avail() { df -B1 --output=avail . 2>/dev/null | tail -n1; }\nbefore=$(avail)\n");
    for step in steps {
        out.push_str(&format!("echo {}\n{step} || exit 1\n", quote_str(&format!("$ {step}"))));
    }
    out.push_str("sync -f . 2>/dev/null\nafter=$(avail)\n");
    out.push_str(&format!("what={}\n", quote_str(what)));
    out.push_str(concat!(
        "if [ -n \"$before\" ] && [ -n \"$after\" ]; then\n",
        "  freed=$((after - before)); [ \"$freed\" -lt 0 ] && freed=0\n",
        "  human=$(awk -v n=\"$freed\" 'BEGIN { split(\"B K M G T\", u); i = 1; while (n >= 1024 && i < 5) { n /= 1024; i++ }; printf(i == 1 ? \"%dB\" : \"%.1f%s\", n, u[i]) }')\n",
        "  echo \"cleaned $what — $human freed on disk\"\n",
    ));
    if let Some(u) = expected.filter(|u| u.freed() > 0) {
        out.push_str(&format!(
            "  if [ \"$freed\" -lt {} ]; then echo 'less than expected: btrfs snapshots may still hold the deleted files'; fi\n",
            u.freed() / 2
        ));
    }
    // The last test above may be false; that's no failure.
    out.push_str("else\n  echo \"cleaned $what\"\nfi\nexit 0\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offers_output_but_not_tracked_folders() {
        let root = std::env::temp_dir().join(format!("mainframe-cleanup-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for d in ["node_modules/x", "dist", "build", "src/__pycache__", "tools/web/node_modules", "old/target", ".hidden/p/node_modules"] {
            fs::create_dir_all(root.join(d)).unwrap();
        }
        fs::write(root.join("package.json"), r#"{"scripts":{"clean":"rm -rf dist"}}"#).unwrap();
        fs::write(root.join("pyproject.toml"), "[project]\nname = \"fx\"\n").unwrap();
        fs::write(root.join("node_modules/x/a.js"), vec![0u8; 50_000]).unwrap();
        fs::write(root.join("build/keep.sh"), "echo source\n").unwrap();
        // Nested: a project, a target left without its Cargo.toml, and one in a hidden folder.
        fs::write(root.join("tools/web/package.json"), "{}").unwrap();
        fs::write(root.join("old/target/CACHEDIR.TAG"), "Signature: 8a477f597d28d172789f06886806bc55").unwrap();
        fs::write(root.join(".hidden/p/package.json"), "{}").unwrap();
        let git = |args: &[&str]| Command::new("git").arg("-C").arg(&root).args(args).output().unwrap();
        git(&["init", "-q", "-b", "main"]);
        git(&["add", "build/keep.sh", "package.json", "pyproject.toml"]);
        git(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "init"]);
        // A linked worktree with its own build output.
        let wt = root.with_file_name(format!("{}-wt", root.file_name().unwrap().to_string_lossy()));
        let _ = fs::remove_dir_all(&wt);
        Command::new("git").arg("-C").arg(&root).args(["worktree", "add", "-q"]).arg(&wt).output().unwrap();
        fs::create_dir_all(wt.join("node_modules")).unwrap();

        let items = scan(&root);
        let titles: Vec<String> = items.iter().map(|i| i.title()).collect();
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&wt);

        let wt_name = wt.file_name().unwrap().to_string_lossy().into_owned();
        assert_eq!(
            titles,
            [
                "node_modules".to_string(),
                "dist".into(),
                "__pycache__".into(),
                "npm clean".into(),
                "old · target".into(),
                "tools/web · node_modules".into(),
                format!("{wt_name} · node_modules"),
                format!("{wt_name} · npm clean"),
            ]
        );
        assert!(items[0].size.unwrap().bytes >= 50_000);
        assert_eq!(items[0].command(&root), "rm -rf -- node_modules");
        assert_eq!(items[3].command(&root), "npm run clean");
        assert_eq!(items[3].size, None);
        assert_eq!(items[4].command(&root), "rm -rf -- old/target");
        assert_eq!(items[5].command(&root), "rm -rf -- tools/web/node_modules");
        assert_eq!(items[6].command(&root), format!("rm -rf -- {}", quote(&wt.join("node_modules"))));
        assert_eq!(items[7].command(&root), format!("(cd {} && npm run clean)", quote(&wt)));
    }
}

#[cfg(test)]
mod usage_tests {
    use super::*;

    fn item(paths: &[&str], bytes: u64, disk: Option<u64>) -> Item {
        let mut i = Item::dirs(Path::new("/p"), "x", "", paths.iter().map(PathBuf::from).collect());
        i.size = Some(Usage { bytes, disk });
        i
    }

    #[test]
    fn totals_count_nested_rows_once() {
        let debug = item(&["/p/target/debug"], 100, Some(60));
        let incr = item(&["/p/target/debug/incremental"], 40, Some(20));
        let release = item(&["/p/target/release"], 50, None);
        assert_eq!(total(&[&debug, &incr]), Usage { bytes: 100, disk: Some(60) });
        assert_eq!(total(&[&incr, &release]), Usage { bytes: 90, disk: Some(70) });
        assert_eq!(total(&[&release]), Usage { bytes: 50, disk: None });
    }

    #[test]
    fn script_succeeds_and_reports_what_was_freed() {
        let dir = std::env::temp_dir().join(format!("mainframe-script-{}", std::process::id()));
        fs::create_dir_all(dir.join("junk")).unwrap();
        fs::write(dir.join("junk/a"), vec![1u8; 100_000]).unwrap();
        let s = script(&["rm -rf -- junk".into()], "junk", Some(Usage { bytes: 1, disk: None }));
        let out = Command::new("sh").arg("-c").arg(&s).current_dir(&dir).output().unwrap();
        let gone = !dir.join("junk").exists();
        fs::remove_dir_all(&dir).unwrap();
        assert!(out.status.success());
        assert!(gone);
        assert!(String::from_utf8_lossy(&out.stdout).contains("cleaned junk"));
    }

    /// Only meaningful on a compressed btrfs (like the dev machine); skipped elsewhere.
    #[test]
    fn compressed_files_measure_smaller_on_disk() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target").join(format!("measure-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        if !is_btrfs(&dir) {
            fs::remove_dir_all(&dir).unwrap();
            return;
        }
        let text: String = (0..200_000).map(|i| format!("line {i} of very compressible build output\n")).collect();
        let f = dir.join("a.txt");
        fs::write(&f, &text).unwrap();
        File::open(&f).unwrap().sync_all().unwrap();
        let u = measure(std::slice::from_ref(&dir));
        fs::remove_dir_all(&dir).unwrap();
        if let Some(disk) = u.disk {
            assert!(disk < u.bytes / 3, "{u:?}");
        }
    }
}
