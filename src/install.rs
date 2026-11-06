//! Finds installable executables in a project and describes what they were built for.

use std::{
    fs::{self, File},
    io::Read,
    os::unix::fs::PermissionsExt,
    path::{Component, Path, PathBuf},
    time::SystemTime,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Installed {
    No,
    Same,
    Differs,
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
        let exec = meta.permissions().mode() & 0o111 != 0 || name.ends_with(".exe");
        if exec && !is_library(&name) {
            found.push((path, meta.len(), modified));
        } else if !in_target && modified > *newest_src {
            *newest_src = modified;
        }
    }
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

    let installed = installed_state(&path, size, &install_dir.join(&name));
    let stale = rel.starts_with("target/") && newest_src > modified;
    Some(Candidate { path, name, rel, kind, triple, arch, size, modified, stale, installed })
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

fn installed_state(src: &Path, size: u64, dest: &Path) -> Installed {
    let Ok(meta) = fs::metadata(dest) else { return Installed::No };
    if meta.len() != size {
        return Installed::Differs;
    }
    match (fs::read(src), fs::read(dest)) {
        (Ok(a), Ok(b)) if a == b => Installed::Same,
        _ => Installed::Differs,
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
        0xF3 => if is64 { "riscv64" } else { "riscv32" },
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
