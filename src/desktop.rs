//! .desktop entries that belong to a project: where they're installed, how, and
//! what installing the project's version would change.

use std::{
    env, fs,
    path::{Path, PathBuf},
};

use crate::config::home;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Not installed anywhere yet.
    New,
    /// Installed, but the content would change.
    Changed,
    UpToDate,
    /// The installed entry is a symlink to the project file itself.
    Linked,
}

#[derive(Debug, Clone)]
pub struct DesktopPlan {
    /// The `.desktop` file in the project.
    pub src: PathBuf,
    /// Where it is (or will be) installed.
    pub target: PathBuf,
    pub status: Status,
    /// We can't write the target ourselves, so it goes through sudo.
    pub system: bool,
    /// Installed target is a symlink (to something other than `src`).
    pub symlink: bool,
    pub old: Option<String>,
    pub new: String,
    /// Human-readable list of rewrites applied to the project's file.
    pub notes: Vec<String>,
}

impl DesktopPlan {
    pub fn needs_update(&self) -> bool {
        matches!(self.status, Status::New | Status::Changed)
    }
}

/// Plans for the project's desktop entries that launch `bin` (or, with `None`,
/// every entry, each targeting the binary its own `Exec=` names).
pub fn plans(root: &Path, bin: Option<&str>, install_dir: &Path) -> Vec<DesktopPlan> {
    let mut files = Vec::new();
    find_desktop_files(root, 0, &mut files);
    files
        .into_iter()
        .filter_map(|src| {
            let text = fs::read_to_string(&src).ok()?;
            let exec_bin = exec_program(&text).map(|p| basename(&p).to_string())?;
            if bin.is_some_and(|b| b != exec_bin) {
                return None;
            }
            Some(plan(root, &src, &text, &exec_bin, &install_dir.join(&exec_bin)))
        })
        .collect()
}

fn plan(root: &Path, src: &Path, text: &str, bin: &str, bin_dest: &Path) -> DesktopPlan {
    let (new, notes) = transform(root, text, bin, bin_dest);
    let file_name = src.file_name().unwrap_or_default();
    let target = installed_location(file_name.as_ref(), bin).unwrap_or_else(|| app_dirs()[0].join(file_name));

    let meta = fs::symlink_metadata(&target).ok();
    let symlink = meta.as_ref().is_some_and(|m| m.file_type().is_symlink());
    let linked = symlink && fs::canonicalize(&target).ok() == fs::canonicalize(src).ok();
    let old = meta.and_then(|_| fs::read_to_string(&target).ok());
    let status = match &old {
        _ if linked => Status::Linked,
        None => Status::New,
        Some(o) if *o == new => Status::UpToDate,
        Some(_) => Status::Changed,
    };
    DesktopPlan {
        src: src.to_path_buf(),
        system: !writable(&target),
        symlink: symlink && !linked,
        target,
        status,
        old,
        new,
        notes,
    }
}

/// The project's file as it should be installed: Exec/TryExec pointing at the
/// installed binary (when they name a path), relative icons made absolute.
fn transform(root: &Path, text: &str, bin: &str, bin_dest: &Path) -> (String, Vec<String>) {
    let mut notes = Vec::new();
    let mut in_entry = false;
    let mut out: Vec<String> = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            // [Desktop Entry] and [Desktop Action …] both launch the binary.
            in_entry = trimmed.starts_with("[Desktop ");
        }
        let Some((key, value)) = line.split_once('=').filter(|_| in_entry) else {
            out.push(line.to_string());
            continue;
        };
        let key_name = key.trim();
        let new_value = match key_name {
            "Exec" | "TryExec" => {
                let (prog, rest) = split_program(value);
                let dest = bin_dest.to_string_lossy();
                if prog.contains('/') && basename(&prog) == bin && prog != dest {
                    notes.push(format!("{key_name}: {prog} → {dest}"));
                    Some(format!("{}{rest}", quote_exec(&dest)))
                } else {
                    None
                }
            }
            "Icon" => {
                let v = value.trim();
                let candidate = root.join(v);
                (!v.is_empty() && !v.starts_with('/') && v.contains(['/', '.']) && candidate.is_file()).then(|| {
                    let abs = candidate.to_string_lossy().into_owned();
                    notes.push(format!("Icon: {v} → {abs}"));
                    abs
                })
            }
            _ => None,
        };
        out.push(match new_value {
            Some(v) => format!("{key}={v}"),
            None => line.to_string(),
        });
    }
    let mut joined = out.join("\n");
    if text.ends_with('\n') {
        joined.push('\n');
    }
    (joined, notes)
}

/// The program of the first `Exec=` line in `[Desktop Entry]`.
fn exec_program(text: &str) -> Option<String> {
    let mut in_entry = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_entry = t == "[Desktop Entry]";
        } else if in_entry
            && let Some(v) = t.strip_prefix("Exec=") {
                let (prog, _) = split_program(v);
                return (!prog.is_empty()).then_some(prog);
            }
    }
    None
}

/// Splits an Exec value into its (unquoted) program and the rest of the line.
fn split_program(value: &str) -> (String, &str) {
    let v = value.trim_start();
    if let Some(stripped) = v.strip_prefix('"') {
        match stripped.find('"') {
            Some(end) => (stripped[..end].to_string(), &stripped[end + 1..]),
            None => (stripped.to_string(), ""),
        }
    } else {
        let end = v.find(char::is_whitespace).unwrap_or(v.len());
        (v[..end].to_string(), &v[end..])
    }
}

fn quote_exec(path: &str) -> String {
    if path.contains(char::is_whitespace) { format!("\"{path}\"") } else { path.to_string() }
}

