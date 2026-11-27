//! What differs between Unix and Windows: where Mainframe keeps its files, what counts as a
//! program, process trees, the shell typed commands run in, the trash and disk usage.
//! Everything else uses these instead of reaching for `std::os::unix` or `libc`.

#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix as sys;
#[cfg(unix)]
pub use unix::{ProcessTree, detach, open, today, trash, trash_available, writable};

#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as sys;
#[cfg(windows)]
pub use windows::{ProcessTree, detach, open, today, trash, trash_available, writable};

use std::{
    env,
    fs::Metadata,
    path::{Path, PathBuf},
};

/// The user's home folder: `$HOME` on Unix, the profile folder (`%USERPROFILE%`) on Windows.
pub fn home() -> PathBuf {
    env::home_dir().unwrap_or_else(|| PathBuf::from(if cfg!(windows) { r"C:\" } else { "/" }))
}

/// An XDG base directory, if it's set to something usable. Honoured on Windows too, for
/// people who keep one set of dotfiles everywhere.
fn xdg(var: &str) -> Option<PathBuf> {
    env::var_os(var).map(PathBuf::from).filter(|p| p.is_absolute()).map(|p| p.join("mainframe"))
}

/// config.toml and the per-project settings.
pub fn config_dir() -> PathBuf {
    xdg("XDG_CONFIG_HOME").unwrap_or_else(sys::default_config_dir)
}

/// Notes and todos.
pub fn data_dir() -> PathBuf {
    xdg("XDG_DATA_HOME").unwrap_or_else(sys::default_data_dir)
}

/// Command history and saved output.
pub fn state_dir() -> PathBuf {
    xdg("XDG_STATE_HOME").unwrap_or_else(sys::default_state_dir)
}

/// Things that can be thrown away, like deploy build checkouts.
pub fn cache_dir() -> PathBuf {
    xdg("XDG_CACHE_HOME").unwrap_or_else(sys::default_cache_dir)
}

/// `fs::canonicalize`, but on Windows without the `\\?\` it puts in front (`\\?\C:\Users\…`)
/// when the path works without it: everything else (git, the home folder, what users type)
/// has paths without it, and those must compare equal.
pub fn canonicalize(path: &Path) -> std::io::Result<PathBuf> {
    std::fs::canonicalize(path).map(simplify)
}

fn simplify(p: PathBuf) -> PathBuf {
    if !cfg!(windows) {
        return p;
    }
    let s = p.to_string_lossy();
    let plain = if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        let b = rest.as_bytes();
        // Only `X:\…`; other verbatim paths (volume GUIDs) have no plain form.
        if b.len() < 3 || !b[0].is_ascii_alphabetic() || b[1] != b':' || b[2] != b'\\' {
            return p;
        }
        rest.to_string()
    } else {
        return p;
    };
    // Too long, or names Windows only accepts verbatim (trailing dot or space, CON, NUL…).
    let reserved = |c: &str| {
        let stem = c.split('.').next().unwrap_or(c).to_ascii_uppercase();
        matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || (stem.len() == 4
                && (stem.starts_with("COM") || stem.starts_with("LPT"))
                && stem.as_bytes()[3].is_ascii_digit())
    };
    let ok = plain.len() < 260 && plain.split('\\').skip(1).all(|c| !c.ends_with(['.', ' ']) && !reserved(c));
    if ok { PathBuf::from(plain) } else { p }
}

/// Bytes free for this user on the file system `path` is on.
pub fn free_space(path: &Path) -> Option<u64> {
    sys::free_space(path)
}

/// Writes out what the file system holds back, so `free_space` counts files just deleted
/// (btrfs frees them in its own time). Best effort.
pub fn flush(path: &Path) {
    sys::flush(path);
}

/// A desktop notification, when the system has a way to show one. Best effort.
pub fn notify(summary: &str, body: &str) {
    sys::notify(summary, body);
}

/// Finds a program on PATH. On Windows `cargo` finds `cargo.exe`, `code` finds `code.cmd`.
pub fn which(name: &str) -> Option<PathBuf> {
    env::split_paths(&env::var_os("PATH")?).find_map(|dir| sys::program_in(&dir, name))
}

/// `sh` for Mainframe's scripts that aren't Rust yet: the system's on Unix, Git's on Windows.
pub fn posix_sh() -> Option<PathBuf> {
    if cfg!(windows) { git_sh("sh") } else { Some("sh".into()) }
}

/// `sh` or `bash` from Git for Windows, found next to `git.exe` (in `cmd\`, `bin\` or
/// `mingw64\bin\` of its install).
pub fn git_sh(name: &str) -> Option<PathBuf> {
    if !cfg!(windows) {
        return None;
    }
    let git = which("git")?;
    let root = git.ancestors().skip(2).find(|d| d.join("usr").join("bin").is_dir())?;
    [root.join("bin"), root.join("usr").join("bin")].iter().map(|d| d.join(format!("{name}.exe"))).find(|p| p.is_file())
}

/// Whether a file is something you can run: the exec bit on Unix, a PATHEXT extension
/// (`.exe`, `.cmd`, …) on Windows.
pub fn is_executable(path: &Path, meta: &Metadata) -> bool {
    meta.is_file() && sys::is_executable(path, meta)
}

/// What you'd type to run a program file found on PATH: `cargo.exe` → `cargo` on Windows.
pub fn command_name(file_name: &str) -> &str {
    sys::command_name(file_name)
}

/// What a file takes up on disk, and an identity when it has more than one hard link, so a
/// walk can count it once.
pub struct DiskUse {
    pub id: Option<(u64, u64)>,
    pub bytes: u64,
}

/// `meta` is the file's own metadata (not following symlinks).
pub fn disk_use(path: &Path, meta: &Metadata) -> DiskUse {
    sys::disk_use(path, meta)
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn verbatim_paths_lose_their_prefix_when_they_can() {
        let s = |p: &str| simplify(PathBuf::from(p)).to_string_lossy().into_owned();
        assert_eq!(s(r"\\?\C:\Users\jan\x"), r"C:\Users\jan\x");
        assert_eq!(s(r"\\?\UNC\server\share\x"), r"\\server\share\x");
        assert_eq!(s(r"\\?\C:\dir\nul.txt"), r"\\?\C:\dir\nul.txt");
        assert_eq!(s(r"\\?\C:\dir\trailing."), r"\\?\C:\dir\trailing.");
        assert_eq!(s(r"\\?\Volume{0000}\x"), r"\\?\Volume{0000}\x");
        assert!(!canonicalize(&env::temp_dir()).unwrap().to_string_lossy().starts_with(r"\\?\"));
    }
}
