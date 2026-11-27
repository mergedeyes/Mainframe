//! The shell that runs command lines the user writes: `:` commands, custom actions, runners.
//! Mainframe's own commands don't go through it (see `cmd`), so this is only about getting
//! the user's text to the shell they chose, and quoting paths for that shell.

use std::{
    env,
    path::{Path, PathBuf},
    process::Command,
};

use crate::{cmd::Step, platform};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// sh, bash, zsh, dash, Git Bash, …
    Posix,
    Fish,
    /// pwsh or Windows PowerShell.
    PowerShell,
    /// cmd.exe
    Cmd,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shell {
    pub program: PathBuf,
    pub kind: Kind,
}

impl Default for Shell {
    /// `sh` on Unix; on Windows PowerShell 7 when it's installed, else Windows PowerShell.
    fn default() -> Self {
        if cfg!(windows) {
            let program = platform::which("pwsh").unwrap_or_else(|| "powershell.exe".into());
            Self { program, kind: Kind::PowerShell }
        } else {
            Self { program: "sh".into(), kind: Kind::Posix }
        }
    }
}

impl Shell {
    /// Plain sh, for Mainframe's own scripts.
    pub fn sh() -> Self {
        Self { program: "sh".into(), kind: Kind::Posix }
    }

    /// From the `shell` setting: a name on PATH or a path, empty for the default.
    pub fn new(setting: &str) -> Self {
        let setting = setting.trim();
        if setting.is_empty() {
            return Self::default();
        }
        let kind = kind_of(Path::new(setting));
        let program = if Path::new(setting).components().count() > 1 {
            PathBuf::from(setting)
        } else if cfg!(windows) && kind == Kind::Posix {
            // `bash` on PATH is often WSL's launcher in System32; Git's is what's meant.
            platform::git_sh(setting).or_else(|| platform::which(setting)).unwrap_or_else(|| setting.into())
        } else {
            platform::which(setting).unwrap_or_else(|| setting.into())
        };
        Self { program, kind }
    }

    /// Runs `line` in this shell, exiting with the status of what ran last.
    pub fn command(&self, line: &str) -> Command {
        let mut c = Command::new(&self.program);
        match self.kind {
            Kind::Posix | Kind::Fish => {
                c.arg("-c").arg(line);
            }
            Kind::PowerShell => {
                c.args(["-NoLogo", "-NoProfile", "-NonInteractive"]);
                if cfg!(windows) {
                    // For this process only; without it `.\build.ps1` is refused on a default install.
                    c.args(["-ExecutionPolicy", "Bypass"]);
                }
                c.arg("-EncodedCommand").arg(encode_command(&format!("{PS_PRELUDE}{line}{PS_EPILOGUE}")));
            }
            Kind::Cmd => cmd_line(&mut c, line),
        }
        c
    }

    /// The shell itself, to hand the terminal to (`!`). On Unix that's your login shell
    /// (`$SHELL`) when Mainframe's is plain sh; elsewhere the configured one.
    pub fn interactive(&self) -> Step {
        if cfg!(unix) && self.program == Path::new("sh") {
            return Step::new(env::var("SHELL").unwrap_or_else(|_| "sh".into()));
        }
        let step = Step::new(self.program.to_string_lossy());
        match self.kind {
            Kind::PowerShell => step.arg("-NoLogo"),
            Kind::Posix if cfg!(windows) => step.arg("-l"),
            _ => step,
        }
    }

    /// `s` as one word for this shell: left alone when nothing in it is special.
    pub fn quote(&self, s: &str) -> String {
        match self.kind {
            Kind::Posix => posix_quote(s),
            Kind::Fish => {
                if is_plain(s, "/._-+=:,@%") {
                    s.into()
                } else {
                    format!("'{}'", s.replace('\\', r"\\").replace('\'', r"\'"))
                }
            }
            // `@` splats and `,` builds arrays in PowerShell; backslashes are just characters.
            Kind::PowerShell => {
                if is_plain(s, r"/\._-+=:") {
                    s.into()
                } else {
                    format!("'{}'", s.replace('\'', "''"))
                }
            }
            Kind::Cmd => {
                if is_plain(s, r"/\._-+=:,@") {
                    s.into()
                } else {
                    format!("\"{}\"", s.replace('"', "\"\""))
                }
            }
        }
    }

    pub fn quote_path(&self, p: &Path) -> String {
        self.quote(&p.to_string_lossy())
    }
}

