//! Figures out what "run", "build", "install" etc. mean for the current selection.

use std::{
    env, fs,
    path::{Path, PathBuf},
};

use crate::{browser::Entry, config::Config, targets};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Cargo,
    Go,
    Npm,
    Python,
    Zig,
    CMake,
    Meson,
    Make,
    Just,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Cargo => "cargo",
            Kind::Go => "go",
            Kind::Npm => "npm",
            Kind::Python => "python",
            Kind::Zig => "zig",
            Kind::CMake => "cmake",
            Kind::Meson => "meson",
            Kind::Make => "make",
            Kind::Just => "just",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Project {
    pub kind: Kind,
    pub root: PathBuf,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Run,
    Build,
    Release,
    Test,
}

impl Op {
    pub fn verb(self) -> &'static str {
        match self {
            Op::Run => "run",
            Op::Build => "build",
            Op::Release => "release build",
            Op::Test => "test",
        }
    }
}

/// A shell command ready to execute.
#[derive(Debug, Clone)]
pub struct Plan {
    pub cmd: String,
    pub cwd: PathBuf,
}

/// Files that make a directory a project, most specific first.
const MARKERS: &[(Kind, &[&str])] = &[
    (Kind::Cargo, &["Cargo.toml"]),
    (Kind::Go, &["go.mod"]),
    (Kind::Npm, &["package.json"]),
    (Kind::Python, &["pyproject.toml", "setup.py"]),
    (Kind::Zig, &["build.zig"]),
    (Kind::CMake, &["CMakeLists.txt"]),
    (Kind::Meson, &["meson.build"]),
    (Kind::Make, &["Makefile", "makefile", "GNUmakefile"]),
    (Kind::Just, &["justfile", "Justfile", ".justfile"]),
];

