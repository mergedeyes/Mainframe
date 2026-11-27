//! Finds `file:line:col` references in task output so they can be jumped to.

use std::path::{Path, PathBuf};

use crate::cmd::Step;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loc {
    pub path: PathBuf,
    pub line: u32,
    pub col: Option<u32>,
}

/// The first location in `text` that names an existing file. Relative paths are tried
/// against `cwd` and a few of its parents (cargo prints paths relative to the workspace).
pub fn find(text: &str, cwd: &Path) -> Option<Loc> {
    find_with(text, |p| {
        let p = Path::new(p);
        if p.is_absolute() {
            return p.is_file().then(|| p.to_path_buf());
        }
        cwd.ancestors().take(4).map(|d| d.join(p)).find(|f| f.is_file())
    })
}

fn find_with(text: &str, resolve: impl Fn(&str) -> Option<PathBuf>) -> Option<Loc> {
    // Python: `File "app.py", line 3, in <module>`
    if let Some(rest) = text.trim_start().strip_prefix("File \"")
        && let Some((file, rest)) = rest.split_once("\", line ")
    {
        let line = rest.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()?;
        return Some(Loc { path: resolve(file)?, line, col: None });
    }
    // Everything else: a `path:line[:col]` token (rustc, gcc, go, tsc, pytest, panics, grep -n…).
    text.split_whitespace().find_map(|tok| {
        let tok = tok.trim_matches(|c| matches!(c, '(' | ')' | '[' | ']' | '\'' | '"' | ',' | '`'));
        let tok = tok.trim_end_matches(':');
        if tok.contains("://") {
            return None;
        }
        let mut parts = tok.split(':');
        let file = parts.next()?;
        let line: u32 = parts.next()?.parse().ok()?;
        let col = parts.next().and_then(|c| c.parse().ok());
        // Requiring a dot or slash skips timestamps like 12:30:45.
        if parts.next().is_some() || line == 0 || !(file.contains('.') || file.contains('/')) {
            return None;
        }
        Some(Loc { path: resolve(file)?, line, col })
    })
}

/// A command that opens `loc` in `editor` at the right line, for the editors that support it.
pub fn editor_cmd(editor: &str, loc: &Loc) -> Step {
    let step = Step::from_setting(editor).unwrap_or_else(|| Step::new("nano"));
    let file = loc.path.to_string_lossy();
    let (l, c) = (loc.line, loc.col.unwrap_or(1));
    match crate::cmd::program_stem(&step.program).as_str() {
        "nano" => step.arg(format!("+{l},{c}")).arg(file),
        "micro" => step.arg(format!("+{l}:{c}")).arg(file),
        "hx" | "helix" => step.arg(format!("{file}:{l}:{c}")),
        "code" | "codium" | "code-insiders" | "cursor" => step.arg("-g").arg(format!("{file}:{l}:{c}")),
        "notepad++" => step.arg(format!("-n{l}")).arg(format!("-c{c}")).arg(file),
        // Windows' own editors take no line.
        "notepad" | "edit" => step.arg(file),
        // vi, vim, nvim, emacs, kak, joe, ne, mcedit … all understand +LINE.
        _ => step.arg(format!("+{l}")).arg(file),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find(text: &str) -> Option<(String, u32, Option<u32>)> {
        find_with(text, |p| Some(PathBuf::from(p))).map(|l| (l.path.to_string_lossy().into_owned(), l.line, l.col))
    }

    #[test]
    fn compilers() {
        assert_eq!(find("  --> src/main.rs:12:5"), Some(("src/main.rs".into(), 12, Some(5))));
        assert_eq!(find("main.c:3:10: error: expected ';'"), Some(("main.c".into(), 3, Some(10))));
        assert_eq!(find("./cmd/x.go:7: undefined: foo"), Some(("./cmd/x.go".into(), 7, None)));
        assert_eq!(find("src/a.ts(4,2): error"), None);
        assert_eq!(find("thread 'main' panicked at src/lib.rs:5:9:"), Some(("src/lib.rs".into(), 5, Some(9))));
    }

    #[test]
    fn python() {
        let l = find(r#"  File "app/run.py", line 42, in main"#);
        assert_eq!(l, Some(("app/run.py".into(), 42, None)));
    }

    #[test]
    fn not_locations() {
        assert_eq!(find("started at 12:30:45"), None);
        assert_eq!(find("see https://example.com:8080/x"), None);
        assert_eq!(find("Compiling foo v0.1.0"), None);
    }

    #[test]
    fn editors() {
        let loc = Loc { path: "/a b.rs".into(), line: 3, col: Some(7) };
        let args = |editor: &str| {
            let s = editor_cmd(editor, &loc);
            std::iter::once(s.program).chain(s.args).collect::<Vec<_>>()
        };
        assert_eq!(args("nano"), ["nano", "+3,7", "/a b.rs"]);
        assert_eq!(args("nvim"), ["nvim", "+3", "/a b.rs"]);
        assert_eq!(args("code --wait"), ["code", "--wait", "-g", "/a b.rs:3:7"]);
        assert_eq!(
            args(r#""C:\Program Files\Notepad++\notepad++.exe""#),
            [r"C:\Program Files\Notepad++\notepad++.exe", "-n3", "-c7", "/a b.rs"]
        );
    }
}
