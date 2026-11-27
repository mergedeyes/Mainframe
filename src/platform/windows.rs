//! Windows.

use std::{
    env,
    ffi::{OsStr, OsString, c_void},
    fs::{self, Metadata, OpenOptions},
    io, iter,
    os::windows::{ffi::OsStrExt, fs::OpenOptionsExt, io::AsRawHandle, process::CommandExt},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    ptr,
    sync::OnceLock,
};

use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE, SYSTEMTIME},
    Globalization::{CP_OEMCP, MultiByteToWideChar},
    Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_STANDARD_INFO, FileStandardInfo,
        GetDiskFreeSpaceExW, GetFileInformationByHandle, GetFileInformationByHandleEx,
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

use super::{DiskUse, home, which};

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

/// Toast notifications need an app identity Mainframe doesn't register yet; the terminal
/// bell still rings.
pub(super) fn notify(_summary: &str, _body: &str) {}

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

/// Visual Studio's C++ build environment (what a Developer PowerShell has): the variables
/// `vcvarsall.bat` sets or changes, worked out once. Empty without Visual Studio, or when
/// `cl` is on PATH already (Mainframe was started from a developer prompt).
pub fn dev_env() -> &'static [(OsString, OsString)] {
    static ENV: OnceLock<Vec<(OsString, OsString)>> = OnceLock::new();
    ENV.get_or_init(|| if which("cl").is_some() { Vec::new() } else { load_dev_env().unwrap_or_default() })
}

/// Whether MSVC can be used: on PATH, or through [`dev_env`].
pub fn msvc_available() -> bool {
    which("cl").is_some() || vs_install().is_some()
}

/// The newest Visual Studio (or Build Tools) with the C++ tools for this machine, by vswhere.
fn vs_install() -> Option<&'static Path> {
    static VS: OnceLock<Option<PathBuf>> = OnceLock::new();
    VS.get_or_init(|| {
        let vswhere =
            PathBuf::from(env::var_os("ProgramFiles(x86)")?).join(r"Microsoft Visual Studio\Installer\vswhere.exe");
        let tools = if cfg!(target_arch = "aarch64") {
            "Microsoft.VisualStudio.Component.VC.Tools.ARM64"
        } else {
            "Microsoft.VisualStudio.Component.VC.Tools.x86.x64"
        };
        let mut c = Command::new(vswhere);
        c.args(["-latest", "-products", "*", "-requires", tools, "-property", "installationPath", "-utf8"]);
        let out = c.creation_flags(CREATE_NO_WINDOW).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
        let path = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
        path.join(VCVARS).is_file().then_some(path)
    })
    .as_deref()
}

const VCVARS: &str = r"VC\Auxiliary\Build\vcvarsall.bat";

fn load_dev_env() -> Option<Vec<(OsString, OsString)>> {
    let bat = vs_install()?.join(VCVARS);
    let arch = if cfg!(target_arch = "aarch64") { "arm64" } else { "x64" };
    let mut c = Command::new(env::var_os("ComSpec").unwrap_or_else(|| "cmd.exe".into()));
    // cmd reads its own command line: /s keeps everything between the outer quotes as is.
    c.args(["/d", "/s", "/c"]).raw_arg(format!("\"\"{}\" {arch} >nul && set\"", bat.display()));
    let out = c.creation_flags(CREATE_NO_WINDOW).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = super::decode(&out.stdout);
    let vars = text
        .lines()
        .filter_map(|l| l.split_once('='))
        // `Platform=x64` makes `dotnet build` look for x64 builds of AnyCPU projects.
        .filter(|(k, _)| !k.eq_ignore_ascii_case("Platform") && !k.is_empty())
        .filter(|(k, v)| env::var_os(k).as_deref() != Some(OsStr::new(v)))
        .map(|(k, v)| (OsString::from(k), OsString::from(v)))
        .collect();
    Some(vars)
}

pub(super) fn decode_legacy(bytes: &[u8]) -> String {
    let Ok(len) = i32::try_from(bytes.len()) else { return String::from_utf8_lossy(bytes).into_owned() };
    // SAFETY: the first call only measures; the second writes at most `n` u16s into `wide`.
    unsafe {
        let n = MultiByteToWideChar(CP_OEMCP, 0, bytes.as_ptr(), len, ptr::null_mut(), 0);
        if n <= 0 {
            return String::from_utf8_lossy(bytes).into_owned();
        }
        let mut wide = vec![0u16; n as usize];
        let n = MultiByteToWideChar(CP_OEMCP, 0, bytes.as_ptr(), len, wide.as_mut_ptr(), n);
        String::from_utf16_lossy(&wide[..n.max(0) as usize])
    }
}

pub(super) fn free_space(path: &Path) -> Option<u64> {
    let dir = wide(path.as_os_str());
    let mut free = 0u64;
    // SAFETY: dir is NUL-terminated; the other outputs may be null.
    let ok = unsafe { GetDiskFreeSpaceExW(dir.as_ptr(), &mut free, ptr::null_mut(), ptr::null_mut()) };
    (ok != 0).then_some(free)
}

/// NTFS counts a deleted file's room as free right away.
pub(super) fn flush(_path: &Path) {}

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

    #[test]
    fn oem_bytes_are_decoded() {
        // ü in code page 850 and 437 both.
        assert_eq!(super::super::decode(b"Verk\x81rzung"), "Verkürzung");
        assert_eq!(super::super::decode("Verkürzung".as_bytes()), "Verkürzung");
    }

    #[test]
    fn visual_studio_environment() {
        if vs_install().is_none() {
            return;
        }
        let env = dev_env();
        assert!(env.iter().any(|(k, _)| k.eq_ignore_ascii_case("INCLUDE")), "{env:?}");
        assert!(!env.iter().any(|(k, _)| k.eq_ignore_ascii_case("Platform")));
    }

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
