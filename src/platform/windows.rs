//! Windows.

use std::{
    env,
    ffi::{OsStr, c_void},
    fs::{self, Metadata, OpenOptions},
    io, iter,
    os::windows::{ffi::OsStrExt, fs::OpenOptionsExt, io::AsRawHandle, process::CommandExt},
    path::{Path, PathBuf},
    process::{Child, Command},
    ptr,
    sync::OnceLock,
};

use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE, SYSTEMTIME},
    Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_STANDARD_INFO, FileStandardInfo,
        GetFileInformationByHandle, GetFileInformationByHandleEx,
    },
    System::{
        JobObjects::{AssignProcessToJobObject, CreateJobObjectW, TerminateJobObject},
        SystemInformation::GetLocalTime,
        Threading::{CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW},
    },
    UI::{
        Shell::{
            FO_DELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, SHFILEOPSTRUCTW, SHFileOperationW,
            ShellExecuteW,
        },
        WindowsAndMessaging::SW_SHOWNORMAL,
    },
};

use super::{DiskUse, home};

/// `%APPDATA%` (roams with the profile) or `%LOCALAPPDATA%` (stays on this machine).
fn known(var: &str, fallback: &str) -> PathBuf {
    env::var_os(var).map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| home().join(fallback))
}

pub(super) fn default_config_dir() -> PathBuf {
    known("APPDATA", r"AppData\Roaming").join("mainframe")
}

/// Notes sit next to the config (in `notes\`), so they roam along with it.
pub(super) fn default_data_dir() -> PathBuf {
    default_config_dir()
}

pub(super) fn default_state_dir() -> PathBuf {
    known("LOCALAPPDATA", r"AppData\Local").join(r"mainframe\state")
}

pub(super) fn default_cache_dir() -> PathBuf {
    known("LOCALAPPDATA", r"AppData\Local").join(r"mainframe\cache")
}

/// `%PATHEXT%`, lowercased: `.com`, `.exe`, `.bat`, `.cmd`, …
fn pathext() -> &'static [String] {
    static EXTS: OnceLock<Vec<String>> = OnceLock::new();
    EXTS.get_or_init(|| {
        let raw = env::var("PATHEXT").unwrap_or_default();
        let raw = if raw.trim().is_empty() { ".COM;.EXE;.BAT;.CMD".into() } else { raw };
        raw.split(';').map(|e| e.trim().to_ascii_lowercase()).filter(|e| e.starts_with('.')).collect()
    })
}

fn has_program_ext(name: &str) -> bool {
    name.rfind('.').is_some_and(|i| pathext().iter().any(|e| e.eq_ignore_ascii_case(&name[i..])))
}

pub(super) fn program_in(dir: &Path, name: &str) -> Option<PathBuf> {
    if has_program_ext(name) {
        let p = dir.join(name);
        return p.is_file().then_some(p);
    }
    pathext().iter().map(|e| dir.join(format!("{name}{e}"))).find(|p| p.is_file())
}

pub(super) fn is_executable(path: &Path, _meta: &Metadata) -> bool {
    path.file_name().is_some_and(|n| has_program_ext(&n.to_string_lossy()))
}

pub(super) fn command_name(file_name: &str) -> &str {
    match file_name.rfind('.') {
        Some(i) if has_program_ext(file_name) => &file_name[..i],
        _ => file_name,
    }
}

/// Whether we may create or replace `path`. Only the read-only attribute is checked: ACLs
/// would need the full access check, and nothing on Windows asks for more than this yet.
pub fn writable(path: &Path) -> bool {
    let existing = path.ancestors().find(|p| p.exists()).unwrap_or(path);
    fs::metadata(existing).is_ok_and(|m| m.is_dir() || !m.permissions().readonly())
}

/// Today in local time, as `YYYY-MM-DD`.
pub fn today() -> String {
    // SAFETY: GetLocalTime only writes the SYSTEMTIME handed to it.
    let t = unsafe {
        let mut t: SYSTEMTIME = std::mem::zeroed();
        GetLocalTime(&mut t);
        t
    };
    format!("{:04}-{:02}-{:02}", t.wYear, t.wMonth, t.wDay)
}

/// No console window for the command or anything it starts; its own process group, so
/// Ctrl-C in Mainframe's terminal doesn't reach it. The hidden console is shared by the
/// whole tree, so `cargo` → `rustc` doesn't flash a window either.
const BACKGROUND: u32 = CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW;

/// A background command and everything it starts, held in a job object so that stopping
/// it takes the binary under `cargo run` along too.
pub struct ProcessTree {
    job: HANDLE,
}

// SAFETY: a job handle is a kernel object handle, usable from any thread.
unsafe impl Send for ProcessTree {}

impl ProcessTree {
    pub fn prepare(command: &mut Command) {
        command.creation_flags(BACKGROUND);
    }