fn kind_of(program: &Path) -> Kind {
    match crate::cmd::program_stem(&program.to_string_lossy()).as_str() {
        "pwsh" | "powershell" => Kind::PowerShell,
        "cmd" => Kind::Cmd,
        "fish" => Kind::Fish,
        _ => Kind::Posix,
    }
}

fn is_plain(s: &str, extra: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || extra.contains(c))
}

/// POSIX single-quote escaping, skipped when the string is obviously safe.
pub fn posix_quote(s: &str) -> String {
    if is_plain(s, "/._-+=:,@%") { s.to_string() } else { format!("'{}'", s.replace('\'', r"'\''")) }
}

#[cfg(windows)]
fn cmd_line(c: &mut Command, line: &str) {
    use std::os::windows::process::CommandExt;
    // cmd parses its own command line, not argv: /s takes everything between the outer quotes as is.
    c.args(["/d", "/s", "/c"]).raw_arg(format!("\"{line}\""));
}

#[cfg(not(windows))]
fn cmd_line(c: &mut Command, line: &str) {
    c.args(["/d", "/s", "/c", line]);
}

/// Before the command: no progress bars in the output, UTF-8 for what PowerShell itself
/// prints, colors kept although the output goes to a pipe (pwsh 7.2+).
const PS_PRELUDE: &str = "$ProgressPreference = 'SilentlyContinue'\n\
    try { [Console]::OutputEncoding = [Text.UTF8Encoding]::new($false) } catch {}\n\
    if ($PSStyle) { $PSStyle.OutputRendering = 'Ansi' }\n";

/// PowerShell's own exit code is only 0 or 1; this passes on the program's real one.
const PS_EPILOGUE: &str = "\nif (-not $?) { if ($LASTEXITCODE) { exit $LASTEXITCODE } else { exit 1 } }";

/// Base64 of the UTF-16LE text, as `-EncodedCommand` wants it.
pub fn encode_command(script: &str) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn shell(kind: Kind) -> Shell {
        Shell { program: "x".into(), kind }
    }

    #[test]
    fn kinds_come_from_the_program_name() {
        assert_eq!(kind_of(Path::new("/usr/bin/fish")), Kind::Fish);
        assert_eq!(kind_of(Path::new(r"C:\Program Files\PowerShell\7\pwsh.exe")), Kind::PowerShell);
        assert_eq!(kind_of(Path::new("powershell.exe")), Kind::PowerShell);
        assert_eq!(kind_of(Path::new("CMD.EXE")), Kind::Cmd);
        assert_eq!(kind_of(Path::new("bash")), Kind::Posix);
    }

    #[test]
    fn quoting_per_shell() {
        let s = "it's a b";
        assert_eq!(shell(Kind::Posix).quote(s), r"'it'\''s a b'");
        assert_eq!(shell(Kind::Fish).quote(s), r"'it\'s a b'");
        assert_eq!(shell(Kind::PowerShell).quote(s), "'it''s a b'");
        assert_eq!(shell(Kind::Cmd).quote("a \"b\""), r#""a ""b""""#);
        assert_eq!(shell(Kind::PowerShell).quote(r"C:\src\main.rs"), r"C:\src\main.rs");
        assert_eq!(shell(Kind::PowerShell).quote("a@b"), "'a@b'");
        assert_eq!(shell(Kind::Posix).quote("src/main.rs"), "src/main.rs");
    }

    #[test]
    fn encoded_command_is_base64_of_utf16le() {
        assert_eq!(encode_command("ab"), "YQBiAA==");
        assert_eq!(encode_command("abc"), "YQBiAGMA");
        assert_eq!(encode_command(""), "");
    }

    #[cfg(unix)]
    #[test]
    fn posix_shell_runs_the_line() {
        let out = Shell::new("sh").command("printf '%s' \"$((6*7))\"").output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), "42");
    }

    #[cfg(windows)]
    #[test]
    fn powershell_passes_exit_codes_and_quotes_through() {
        let sh = Shell::default();
        let out = sh.command("Write-Output 'it''s \"here\"'").output().unwrap();
        assert!(out.status.success());
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "it's \"here\"");
        assert_eq!(sh.command("cmd /c exit 7").status().unwrap().code(), Some(7));
    }

    #[cfg(windows)]
    #[test]
    fn cmd_shell_runs_the_line() {
        let out = Shell::new("cmd").command("echo \"a b\"& exit /b 3").output().unwrap();
        assert_eq!(out.status.code(), Some(3));
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "\"a b\"");
    }
}
