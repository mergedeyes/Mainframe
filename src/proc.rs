//! Asking a program for an answer: no terminal, stderr dropped, only a clean exit counts.

use std::process::{Command, Stdio};

/// stdout of `cmd`, if it ran and exited 0.
pub fn output(cmd: &mut Command) -> Option<Vec<u8>> {
    let out = cmd.stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    out.status.success().then_some(out.stdout)
}

/// [`output`] as text, trimmed.
pub fn text(cmd: &mut Command) -> Option<String> {
    output(cmd).map(|o| String::from_utf8_lossy(&o).trim().to_string())
}