    /// Processes started before the child joins the job aren't in it. The shell takes far
    /// longer to start than this takes, so in practice that's nothing.
    pub fn attach(child: &Child) -> Self {
        // SAFETY: plain Win32 calls on a handle we own and the child's process handle.
        unsafe {
            let job = CreateJobObjectW(ptr::null(), ptr::null());
            if !job.is_null() && AssignProcessToJobObject(job, child.as_raw_handle() as HANDLE) == 0 {
                CloseHandle(job);
                return Self { job: ptr::null_mut() };
            }
            Self { job }
        }
    }

    /// Ends every process in the tree. Windows has no polite SIGTERM for console programs
    /// without a console of their own, so this is the hard stop.
    pub fn terminate(&self) {
        if !self.job.is_null() {
            // SAFETY: job is a live job handle.
            unsafe { TerminateJobObject(self.job, 1) };
        }
    }

    pub fn kill(&self) {
        self.terminate();
    }
}

impl Drop for ProcessTree {
    fn drop(&mut self) {
        if !self.job.is_null() {
            // SAFETY: we own the handle. Closing it doesn't end the processes in the job.
            unsafe { CloseHandle(self.job) };
        }
    }
}

/// Keeps a program (an editor, a notification) running on its own, without a console
/// window popping up for it.
pub fn detach(command: &mut Command) {
    command.creation_flags(BACKGROUND);
}

fn wide(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(iter::once(0)).collect()
}

/// Opens a file in its default application, like double-clicking it in Explorer.
pub fn open(path: &Path) -> io::Result<()> {
    let file = wide(path.as_os_str());
    let verb = wide(OsStr::new("open"));
    // SAFETY: both strings are NUL-terminated and outlive the call.
    let result = unsafe {
        ShellExecuteW(ptr::null_mut(), verb.as_ptr(), file.as_ptr(), ptr::null(), ptr::null(), SW_SHOWNORMAL)
    };
    // Anything above 32 is success; the rest are error codes.
    if result as isize > 32 {
        Ok(())
    } else {
        Err(io::Error::other(format!("couldn't open it (error {})", result as isize)))
    }
}

/// The Recycle Bin is always there.
pub fn trash_available() -> bool {
    true
}

/// Moves a file or folder to the Recycle Bin. On a drive without one (network shares) the
/// shell deletes it for good: there's no asking first without its dialog.
pub fn trash(path: &Path) -> io::Result<()> {
    let path = std::path::absolute(path)?;
    // pFrom is a list of NUL-terminated names ending in an extra NUL.
    let mut from = wide(path.as_os_str());
    from.push(0);
    let mut op = SHFILEOPSTRUCTW {
        hwnd: ptr::null_mut(),
        wFunc: FO_DELETE,
        pFrom: from.as_ptr(),
        pTo: ptr::null(),
        fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT | FOF_NOERRORUI) as u16,
        fAnyOperationsAborted: 0,
        hNameMappings: ptr::null_mut(),
        lpszProgressTitle: ptr::null(),
    };
    // SAFETY: op is fully initialized and `from` outlives the call.
    let code = unsafe { SHFileOperationW(&mut op) };
    if code == 0 && op.fAnyOperationsAborted == 0 {
        Ok(())
    } else {
        Err(io::Error::other(format!("couldn't move it to the Recycle Bin (error {code:#x})")))
    }
}

/// Allocated size (what Explorer calls "size on disk": clusters, NTFS compression, files
/// small enough to live in the MFT taking none), and the file's id when it's hard-linked,
/// as pnpm's store does.
pub(super) fn disk_use(path: &Path, meta: &Metadata) -> DiskUse {
    let fallback = DiskUse { id: None, bytes: if meta.is_dir() { 0 } else { meta.len() } };
    if meta.is_dir() {
        return fallback;
    }
    let Ok(file) = OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
    else {
        return fallback;
    };
    let handle = file.as_raw_handle() as HANDLE;
    // SAFETY: handle is open for the duration; both structs are plain data sized as passed.
    unsafe {
        let mut info: FILE_STANDARD_INFO = std::mem::zeroed();
        let size = std::mem::size_of::<FILE_STANDARD_INFO>() as u32;
        if GetFileInformationByHandleEx(handle, FileStandardInfo, (&raw mut info).cast::<c_void>(), size) == 0 {
            return fallback;
        }
        let mut id = None;
        if info.NumberOfLinks > 1 {
            let mut by: BY_HANDLE_FILE_INFORMATION = std::mem::zeroed();
            if GetFileInformationByHandle(handle, &mut by) != 0 {
                let index = u64::from(by.nFileIndexHigh) << 32 | u64::from(by.nFileIndexLow);
                id = Some((u64::from(by.dwVolumeSerialNumber), index));
            }
        }
        DiskUse { id, bytes: info.AllocationSize.max(0) as u64 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::which;

    #[test]
    fn program_names_drop_their_extension() {
        assert_eq!(command_name("cargo.exe"), "cargo");
        assert_eq!(command_name("code.CMD"), "code");
        assert_eq!(command_name("notes.txt"), "notes.txt");
    }

    #[test]
    fn which_finds_programs_without_their_extension() {
        assert!(which("cmd").is_some_and(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("exe"))));
    }
}