fn basename(p: &str) -> &str {
    p.rsplit('/').next().unwrap_or(p)
}

/// Whether we may create or replace `path`: the file itself if it exists,
/// otherwise the nearest existing parent directory.
fn writable(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let existing = path.ancestors().find(|p| p.exists()).unwrap_or(Path::new("/"));
    // Replacing a file also works through a writable directory, but sudo-owned
    // files in a user dir are rare; checking the file itself keeps it simple.
    let Ok(c) = std::ffi::CString::new(existing.as_os_str().as_bytes()) else { return false };
    unsafe { libc::access(c.as_ptr(), libc::W_OK) == 0 }
}

/// User applications dir first, then the system ones from $XDG_DATA_DIRS.
pub fn app_dirs() -> Vec<PathBuf> {
    let data_home = env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".local/share"));
    let data_dirs = env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    std::iter::once(data_home)
        .chain(data_dirs.split(':').filter(|d| !d.is_empty()).map(PathBuf::from))
        .map(|d| d.join("applications"))
        .collect()
}

/// An installed entry with the same file name, else one whose Exec launches `bin`.
fn installed_location(file_name: &Path, bin: &str) -> Option<PathBuf> {
    let dirs = app_dirs();
    if let Some(p) = dirs.iter().map(|d| d.join(file_name)).find(|p| fs::symlink_metadata(p).is_ok()) {
        return Some(p);
    }
    dirs.iter().find_map(|d| {
        fs::read_dir(d).ok()?.flatten().map(|de| de.path()).find(|p| {
            p.extension().is_some_and(|e| e == "desktop")
                && fs::read_to_string(p)
                    .ok()
                    .and_then(|t| exec_program(&t))
                    .is_some_and(|prog| basename(&prog) == bin)
        })
    })
}

fn find_desktop_files(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for de in rd.flatten() {
        let name = de.file_name();
        let name = name.to_string_lossy();
        let path = de.path();
        let Ok(ft) = de.file_type() else { continue };
        if ft.is_dir() {
            let skip = name.starts_with('.') || matches!(name.as_ref(), "target" | "node_modules" | "build" | "dist");
            if !skip && depth < 4 {
                find_desktop_files(&path, depth + 1, out);
            }
        } else if name.ends_with(".desktop") {
            out.push(path);
        }
    }
    out.sort();
}

/// Writes the entry (user locations only; system ones go through sudo) and
/// refreshes the desktop database if the tool is around.
pub fn apply_user(plan: &DesktopPlan) -> std::io::Result<()> {
    if let Some(dir) = plan.target.parent() {
        fs::create_dir_all(dir)?;
    }
    if plan.symlink {
        // Replace the link itself rather than writing through it.
        fs::remove_file(&plan.target)?;
    }
    fs::write(&plan.target, &plan.new)
}

/// Shell command that installs a system entry with sudo, from a temp copy.
pub fn system_cmd(plan: &DesktopPlan) -> std::io::Result<String> {
    let q = crate::project::quote;
    let name = plan.target.file_name().unwrap_or_default().to_string_lossy();
    let tmp = env::temp_dir().join(format!("mainframe-{name}"));
    fs::write(&tmp, &plan.new)?;
    let dir = plan.target.parent().unwrap_or(Path::new("/usr/share/applications"));
    Ok(format!(
        "sudo install -Dm644 -- {} {} && (command -v update-desktop-database >/dev/null && sudo update-desktop-database -q {} || true)",
        q(&tmp),
        q(&plan.target),
        q(dir)
    ))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Same,
    Removed,
    Added,
}

/// Line diff (LCS). Desktop files are tiny, so the quadratic table is fine.
pub fn diff(old: &str, new: &str) -> Vec<(Change, String)> {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    let (n, m) = (a.len(), b.len());
    let mut lcs = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if a[i] == b[j] { lcs[i + 1][j + 1] + 1 } else { lcs[i + 1][j].max(lcs[i][j + 1]) };
        }
    }
    let (mut i, mut j, mut out) = (0, 0, Vec::new());
    while i < n || j < m {
        if i < n && j < m && a[i] == b[j] {
            out.push((Change::Same, a[i].to_string()));
            i += 1;
            j += 1;
        } else if i < n && (j == m || lcs[i + 1][j] >= lcs[i][j + 1]) {
            out.push((Change::Removed, a[i].to_string()));
            i += 1;
        } else {
            out.push((Change::Added, b[j].to_string()));
            j += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_path_exec_but_not_bare_names() {
        let root = Path::new("/nonexistent");
        let dest = Path::new("/home/u/.local/bin/app");
        let text = "[Desktop Entry]\nName=App\nExec=/p/target/release/app --flag %U\nTryExec=app\n";
        let (new, notes) = transform(root, text, "app", dest);
        assert!(new.contains("Exec=/home/u/.local/bin/app --flag %U"));
        assert!(new.contains("TryExec=app\n"));
        assert_eq!(notes.len(), 1);
    }

    #[test]
    fn rewrites_actions_but_not_foreign_groups() {
        let text = "[Desktop Entry]\nExec=app\n[Desktop Action new]\nExec=/x/app --new\n[X-Other]\nExec=/x/app\n";
        let (new, _) = transform(Path::new("/"), text, "app", Path::new("/b/app"));
        assert!(new.contains("Exec=/b/app --new"));
        assert!(new.ends_with("[X-Other]\nExec=/x/app\n"));
    }

    #[test]
    fn diff_marks_changes() {
        let d = diff("a\nb\nc\n", "a\nB\nc\n");
        let kinds: Vec<Change> = d.iter().map(|(c, _)| *c).collect();
        assert_eq!(kinds, [Change::Same, Change::Removed, Change::Added, Change::Same]);
    }
}
