//! Linux and the other Unixes.

use std::{
    ffi::CString,
    fs::Metadata,
    io,
    os::unix::{
        ffi::OsStrExt,
        fs::{MetadataExt, PermissionsExt},
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
};

use super::{DiskUse, home, which};

pub(super) fn default_config_dir() -> PathBuf {
    home().join(".config/mainframe")
}

pub(super) fn default_data_dir() -> PathBuf {
    home().join(".local/share/mainframe")
}

pub(super) fn default_state_dir() -> PathBuf {
    home().join(".local/state/mainframe")
}

pub(super) fn default_cache_dir() -> PathBuf {
    home().join(".cache/mainframe")
}

pub(super) fn program_in(dir: &Path, name: &str) -> Option<PathBuf> {
    let p = dir.join(name);
    p.is_file().then_some(p)
}

pub(super) fn is_executable(_path: &Path, meta: &Metadata) -> bool {
    meta.permissions().mode() & 0o111 != 0
}

pub(super) fn command_name(file_name: &str) -> &str {
    file_name
}

/// Whether we may create or replace `path`: the file itself if it exists, otherwise the
/// nearest existing parent directory.
pub fn writable(path: &Path) -> bool {
    let existing = path.ancestors().find(|p| p.exists()).unwrap_or(Path::new("/"));
    let Ok(c) = CString::new(existing.as_os_str().as_bytes()) else { return false };
    // SAFETY: c is a valid C string.
    unsafe { libc::access(c.as_ptr(), libc::W_OK) == 0 }
}

/// Today in local time, as `YYYY-MM-DD`.
pub fn today() -> String {
    // SAFETY: `time` and `localtime_r` only write to the `tm` handed to them.
    unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&now, &mut tm).is_null() {
            return "1970-01-01".into();
        }
        format!("{:04}-{:02}-{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday)
    }
}

/// A background command and everything it starts, stopped together.
pub struct ProcessTree {
    pgid: libc::pid_t,
}

impl ProcessTree {
    /// New session: no controlling terminal, so anything that wants a password (sudo, ssh)
    /// fails fast instead of fighting the TUI for the tty, and the process group lets
    /// `terminate` take grandchildren (e.g. the binary under `cargo run`) too.
    pub fn prepare(command: &mut Command) {
        // SAFETY: setsid is async-signal-safe.
        unsafe {
            command.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }

    pub fn attach(child: &Child) -> Self {
        Self { pgid: child.id() as libc::pid_t }
    }

    /// Asks the whole group to stop (SIGTERM), without waiting for it.
    pub fn terminate(&self) {
        // SAFETY: plain syscall on a pid we started.
        unsafe {
            libc::killpg(self.pgid, libc::SIGTERM);
        }
    }
}

/// Keeps a program (an editor, a notification) running on its own, apart from Mainframe's
/// process group.
pub fn detach(command: &mut Command) {
    command.process_group(0);
}

/// A command line typed by the user (or written in the config), run by `sh`.
pub fn shell(cmd: &str) -> Command {
    let mut c = Command::new("sh");
    c.arg("-c").arg(cmd);
    c
}

/// The command that opens the user's own shell in the terminal: `$SHELL`.
pub fn interactive_shell() -> String {
    crate::project::quote_str(&std::env::var("SHELL").unwrap_or_else(|_| "sh".into()))
}

/// Opens a file in its default application.
pub fn open(path: &Path) -> io::Result<()> {
    let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    if which(opener).is_none() {
        return Err(io::Error::new(io::ErrorKind::NotFound, format!("{opener} isn't installed")));
    }
    let mut c = Command::new(opener);
    c.arg(path).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    detach(&mut c);
    let mut child = c.spawn()?;
    // Reap it in the background so it doesn't linger as a zombie.
    drop(thread::spawn(move || child.wait()));
    Ok(())
}

fn trash_program() -> Option<(&'static str, &'static [&'static str])> {
    if which("gio").is_some() {
        Some(("gio", &["trash", "--"]))
    } else if which("trash-put").is_some() {
        Some(("trash-put", &["--"]))
    } else {
        None
    }
}

/// Whether `trash` moves things to the trash rather than failing.
pub fn trash_available() -> bool {
    trash_program().is_some()
}

pub fn trash(path: &Path) -> io::Result<()> {
    let Some((program, args)) = trash_program() else {
        return Err(io::Error::new(io::ErrorKind::NotFound, "no trash: install gio or trash-cli"));
    };
    let out = Command::new(program).args(args).arg(path).stdin(Stdio::null()).output()?;
    if out.status.success() {
        return Ok(());
    }
    let msg = String::from_utf8_lossy(&out.stderr).trim().to_string();
    Err(io::Error::other(if msg.is_empty() { format!("{program} failed") } else { msg }))
}

pub(super) fn disk_use(_path: &Path, meta: &Metadata) -> DiskUse {
    let id = (!meta.is_dir() && meta.nlink() > 1).then(|| (meta.dev(), meta.ino()));
    DiskUse { id, bytes: meta.blocks() * 512 }
}
