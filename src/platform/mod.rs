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
