//! Everything a project knows how to run, for the task picker: the build tool's usual
//! commands, its binaries and examples, package.json scripts, make targets, just recipes.

use std::{
    collections::BTreeSet,
    fs,
    path::Path,
    process::{Command, Stdio},
};

use crate::project::{Kind, Op, Project, quote_str, which};

pub struct Target {
    /// Where it comes from: cargo, npm, make, just, …
    pub group: &'static str,
    pub name: String,
    pub cmd: String,
}

pub fn list(p: &Project) -> Vec<Target> {
    let root = &p.root;
    let mut out = Vec::new();
    let mut add = |group: &'static str, name: &str, cmd: String| {
        out.push(Target { group, name: name.to_string(), cmd });
    };
    match p.kind {
        Kind::Cargo => {
            for (name, cmd) in [
                ("run", "cargo run"),
                ("build", "cargo build"),
                ("release", "cargo build --release"),
                ("test", "cargo test"),
                ("check", "cargo check --all-targets"),
                ("clippy", "cargo clippy --all-targets"),
                ("fmt", "cargo fmt"),
                ("doc", "cargo doc --no-deps"),
                ("update", "cargo update"),
            ] {
                add("cargo", name, cmd.into());
            }
            let bins = cargo_targets(root, "bin");
            if bins.len() > 1 {
                for b in &bins {
                    add("bin", b, format!("cargo run --bin {}", quote_str(b)));
                }
            }
            for e in cargo_targets(root, "example") {
                add("example", &e, format!("cargo run --example {}", quote_str(&e)));
            }
        }
        Kind::Go => {
            for (name, cmd) in [
                ("run", "go run ."),
                ("build", "go build ./..."),
                ("test", "go test ./..."),
                ("vet", "go vet ./..."),
                ("fmt", "gofmt -l -w ."),
                ("tidy", "go mod tidy"),
            ] {
                add("go", name, cmd.into());
            }
            for dir in subdirs(&root.join("cmd")) {
                add("cmd", &dir, format!("go run ./cmd/{}", quote_str(&dir)));
            }
        }
        Kind::Python => {
            let run = python_runner(root);
            for s in python_scripts(root) {
                add("script", &s, format!("{run}{}", quote_str(&s)));
            }
            for op in [Op::Run, Op::Test, Op::Build] {
                if let Ok(cmd) = python_plan(op, root) {
                    add("python", op.verb(), cmd);
                }
            }
        }
        Kind::Zig => {
            for (name, cmd) in [("build", "zig build"), ("run", "zig build run"), ("test", "zig build test")] {
                add("zig", name, cmd.into());
            }
        }
        Kind::CMake => {
            for (name, cmd) in [
                ("configure", "cmake -S . -B build"),
                ("build", "cmake --build build"),
                ("test", "ctest --test-dir build --output-on-failure"),
                ("clean", "cmake --build build --target clean"),
            ] {
                add("cmake", name, cmd.into());
            }
        }
        Kind::Meson => {
            for (name, cmd) in [
                ("setup", "meson setup build"),
                ("compile", "meson compile -C build"),
                ("test", "meson test -C build"),
            ] {
                add("meson", name, cmd.into());
            }
        }
        Kind::Npm | Kind::Make | Kind::Just => {}
    }

    // These can sit next to any build tool, so look for them in every project.
    if root.join("package.json").is_file() {
        let pm = package_manager(root);
        for s in npm_scripts(root) {
            add(pm, &s, format!("{pm} run {}", quote_str(&s)));
        }
        add(pm, "install", format!("{pm} install"));
    }
    if let Some(mf) = ["GNUmakefile", "makefile", "Makefile"].iter().map(|f| root.join(f)).find(|f| f.is_file()) {
        for t in make_targets(&fs::read_to_string(mf).unwrap_or_default()) {
            add("make", &t, format!("make {}", quote_str(&t)));
        }
    }
    if ["justfile", "Justfile", ".justfile"].iter().any(|f| root.join(f).is_file()) {
        for r in just_recipes(root) {
            add("just", &r, format!("just {}", quote_str(&r)));
        }
    }
    out
}

/// Binaries or examples: `[[bin]]`/`[[example]]` entries, `src/bin/*` (or `examples/*`),
/// and for binaries the package itself when it has a `src/main.rs`.
fn cargo_targets(root: &Path, kind: &str) -> Vec<String> {
    let mut names = BTreeSet::new();
    let manifest: toml::Table = fs::read_to_string(root.join("Cargo.toml"))
        .ok()
        .and_then(|t| toml::from_str(&t).ok())
        .unwrap_or_default();
    if let Some(list) = manifest.get(kind).and_then(|v| v.as_array()) {
        names.extend(list.iter().filter_map(|t| t.get("name")?.as_str()).map(String::from));
    }
    let dir = if kind == "bin" { root.join("src/bin") } else { root.join("examples") };
    if let Ok(rd) = fs::read_dir(&dir) {
        for e in rd.flatten() {
            let path = e.path();
            if (path.extension().is_some_and(|x| x == "rs") || path.join("main.rs").is_file())
                && let Some(stem) = path.file_stem()
            {
                names.insert(stem.to_string_lossy().into_owned());
            }
        }
    }
    if kind == "bin"
        && root.join("src/main.rs").is_file()
        && let Some(pkg) = manifest.get("package").and_then(|p| p.get("name")?.as_str())
    {
        names.insert(pkg.to_string());
    }
    names.into_iter().collect()
}

