//! Finds installable executables in a project and describes what they were built for.

use crate::{
    cmd::{Builtin, Job, Step},
    desktop::DesktopPlan,
};

use std::{
    fs::{self, File},
    io::Read,
    path::{Component, Path, PathBuf},
    time::SystemTime,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Installed {
    No,
    Same,
    /// This build is newer than the installed binary.
    Newer,
    /// The installed binary is newer than this build.
    Older,
}

#[derive(Debug, Clone)]
pub struct Candidate {
    pub path: PathBuf,
    pub name: String,
    /// Path relative to the project root.
    pub rel: String,
    /// "release", "debug", "debug example", "bash script", "binary", …
    pub kind: String,
    /// Target triple when cross-compiled with cargo (`target/<triple>/…`).
    pub triple: Option<String>,
    /// "x86_64 · dynamic", "aarch64 · static", "windows exe", …
    pub arch: String,
    pub size: u64,
    pub modified: SystemTime,
    /// Older than the newest source file in the project.
    pub stale: bool,
    pub installed: Installed,
    /// Where `i` puts it: `<install_dir>/<name>`.
    pub dest: PathBuf,
    /// Another copy earlier on $PATH that runs instead of `dest`.
    pub shadowed_by: Option<PathBuf>,
    /// The project's .desktop entries that launch this binary.
    pub desktop: Vec<DesktopPlan>,
}

const MAX_DEPTH: usize = 7;
const MAX_FILES: usize = 50_000;

/// Directories never worth descending into.
const SKIP: &[&str] = &["node_modules", "__pycache__", "venv", "dist-newstyle"];
/// Cargo internals inside `target/` (test binaries, build scripts, caches).
const SKIP_IN_TARGET: &[&str] = &["deps", "build", "incremental", ".fingerprint", "doc", "tmp"];

pub fn scan(root: &Path, install_dir: &Path) -> Vec<Candidate> {
    let mut found = Vec::new();
    let mut newest_src = SystemTime::UNIX_EPOCH;
    let mut budget = MAX_FILES;
    walk(root, root, 0, &mut found, &mut newest_src, &mut budget);

    let mut out: Vec<Candidate> = found
        .into_iter()
        .filter_map(|(path, size, modified)| describe(root, path, size, modified, newest_src, install_dir))
        .collect();
    // Cargo outputs first (release before debug), newest first within each group.
    out.sort_by(|a, b| {
        let rank = |c: &Candidate| (!c.rel.starts_with("target/"), !c.kind.starts_with("release"));
        rank(a).cmp(&rank(b)).then(b.modified.cmp(&a.modified))
    });
    out
}

fn walk(
    dir: &Path,
    root: &Path,
    depth: usize,
    found: &mut Vec<(PathBuf, u64, SystemTime)>,
    newest_src: &mut SystemTime,
    budget: &mut usize,
) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    let in_target = dir.strip_prefix(root).is_ok_and(|r| r.starts_with("target"));
    for de in rd.flatten() {
        if *budget == 0 {
            return;
        }
        *budget -= 1;
        let name = de.file_name();
        let name = name.to_string_lossy();
        let Ok(ft) = de.file_type() else { continue };
        let path = de.path();

        if ft.is_dir() {
            let skip = name.starts_with('.')
                || SKIP.contains(&name.as_ref())
                || (in_target && SKIP_IN_TARGET.contains(&name.as_ref()));
            if !skip && depth < MAX_DEPTH {
                walk(&path, root, depth + 1, found, newest_src, budget);
            }
            continue;
        }
        if !ft.is_file() {
            continue;
        }
        let Ok(meta) = de.metadata() else { continue };
        let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        let exec = crate::platform::is_executable(&path, &meta) || name.ends_with(".exe");
        if exec && !is_library(&name) {
            found.push((path, meta.len(), modified));
        } else if !in_target && modified > *newest_src && is_cargo_input(root, &path) {
            *newest_src = modified;
        }
    }
}

/// Files that make cargo rebuild: Rust sources anywhere, anything under src/
/// (include_str! etc.), the manifest, lockfile and build script. Docs and other
/// files don't, so editing a README mustn't mark a build stale.
fn is_cargo_input(root: &Path, path: &Path) -> bool {
    let rel = path.strip_prefix(root).unwrap_or(path);
    path.extension().is_some_and(|e| e == "rs")
        || rel.starts_with("src")
        || matches!(rel.to_str(), Some("Cargo.toml" | "Cargo.lock" | "build.rs"))
}

fn is_library(name: &str) -> bool {
    name.ends_with(".so") || name.contains(".so.") || name.ends_with(".dylib") || name.ends_with(".dll")
}

