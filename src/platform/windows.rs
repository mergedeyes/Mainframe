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

/// PowerShell 7 (`pwsh`) when it's installed, else the Windows PowerShell every Windows has.
fn powershell() -> &'static Path {
    static EXE: OnceLock<PathBuf> = OnceLock::new();
    EXE.get_or_init(|| which("pwsh").unwrap_or_else(|| "powershell.exe".into()))
}

/// Set before the command: no progress bars in the output, UTF-8 for what PowerShell itself
/// prints, colors kept although the output goes to a pipe (pwsh 7.2+).
const PRELUDE: &str = "$ProgressPreference = 'SilentlyContinue'\n\
    try { [Console]::OutputEncoding = [Text.UTF8Encoding]::new($false) } catch {}\n\
    if ($PSStyle) { $PSStyle.OutputRendering = 'Ansi' }\n";

/// PowerShell's own exit code is only 0 or 1; this passes on the program's real one.
const EPILOGUE: &str = "\nif (-not $?) { if ($LASTEXITCODE) { exit $LASTEXITCODE } else { exit 1 } }";

/// A command line typed by the user (or written in the config), run by PowerShell. It goes
/// in as `-EncodedCommand`, so no quote in it can be mangled on the way.
pub fn shell(cmd: &str) -> Command {
    let script = format!("{PRELUDE}{cmd}{EPILOGUE}");
    let mut c = Command::new(powershell());
    // Bypass applies to this process only; without it `.\build.ps1` is refused on a
    // default Windows install.
    c.args(["-NoLogo", "-NoProfile", "-ExecutionPolicy", "Bypass", "-EncodedCommand"]).arg(encode_command(&script));
    c
}

/// Base64 of the UTF-16LE text, as `-EncodedCommand` wants it.
fn encode_command(script: &str) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, b)| n | u32::from(*b) << (16 - 8 * i));
        for i in 0..4 {
            out.push(if i <= chunk.len() { ABC[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' });
        }
    }
    out
}

/// The command that opens a shell in the terminal: PowerShell, as `shell` runs. Started by
/// name, so it's the same one the user gets from the Start menu.
pub fn interactive_shell() -> String {
    let name = if which("pwsh").is_some() { "pwsh" } else { "powershell" };
    format!("{name} -NoLogo")
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

    #[test]
    fn encoded_command_is_base64_of_utf16le() {
        // "ab" → 61 00 62 00
        assert_eq!(encode_command("ab"), "YQBiAA==");
        assert_eq!(encode_command("abc"), "YQBiAGMA");
        assert_eq!(encode_command(""), "");
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

    #[test]
    fn shell_passes_exit_codes_and_quotes_through() {
        let out = shell("Write-Output 'it''s \"here\"'").output().unwrap();
        assert!(out.status.success());
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "it's \"here\"");
        let status = shell("cmd /c exit 7").status().unwrap();
        assert_eq!(status.code(), Some(7));
    }
}