fn subdirs(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.path().is_dir())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

fn package_manager(root: &Path) -> &'static str {
    if root.join("pnpm-lock.yaml").is_file() {
        "pnpm"
    } else if root.join("yarn.lock").is_file() {
        "yarn"
    } else if root.join("bun.lockb").is_file() || root.join("bun.lock").is_file() {
        "bun"
    } else {
        "npm"
    }
}

/// package.json scripts, in the order they're written.
fn npm_scripts(root: &Path) -> Vec<String> {
    let Ok(text) = fs::read_to_string(root.join("package.json")) else { return Vec::new() };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else { return Vec::new() };
    json.get("scripts")
        .and_then(|s| s.as_object())
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default()
}

/// Explicit targets of a Makefile, skipping pattern rules, special targets and variables.
fn make_targets(text: &str) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for line in text.lines() {
        if line.starts_with(['\t', ' ', '.', '#']) {
            continue;
        }
        let Some((lhs, rhs)) = line.split_once(':') else { continue };
        if rhs.starts_with([':', '=']) || lhs.contains(['=', '$', '%']) {
            continue;
        }
        for t in lhs.split_whitespace() {
            if t.chars().all(|c| c.is_ascii_alphanumeric() || "_-./".contains(c)) && seen.insert(t.to_string()) {
                out.push(t.to_string());
            }
        }
    }
    out
}

fn just_recipes(root: &Path) -> Vec<String> {
    if which("just").is_none() {
        return Vec::new();
    }
    Command::new("just")
        .arg("--summary")
        .current_dir(root)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).split_whitespace().map(String::from).collect())
        .unwrap_or_default()
}

fn pyproject(root: &Path) -> toml::Table {
    fs::read_to_string(root.join("pyproject.toml"))
        .ok()
        .and_then(|t| toml::from_str(&t).ok())
        .unwrap_or_default()
}

pub fn python_name(root: &Path) -> Option<String> {
    let t = pyproject(root);
    let name = t.get("project").and_then(|p| p.get("name")).or_else(|| t.get("tool")?.get("poetry")?.get("name"));
    Some(name?.as_str()?.to_string())
}

/// `[project.scripts]` and `[tool.poetry.scripts]` entry points.
fn python_scripts(root: &Path) -> Vec<String> {
    let t = pyproject(root);
    let tables = [
        t.get("project").and_then(|p| p.get("scripts")),
        t.get("tool").and_then(|t| t.get("poetry")?.get("scripts")),
    ];
    tables
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_table())
        .flat_map(|m| m.keys().cloned())
        .collect()
}

/// Prefix that runs things inside the project's environment ("uv run ", "poetry run ", or nothing).
fn python_runner(root: &Path) -> &'static str {
    if root.join("uv.lock").is_file() {
        "uv run "
    } else if root.join("poetry.lock").is_file() {
        "poetry run "
    } else if which("uv").is_some() && root.join("pyproject.toml").is_file() {
        "uv run "
    } else {
        ""
    }
}

pub fn python_plan(op: Op, root: &Path) -> Result<String, String> {
    let run = python_runner(root);
    let py = if run.is_empty() { "python3" } else { "python" };
    Ok(match op {
        Op::Run => {
            if let Some(s) = python_scripts(root).first() {
                format!("{run}{}", quote_str(s))
            } else if let Some(main) = ["main.py", "app.py", "__main__.py"].into_iter().find(|f| root.join(f).is_file()) {
                format!("{run}{py} {main}")
            } else {
                return Err("no script entry point or main.py — T lists the python tasks".into());
            }
        }
        Op::Test => format!("{run}{py} -m pytest"),
        Op::Build | Op::Release if run == "uv run " => "uv build".into(),
        Op::Build | Op::Release if run == "poetry run " => "poetry build".into(),
        Op::Build | Op::Release => format!("{py} -m build"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn makefile() {
        let mf = "CC := gcc\nFLAGS = -O2\n.PHONY: all clean\nall: main\nmain: main.o util.o\n\t$(CC) -o $@ $^\n\
                  %.o: %.c\n\t$(CC) -c $<\nclean:\n\trm -f *.o\ninstall test: all\nall: extra\n";
        assert_eq!(make_targets(mf), ["all", "main", "clean", "install", "test"]);
    }
}