fn describe(
    root: &Path,
    path: PathBuf,
    size: u64,
    modified: SystemTime,
    newest_src: SystemTime,
    install_dir: &Path,
) -> Option<Candidate> {
    let mut head = [0u8; 4096];
    let n = File::open(&path).and_then(|mut f| f.read(&mut head)).ok()?;
    let head = &head[..n];

    let rel_path = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
    let rel = rel_path.to_string_lossy().into_owned();
    let name = path.file_name()?.to_string_lossy().into_owned();
    let (mut kind, triple) = cargo_profile(&rel_path).unwrap_or_else(|| ("binary".into(), None));

    let arch = if head.starts_with(b"\x7fELF") {
        elf_summary(&path, head).unwrap_or_else(|| "elf".into())
    } else if head.starts_with(b"#!") {
        let line = String::from_utf8_lossy(head.split(|&b| b == b'\n').next()?);
        let interp = line[2..].split_whitespace().last().unwrap_or("?");
        let interp = interp.rsplit('/').next().unwrap_or(interp).to_string();
        if kind == "binary" {
            kind = "script".into();
        }
        format!("{interp} script")
    } else if head.starts_with(b"MZ") {
        "windows exe".into()
    } else {
        // Exec bit on something that isn't a program (common on FAT/NTFS mounts).
        return None;
    };

    let dest = install_dir.join(&name);
    let shadowed_by = shadowing_copy(&name, &dest);
    let installed = installed_state(&path, size, modified, &dest);
    let stale = rel.starts_with("target/") && newest_src > modified;
    Some(Candidate {
        path,
        name,
        rel,
        kind,
        triple,
        arch,
        size,
        modified,
        stale,
        installed,
        dest,
        shadowed_by,
        desktop: Vec::new(),
    })
}

/// `target/release/foo` → ("release", None)
/// `target/aarch64-unknown-linux-gnu/release/foo` → ("release", Some(triple))
/// `target/debug/examples/foo` → ("debug example", None)
fn cargo_profile(rel: &Path) -> Option<(String, Option<String>)> {
    let parts: Vec<String> = rel
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    if parts.len() < 3 || parts[0] != "target" {
        return None;
    }
    let mut dirs = &parts[1..parts.len() - 1];
    let example = dirs.last().is_some_and(|d| d == "examples");
    if example {
        dirs = &dirs[..dirs.len() - 1];
    }
    let (triple, profile) = match dirs {
        [profile] => (None, profile.clone()),
        [triple, profile] => (Some(triple.clone()), profile.clone()),
        _ => return None,
    };
    let kind = if example { format!("{profile} example") } else { profile };
    Some((kind, triple))
}

/// The copy your shell runs instead of `dest`, if one comes first on $PATH.
/// Only copies under $HOME count, so a project binary called `test` never
/// "conflicts" with /usr/bin/test.
fn shadowing_copy(name: &str, dest: &Path) -> Option<PathBuf> {
    crate::project::which(name).filter(|p| p != dest && p.starts_with(crate::config::home()))
}

/// Shell command that removes an installed copy: `cargo uninstall` when cargo
/// put it there (keeps cargo's records straight), plain `rm` otherwise.
pub fn removal(path: &Path, name: &str) -> Job {
    let cargo_home =
        std::env::var_os("CARGO_HOME").map(PathBuf::from).unwrap_or_else(|| crate::config::home().join(".cargo"));
    if path.parent() == Some(&cargo_home.join("bin"))
        && let Some(pkg) = cargo_owner(&cargo_home, name)
    {
        return Step::new("cargo").args(["uninstall", &pkg, "--bin", name]).into();
    }
    Builtin::RemoveFile(path.to_path_buf()).into()
}

/// Which `cargo install`ed package owns binary `name` (from .crates.toml).
fn cargo_owner(cargo_home: &Path, name: &str) -> Option<String> {
    let text = fs::read_to_string(cargo_home.join(".crates.toml")).ok()?;
    let table: toml::Table = toml::from_str(&text).ok()?;
    table.get("v1")?.as_table()?.iter().find_map(|(key, bins)| {
        let owns = bins.as_array()?.iter().any(|b| b.as_str() == Some(name));
        // Keys look like "mainframe 0.1.0 (path+file:///…)".
        owns.then(|| key.split_whitespace().next().unwrap_or(key).to_string())
    })
}

/// `cargo install` into the configured install dir: `--root ~/.local` puts
/// binaries in `~/.local/bin`. Plain `cargo install` would use ~/.cargo/bin.
/// What `p` in the install picker runs first: only the commits the branch's remote has.
/// `--ff-only` stops instead of merging when the branch has commits of its own.
pub fn pull() -> Step {
    Step::git().args(["pull", "--ff-only"])
}