/// Walks up from `start` looking for a project marker. Stops at $HOME so a stray
/// Makefile in your home dir doesn't hijack everything below it.
pub fn detect(start: &Path) -> Option<Project> {
    let home = crate::config::home();
    let mut dir = Some(start);
    while let Some(d) = dir {
        if d == home {
            break;
        }
        let kind = MARKERS.iter().find(|(_, files)| files.iter().any(|f| d.join(f).is_file())).map(|&(k, _)| k);
        if let Some(kind) = kind {
            let name = match kind {
                Kind::Cargo => cargo_name(d),
                Kind::Python => targets::python_name(d),
                _ => None,
            }
            .or_else(|| d.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_default();
            return Some(Project { kind, root: d.to_path_buf(), name });
        }
        dir = d.parent();
    }
    None
}

fn cargo_name(root: &Path) -> Option<String> {
    let text = fs::read_to_string(root.join("Cargo.toml")).ok()?;
    let value: toml::Table = toml::from_str(&text).ok()?;
    Some(value.get("package")?.get("name")?.as_str()?.to_string())
}

/// What `op` means for `target` (the selected entry), or for `cwd` when nothing is selected.
pub fn plan(op: Op, target: Option<&Entry>, cwd: &Path, cfg: &Config) -> Result<Plan, String> {
    let file = target.filter(|e| !e.is_dir);
    let start = match target {
        Some(e) if e.is_dir => e.path.clone(),
        _ => cwd.to_path_buf(),
    };
    let project = detect(&start);

    if let Some(f) = file
        && let Some(plan) = file_plan(op, f, project.as_ref(), cfg) {
            return Ok(plan);
        }

    let Some(p) = project else {
        return Err(match file {
            Some(f) => format!("don't know how to {} {}", op.verb(), f.name),
            None => format!("no project here to {}", op.verb()),
        });
    };
    if p.kind == Kind::Python {
        return targets::python_plan(op, &p.root).map(|cmd| Plan { cmd, cwd: p.root.clone() });
    }
    let cmd = match (p.kind, op) {
        (Kind::Cargo, Op::Run) => "cargo run",
        (Kind::Cargo, Op::Build) => "cargo build",
        (Kind::Cargo, Op::Release) => "cargo build --release",
        (Kind::Cargo, Op::Test) => "cargo test",
        (Kind::Go, Op::Run) => "go run .",
        (Kind::Go, Op::Build | Op::Release) => "go build ./...",
        (Kind::Go, Op::Test) => "go test ./...",
        (Kind::Npm, Op::Run) => "npm start",
        (Kind::Npm, Op::Build | Op::Release) => "npm run build",
        (Kind::Npm, Op::Test) => "npm test",
        (Kind::Make, Op::Run) => "make run",
        (Kind::Make, Op::Build | Op::Release) => "make",
        (Kind::Make, Op::Test) => "make test",
        (Kind::Zig, Op::Run) => "zig build run",
        (Kind::Zig, Op::Build) => "zig build",
        (Kind::Zig, Op::Release) => "zig build -Doptimize=ReleaseFast",
        (Kind::Zig, Op::Test) => "zig build test",
        (Kind::CMake, Op::Run) => return Err("cmake doesn't say what to run — T lists its tasks".into()),
        (Kind::CMake, Op::Build) => "cmake -S . -B build && cmake --build build",
        (Kind::CMake, Op::Release) => "cmake -S . -B build-release -DCMAKE_BUILD_TYPE=Release && cmake --build build-release",
        (Kind::CMake, Op::Test) => "ctest --test-dir build --output-on-failure",
        (Kind::Meson, Op::Run) => return Err("meson doesn't say what to run — T lists its tasks".into()),
        (Kind::Meson, Op::Build) => "{ [ -d build ] || meson setup build; } && meson compile -C build",
        (Kind::Meson, Op::Release) => {
            "{ [ -d build-release ] || meson setup build-release --buildtype=release; } && meson compile -C build-release"
        }
        (Kind::Meson, Op::Test) => "meson test -C build",
        (Kind::Just, Op::Run) => "just",
        (Kind::Just, Op::Build | Op::Release) => "just build",
        (Kind::Just, Op::Test) => "just test",
        (Kind::Python, _) => unreachable!(),
    };
    Ok(Plan { cmd: cmd.into(), cwd: p.root })
}

fn file_plan(op: Op, f: &Entry, project: Option<&Project>, cfg: &Config) -> Option<Plan> {
    let dir = f.path.parent().unwrap_or(Path::new("/")).to_path_buf();
    // Commands run from the file's own directory, so the bare name is enough.
    let q = quote_str(&f.name);
    let ext = f.path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();

    if op != Op::Run {
        return None;
    }

    // A .rs file inside a cargo project: run its bin/example target.
    if let Some(p) = project.filter(|p| p.kind == Kind::Cargo && ext == "rs") {
        let stem = f.path.file_stem()?.to_string_lossy();
        let parent = f.path.parent()?;
        let target = if parent == p.root.join("src/bin") {
            format!(" --bin {}", quote_str(&stem))
        } else if parent == p.root.join("examples") {
            format!(" --example {}", quote_str(&stem))
        } else {
            String::new()
        };
        return Some(Plan { cmd: format!("cargo run{target}"), cwd: p.root.clone() });
    }

    if let Some(tpl) = cfg.runners.get(&ext) {
        return Some(Plan { cmd: expand_file(tpl, f), cwd: dir });
    }

    let tmp = env::temp_dir().join(format!("mainframe-{}", f.path.file_stem()?.to_string_lossy()));
    let tmp = quote(&tmp);
    let cmd = match ext.as_str() {
        "sh" | "bash" => format!("bash {q}"),
        "zsh" => format!("zsh {q}"),
        "fish" => format!("fish {q}"),
        "py" => format!("python3 {q}"),
        "js" | "mjs" | "cjs" => format!("node {q}"),
        "ts" => format!("npx --yes tsx {q}"),
        "rb" => format!("ruby {q}"),
        "lua" => format!("lua {q}"),
        "pl" => format!("perl {q}"),
        "php" => format!("php {q}"),
        "go" => format!("go run {q}"),
        "rs" => format!("rustc --edition 2024 -o {tmp} {q} && {tmp}"),
        "c" => format!("cc -Wall -o {tmp} {q} && {tmp}"),
        "cpp" | "cc" | "cxx" => format!("c++ -Wall -o {tmp} {q} && {tmp}"),
        _ if f.is_exec => format!("./{q}"),
        _ => return None,
    };
    Some(Plan { cmd, cwd: dir })
}

fn expand_file(tpl: &str, f: &Entry) -> String {
    tpl.replace("{file}", &quote(&f.path)).replace("{name}", &quote_str(&f.name))
}

pub fn quote(p: &Path) -> String {
    quote_str(&p.to_string_lossy())
}

/// POSIX single-quote escaping, skipped when the string is obviously safe.
pub fn quote_str(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "/._-+=:,@%".contains(c)) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

/// Finds an executable on $PATH.
pub fn which(name: &str) -> Option<PathBuf> {
    env::split_paths(&env::var_os("PATH")?)
        .map(|d| d.join(name))
        .find(|p| p.is_file())
}