/// What `p` runs before installing `c`: the pull, and a rebuild when `c` is a build the pull
/// makes stale. Err when Mainframe can't tell how to rebuild it.
pub fn pull_first(c: &Candidate, cargo: bool) -> Result<Vec<Step>, String> {
    match (c.kind.as_str(), &c.triple) {
        ("script", _) => Ok(vec![pull()]),
        ("release", None) if cargo => Ok(vec![pull(), Step::words("cargo build --release")]),
        ("debug", None) if cargo => Ok(vec![pull(), Step::words("cargo build")]),
        _ => Err(format!(
            "p can't rebuild {} ({}) after pulling — g p, build it, then i{}",
            c.name,
            c.kind,
            if cargo { ", or p on the cargo install row" } else { "" }
        )),
    }
}

pub fn cargo_install(install_dir: &Path) -> Step {
    let step = Step::words("cargo install --path .");
    match install_dir.parent().filter(|_| install_dir.file_name().is_some_and(|n| n == "bin")) {
        Some(root) => step.arg("--root").path(root),
        None => step,
    }
}

/// [`Builtin::InstallFile`]: copies `src` to `dest`, executable, next to it first and then
/// renamed over it, so a running copy of the program doesn't stand in the way. Notes when
/// `dest`'s folder isn't on PATH.
pub fn install_file(ctx: &mut crate::run::Ctx, src: &Path, dest: &Path) -> Result<(), String> {
    let pretty = crate::config::pretty_path(dest);
    let dir = dest.parent().ok_or("no folder to install into")?;
    fs::create_dir_all(dir).map_err(|e| format!("can't create {}: {e}", crate::config::pretty_path(dir)))?;
    let name = dest.file_name().unwrap_or_default().to_string_lossy();
    let tmp = dir.join(format!(".{name}.mainframe-new"));
    let copied = fs::copy(src, &tmp).and_then(|_| make_executable(&tmp)).and_then(|_| replace(&tmp, dest));
    if let Err(e) = copied {
        let _ = fs::remove_file(&tmp);
        return Err(format!("can't install {pretty}: {e}"));
    }
    ctx.success(format!("installed → {pretty}"));
    let on_path = std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d == dir));
    if !on_path {
        ctx.say(format!("note: {} is not on your PATH", crate::config::pretty_path(dir)));
    }
    Ok(())
}

#[cfg(unix)]
fn make_executable(p: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(p, fs::Permissions::from_mode(0o755))
}

#[cfg(windows)]
fn make_executable(_p: &Path) -> std::io::Result<()> {
    Ok(())
}

/// Renames `new` over `dest`. A running .exe on Windows can't be replaced, but it can be
/// renamed: it goes to `<name>.old` (removed by the next install, once it isn't running).
fn replace(new: &Path, dest: &Path) -> std::io::Result<()> {
    match fs::rename(new, dest) {
        Err(e) if cfg!(windows) && e.kind() == std::io::ErrorKind::PermissionDenied => {
            let old = dest.with_extension("old");
            let _ = fs::remove_file(&old);
            fs::rename(dest, &old)?;
            fs::rename(new, dest)
        }
        other => {
            if cfg!(windows) {
                let _ = fs::remove_file(dest.with_extension("old"));
            }
            other
        }
    }
}

fn installed_state(src: &Path, size: u64, modified: SystemTime, dest: &Path) -> Installed {
    let Ok(meta) = fs::metadata(dest) else { return Installed::No };
    if meta.len() == size && matches!((fs::read(src), fs::read(dest)), (Ok(a), Ok(b)) if a == b) {
        return Installed::Same;
    }
    match meta.modified() {
        Ok(installed) if installed > modified => Installed::Older,
        _ => Installed::Newer,
    }
}

/// "x86_64 · dynamic" from the ELF header and program headers.
fn elf_summary(path: &Path, head: &[u8]) -> Option<String> {
    let is64 = *head.get(4)? == 2;
    let le = *head.get(5)? == 1;
    let u16_at = |b: &[u8], o: usize| -> Option<u16> {
        let s: [u8; 2] = b.get(o..o + 2)?.try_into().ok()?;
        Some(if le { u16::from_le_bytes(s) } else { u16::from_be_bytes(s) })
    };
    let u32_at = |b: &[u8], o: usize| -> Option<u32> {
        let s: [u8; 4] = b.get(o..o + 4)?.try_into().ok()?;
        Some(if le { u32::from_le_bytes(s) } else { u32::from_be_bytes(s) })
    };
    let u64_at = |b: &[u8], o: usize| -> Option<u64> {
        let s: [u8; 8] = b.get(o..o + 8)?.try_into().ok()?;
        Some(if le { u64::from_le_bytes(s) } else { u64::from_be_bytes(s) })
    };

    let arch = match u16_at(head, 18)? {
        0x03 => "x86",
        0x3E => "x86_64",
        0x28 => "arm",
        0xB7 => "aarch64",
        0xF3 => {
            if is64 {
                "riscv64"
            } else {
                "riscv32"
            }
        }
        0x08 => "mips",
        0x14 => "ppc",
        0x15 => "ppc64",
        0x16 => "s390x",
        0x102 => "loongarch",
        0xF7 => "bpf",
        _ => "unknown arch",
    };

    // PT_INTERP present ⇒ dynamically linked.
    let (phoff, phentsize, phnum) = if is64 {
        (u64_at(head, 32)? as usize, u16_at(head, 54)? as usize, u16_at(head, 56)? as usize)
    } else {
        (u32_at(head, 28)? as usize, u16_at(head, 42)? as usize, u16_at(head, 44)? as usize)
    };
    let mut ph = vec![0u8; phentsize * phnum];
    let mut f = File::open(path).ok()?;
    use std::io::{Seek, SeekFrom};
    f.seek(SeekFrom::Start(phoff as u64)).ok()?;
    f.read_exact(&mut ph).ok()?;
    let dynamic = (0..phnum).any(|i| u32_at(&ph, i * phentsize) == Some(3));

    Some(format!("{arch} · {}", if dynamic { "dynamic" } else { "static" }))
}

pub fn age(t: SystemTime) -> String {
    let secs = SystemTime::now().duration_since(t).map(|d| d.as_secs()).unwrap_or(0);
    match secs {
        0..60 => "just now".into(),
        60..3600 => format!("{}m ago", secs / 60),
        3600..86400 => format!("{}h ago", secs / 3600),
        _ => format!("{}d ago", secs / 86400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(kind: &str, triple: Option<&str>) -> Candidate {
        Candidate {
            path: PathBuf::from("/p/target/release/app"),
            name: "app".into(),
            rel: "target/release/app".into(),
            kind: kind.into(),
            triple: triple.map(String::from),
            arch: String::new(),
            size: 0,
            modified: SystemTime::UNIX_EPOCH,
            stale: false,
            installed: Installed::No,
            dest: PathBuf::from("/home/u/.local/bin/app"),
            shadowed_by: None,
            desktop: Vec::new(),
        }
    }

    /// Scripts only need the pull; plain cargo builds are rebuilt after it; the rest can't be.
    #[test]
    fn pull_first_rebuilds_what_it_knows_how_to() {
        let shown = |steps: Vec<Step>| {
            steps.iter().map(|s| s.display(&crate::shell::Shell::sh())).collect::<Vec<_>>().join(" && ")
        };
        assert_eq!(shown(pull_first(&candidate("script", None), false).unwrap()), "git pull --ff-only");
        assert_eq!(
            shown(pull_first(&candidate("release", None), true).unwrap()),
            "git pull --ff-only && cargo build --release"
        );
        assert_eq!(shown(pull_first(&candidate("debug", None), true).unwrap()), "git pull --ff-only && cargo build");
        assert!(pull_first(&candidate("release", Some("aarch64-unknown-linux-gnu")), true).is_err());
        assert!(pull_first(&candidate("release example", None), true).unwrap_err().contains("cargo install row"));
        assert!(!pull_first(&candidate("binary", None), false).unwrap_err().contains("cargo install row"));
    }

    /// Into a folder that isn't there yet, then over the installed copy.
    #[test]
    fn installs_and_replaces() {
        use crate::{cmd::Cmd, run::testing::run_in, testutil::temp_dir};
        let dir = temp_dir("install-file");
        let (src, dest) = (dir.join("app-build"), dir.join("bin").join("app"));
        fs::write(&src, "one").unwrap();
        let install = Cmd::from(Builtin::InstallFile { src: src.clone(), dest: dest.clone() });
        let (ok, out) = run_in(&install, &dir);
        assert!(ok, "{out}");
        assert!(out.contains("installed → "), "{out}");
        fs::write(&src, "two").unwrap();
        assert!(run_in(&install, &dir).0);
        assert_eq!(fs::read_to_string(&dest).unwrap(), "two");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(&dest).unwrap().permissions().mode() & 0o777, 0o755);
        }
        let left: Vec<_> = fs::read_dir(dir.join("bin")).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(left, ["app"]);
        fs::remove_dir_all(&dir).unwrap();
    }
}
