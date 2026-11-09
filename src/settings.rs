//! Per-project settings in `$XDG_CONFIG_HOME/mainframe/projects/<project>.toml`, kept outside
//! the project like the notes, so host names never end up in a commit. They hold deploy
//! targets (`D`: build, copy to another machine, swap the binary in place with a backup)
//! and project-only custom actions.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use ratatui::crossterm::event::KeyEvent;
use serde::Deserialize;

use crate::{
    config::{self, Action, Config, path_key},
    project::{quote, quote_str},
};

pub const TEMPLATE: &str = r#"# Mainframe settings for {project}
#
# Deploy targets (D): build, copy the result to another machine over ssh and swap it in
# for the binary there. The old one is kept next to it as <remote_path>.bak-<date-time>.
#
# [[deploy]]
# name = "pi"
# host = "pi@raspberrypi.local"          # anything ssh accepts, incl. Host aliases from ~/.ssh/config
# remote_path = "/usr/local/bin/myapp"   # replaced in place, mode and owner kept
# build = "cross build --release --target aarch64-unknown-linux-gnu"   # runs in the project root; "" skips it
# from = "main"           # optional: build this branch (or commit) in a separate checkout, not the
#                         # folder as it is; keep_deploy_builds in the main config keeps that checkout
# artifact = "target/aarch64-unknown-linux-gnu/release/myapp"           # relative to the project root
# sudo = true             # replace it with sudo (needed for /usr/local/bin and the like)
# restart = "sudo systemctl restart myapp"   # optional, runs on the remote afterwards
# keep = 3                # backups to keep on the remote; 0 keeps all
# ssh = ""                # extra ssh options, e.g. "-p 2222 -i ~/.ssh/pi"
# interactive = false     # true: runs in the real terminal, for ssh / sudo passwords
# key = "F9"              # optional: deploys with one key, no picker
#
# Custom actions only for this project, same as in the main config; they win over those.
#
# [[actions]]
# key = "F6"
# name = "logs on the pi"
# cmd = "ssh pi@raspberrypi.local journalctl -fu myapp"
"#;

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ProjectSettings {
    pub deploy: Vec<Deploy>,
    pub actions: Vec<Action>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct Deploy {
    pub name: String,
    pub host: String,
    pub remote_path: String,
    pub build: String,
    /// A branch or commit to build in a separate checkout; empty builds the project folder.
    pub from: String,
    pub artifact: String,
    pub sudo: bool,
    pub restart: String,
    pub keep: usize,
    pub ssh: String,
    pub interactive: bool,
    pub key: String,
}

impl Default for Deploy {
    fn default() -> Self {
        Self {
            name: String::new(),
            host: String::new(),
            remote_path: String::new(),
            build: String::new(),
            from: String::new(),
            artifact: String::new(),
            sudo: false,
            restart: String::new(),
            keep: 3,
            ssh: String::new(),
            interactive: false,
            key: String::new(),
        }
    }
}

impl ProjectSettings {
    /// The settings of the project at `root`, plus an error when the file is broken.
    pub fn load(root: &Path) -> (Self, Option<String>) {
        let path = file_for(root);
        let Ok(text) = fs::read_to_string(&path) else {
            return (Self::default(), None);
        };
        match toml::from_str::<Self>(&text) {
            Ok(s) => match s.problem() {
                Some(p) => (s, Some(format!("{}: {p}", config::pretty_path(&path)))),
                None => (s, None),
            },
            Err(e) => (Self::default(), Some(format!("{}: {}", config::pretty_path(&path), e.message()))),
        }
    }

    fn problem(&self) -> Option<String> {
        self.deploy.iter().find_map(|d| d.problem().map(|p| format!("deploy {:?}: {p}", d.name)))
    }

    pub fn deploy_for_key(&self, key: &KeyEvent) -> Option<&Deploy> {
        self.deploy.iter().find(|d| !d.key.is_empty() && config::key_matches(&d.key, key))
    }
}

pub fn dir() -> PathBuf {
    Config::path().parent().map_or_else(|| PathBuf::from("."), Path::to_path_buf).join("projects")
}

/// `~/Projekte/Games/bash_emu` → `projects/Projekte%2FGames%2Fbash_emu.toml`.
pub fn file_for(root: &Path) -> PathBuf {
    dir().join(format!("{}.toml", path_key(root)))
}

/// Writes the commented template for `root` unless the file exists already.
pub fn ensure_file(root: &Path) -> io::Result<PathBuf> {
    let path = file_for(root);
    if !path.exists() {
        fs::create_dir_all(dir())?;
        fs::write(&path, template(root))?;
    }
    Ok(path)
}

fn template(root: &Path) -> String {
    TEMPLATE.replace("{project}", &config::pretty_path(root))
}

/// Appends `d` as a `[[deploy]]` table, creating the file (with the template) if needed.
/// Only non-default values are written, so the file stays short.
pub fn append_deploy(root: &Path, d: &Deploy) -> io::Result<PathBuf> {
    let path = ensure_file(root)?;
    let mut text = fs::read_to_string(&path)?;
    if !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(&render_deploy(d));
    fs::write(&path, text)?;
    Ok(path)
}

fn render_deploy(d: &Deploy) -> String {
    let s = |v: &str| toml::Value::String(v.to_string()).to_string();
    let def = Deploy::default();
    let mut out = String::from("\n[[deploy]]\n");
    out += &format!("name = {}\n", s(&d.name));
    out += &format!("host = {}\n", s(&d.host));
    out += &format!("remote_path = {}\n", s(&d.remote_path));
    out += &format!("build = {}\n", s(&d.build));
    if !d.from.is_empty() {
        out += &format!("from = {}\n", s(&d.from));
    }
    out += &format!("artifact = {}\n", s(&d.artifact));
    out += &format!("sudo = {}\n", d.sudo);
    for (k, v, dv) in [("restart", &d.restart, &def.restart), ("ssh", &d.ssh, &def.ssh), ("key", &d.key, &def.key)] {
        if v != dv {
            out += &format!("{k} = {}\n", s(v));
        }
    }
    if d.keep != def.keep {
        out += &format!("keep = {}\n", d.keep);
    }
    if d.interactive {
        out += "interactive = true\n";
    }
    out
}

impl Deploy {
    /// What's missing or wrong, for the form and when loading.
    pub fn problem(&self) -> Option<String> {
        if self.name.trim().is_empty() {
            return Some("needs a name".into());
        }
        if self.host.trim().is_empty() {
            return Some("needs a host (user@host or an ssh alias)".into());
        }
        if !self.remote_path.starts_with('/') {
            return Some("remote_path must be absolute (/usr/local/bin/…)".into());
        }
        if self.remote_path.ends_with('/') {
            return Some("remote_path must name the file, not its folder".into());
        }
        if self.artifact.trim().is_empty() {
            return Some("needs the artifact: the built file to copy".into());
        }
        if self.from.trim().contains(char::is_whitespace) {
            return Some("from must be one branch or commit, like main".into());
        }
        None
    }

    /// Where a `from` build happens: one checkout per project and target.
    pub fn build_dir(&self, root: &Path) -> PathBuf {
        let safe: String = self.name.chars().map(|c| if c.is_ascii_alphanumeric() || "-_.".contains(c) { c } else { '-' }).collect();
        config::cache_dir().join("deploy").join(format!("{}--{safe}", path_key(root)))
    }

    pub fn artifact_path(&self, root: &Path) -> PathBuf {
        let p = config::expand_tilde(self.artifact.trim());
        if p.is_absolute() { p } else { root.join(p) }
    }

    fn remote_file_name(&self) -> &str {
        self.remote_path.rsplit('/').next().unwrap_or("bin")
    }

    /// The steps in plain words, for the picker and the form.
    pub fn summary(&self, root: &Path, keep_build: bool) -> Vec<String> {
        let mut steps = Vec::new();
        let from = self.from.trim();
        if !from.is_empty() {
            let after = if keep_build { "kept for the next deploy" } else { "removed afterwards" };
            steps.push(format!("check out {from} in {} ({after})", config::pretty_path(&self.build_dir(root))));
        }
        if !self.build.trim().is_empty() {
            steps.push(self.build.trim().to_string());
        }
        let artifact = self.artifact_path(root);
        let mut shown = artifact.strip_prefix(root).map_or_else(|_| config::pretty_path(&artifact), |r| r.display().to_string());
        if !from.is_empty() {
            shown = format!("{shown} (of {from})");
        }
        steps.push(format!("copy {shown} → {}", self.host));
        let sudo = if self.sudo { "with sudo, " } else { "" };
        let keep = match self.keep {
            0 => "keeps all backups".to_string(),
            n => format!("keeps {n} backups"),
        };
        steps.push(format!(
            "{sudo}back up {path} as {name}.bak-<date-time> and swap the new one in ({keep})",
            path = self.remote_path,
            name = self.remote_file_name(),
        ));
        if !self.restart.trim().is_empty() {
            steps.push(format!("on {}: {}", self.host, self.restart.trim()));
        }
        steps
    }

    fn sudo_prefix(&self) -> &'static str {
        match (self.sudo, self.interactive) {
            (false, _) => "",
            // Background tasks have no terminal: fail right away instead of waiting for a password.
            (true, false) => "sudo -n ",
            (true, true) => "sudo ",
        }
    }

    fn ssh_cmd(&self, tty: bool) -> String {
        let mut cmd = String::from("ssh -o ConnectTimeout=10");
        if self.interactive {
            // Several connections, one password: they share the first one.
            let dir = std::env::var_os("XDG_RUNTIME_DIR").map_or_else(std::env::temp_dir, PathBuf::from);
            let sock = dir.join("mainframe-ssh-%C");
            cmd += &format!(" -o ControlMaster=auto -o ControlPath={} -o ControlPersist=60", quote(&sock));
            if tty {
                cmd += " -t";
            }
        } else {
            // No terminal to ask on; a key (or agent) has to do.
            cmd += " -o BatchMode=yes";
        }
        if !self.ssh.trim().is_empty() {
            cmd += " ";
            cmd += self.ssh.trim();
        }
        cmd += " ";
        cmd += &quote_str(self.host.trim());
        cmd
    }

    /// `ssh … host 'sh -c …'`: the remote login shell may be fish or anything, so the
    /// script always runs under sh.
    fn remote(&self, script: &str, tty: bool) -> String {
        format!("{} {}", self.ssh_cmd(tty), quote_str(&format!("sh -c {}", quote_str(script))))
    }

    /// Upload goes to /tmp first (no sudo needed), the swap is a rename in the target's folder.
    fn remote_tmp(&self) -> String {
        format!("tmp=\"/tmp/mainframe-$(id -u)-\"{}", quote_str(self.remote_file_name()))
    }

    fn hint_ssh(&self) -> &'static str {
        if self.interactive {
            "ssh failed — is the host reachable?"
        } else {
            "ssh failed — reachable? needs a key without prompt (ssh-copy-id), or interactive = true"
        }
    }

    fn hint_sudo(&self) -> &'static str {
        if self.sudo && !self.interactive {
            " (sudo without password? else set interactive = true)"
        } else {
            ""
        }
    }

    /// The whole deploy as a shell script, run from the project root.
    pub fn script(&self, root: &Path, keep_build: bool) -> String {
        self.script_in(root, &self.build_dir(root), keep_build)
    }

    /// `script`, with the checkout for a `from` build at `work`.
    fn script_in(&self, root: &Path, work: &Path, keep_build: bool) -> String {
        let s = self.sudo_prefix();
        let dst = quote_str(&self.remote_path);
        let keep_tail = if self.keep == 0 {
            String::new()
        } else {
            format!(
                "ls -1d \"$dst\".bak-* 2>/dev/null | sort -r | tail -n +{} | while IFS= read -r f; do {s}rm -f \"$f\" && echo \"dropped old backup $f\"; done\n",
                self.keep + 1
            )
        };
        let replace = format!(
            r#"set -e
dst={dst}
{tmp}
new="$dst.mainframe-new"
if [ -e "$dst" ] && cmp -s "$tmp" "$dst"; then
  rm -f "$tmp"
  echo "$dst is already this build — left as is"
  exit 0
fi
{s}mkdir -p "$(dirname "$dst")"
mode=755
owner=
if [ -e "$dst" ]; then
  bak="$dst.bak-$(date +%Y%m%d-%H%M%S)"
  [ ! -e "$bak" ] || bak="$bak-$$"
  {s}cp -p "$dst" "$bak"
  echo "backup → $bak"
  mode=$(stat -c %a "$dst" 2>/dev/null || echo 755)
  owner=$(stat -c %u:%g "$dst" 2>/dev/null || true)
fi
{s}cp "$tmp" "$new"
{s}chmod "$mode" "$new"
if [ -n "$owner" ]; then {s}chown "$owner" "$new" 2>/dev/null || true; fi
{s}mv -f "$new" "$dst"
rm -f "$tmp"
echo "✔ replaced $dst"
{keep_tail}"#,
            tmp = self.remote_tmp(),
        );
        let upload = format!("set -e\n{}\ncat > \"$tmp\"", self.remote_tmp());

        let mut out = String::from(PRELUDE);
        let from = self.from.trim();
        let artifact = if from.is_empty() {
            quote(&self.artifact_path(root))
        } else {
            out += &self.checkout(from, work, keep_build);
            let p = config::expand_tilde(self.artifact.trim());
            // The project may be a subfolder of the repo: the same subfolder of the checkout.
            if p.is_absolute() { quote(&p) } else { format!("\"$work/$prefix\"{}", quote(&p)) }
        };
        if !self.build.trim().is_empty() {
            out += &format!("step {}\n(\n{}\n) || fail 'build failed — nothing was copied'\n", quote_str(&format!("build · {}", self.build.trim())), self.build.trim());
        }
        out += &format!("a={artifact}\n[ -f \"$a\" ] || fail \"no file at $a — check artifact in the deploy settings\"\n");
        out += &format!("step \"copy $(du -h \"$a\" | cut -f1) → \"{}\n", quote_str(self.host.trim()));
        out += &format!("{} < \"$a\" || fail {}\n", self.remote(&upload, false), quote_str(self.hint_ssh()));
        out += &format!("step {}\n", quote_str(&format!("swap in {}", self.remote_path)));
        out += &format!(
            "{} || fail {}\n",
            self.remote(&replace, true),
            quote_str(&format!("replacing failed{} — the binary there is the old one", self.hint_sudo()))
        );
        self.push_restart(&mut out, "the new binary is in place, D then u rolls back");
        out += "printf '\\033[1;32m✔ deployed\\033[0m\\n'\n";
        out
    }

    /// Script lines that check `from` out in its own worktree at `work` (reused if it's
    /// there) and continue in there; unless kept, it's removed when the script ends,
    /// whether the deploy worked or not.
    fn checkout(&self, from: &str, work: &Path, keep_build: bool) -> String {
        let r = quote_str(from);
        let mut out = format!(
            r#"src=$(pwd)
work={work}
prefix=$(git rev-parse --show-prefix 2>/dev/null) || fail {needs_git}
git rev-parse --verify --quiet {r}^{{commit}} >/dev/null || fail {missing}
step {step}
if [ -e "$work/.git" ]; then
  git -C "$work" checkout -q --detach --force {r} && git -C "$work" clean -q -fd || fail 'could not update the build checkout'
else
  rm -rf "$work"
  git worktree prune
  mkdir -p "$(dirname "$work")"
  git worktree add -q --detach "$work" {r} || fail 'could not create the build checkout'
fi
"#,
            work = quote(work),
            needs_git = quote_str(&format!("from = {from} needs a git repository")),
            missing = quote_str(&format!("no branch or commit {from} here")),
            step = quote_str(&format!("check out {from} → {}", config::pretty_path(work))),
        );
        if !keep_build {
            out += "trap 'step \"remove build checkout\"; git -C \"$src\" worktree remove --force \"$work\" && echo \"removed $work\"' EXIT
";
        }
        out += "[ ! -f \"$work/.gitmodules\" ] || git -C \"$work\" submodule update -q --init --recursive || fail 'could not fetch submodules'
";
        out += "echo \"building $(git -C \"$work\" log -1 --format='%h %s')\"
";
        out += "cd \"$work/$prefix\" || fail \"no $prefix in the checkout\"
";
        out
    }

    /// Puts the newest backup back. It's moved, not copied, so rolling back again goes one further.
    pub fn rollback_script(&self) -> String {
        let s = self.sudo_prefix();
        let restore = format!(
            r#"set -e
dst={dst}
bak=$(ls -1d "$dst".bak-* 2>/dev/null | sort -r | head -n 1)
[ -n "$bak" ] || {{ echo "no backup of $dst"; exit 1; }}
{s}mv -f "$bak" "$dst"
echo "✔ restored $bak"
echo "$(ls -1d "$dst".bak-* 2>/dev/null | wc -l) older backup(s) left"
"#,
            dst = quote_str(&self.remote_path),
        );
        let mut out = String::from(PRELUDE);
        out += &format!("step {}\n", quote_str(&format!("roll back {} on {}", self.remote_path, self.host.trim())));
        out += &format!("{} || fail {}\n", self.remote(&restore, true), quote_str(&format!("roll back failed{}", self.hint_sudo())));
        self.push_restart(&mut out, "the backup is back in place");
        out
    }

    /// Lists the binary and its backups on the remote.
    pub fn backups_script(&self) -> String {
        let list = format!(
            "dst={}\nls -lh \"$dst\" \"$dst\".bak-* 2>/dev/null || echo \"nothing at $dst yet\"",
            quote_str(&self.remote_path)
        );
        format!("{PRELUDE}{} || fail {}\n", self.remote(&list, false), quote_str(self.hint_ssh()))
    }

    fn push_restart(&self, out: &mut String, after: &str) {
        let restart = self.restart.trim();
        if restart.is_empty() {
            return;
        }
        *out += &format!("step {}\n", quote_str(&format!("restart · {restart}")));
        *out += &format!("{} || fail {}\n", self.remote(restart, true), quote_str(&format!("restart failed — {after}")));
    }
}

const PRELUDE: &str = "step() { printf '\\033[1;36m── %s\\033[0m\\n' \"$1\"; }\n\
fail() { printf '\\033[1;31m✘ %s\\033[0m\\n' \"$1\"; exit 1; }\n";

/// Where the script for a run is written, so the output pane shows a short command and
/// you can read exactly what ran.
pub fn script_file(root: &Path, name: &str, what: &str) -> PathBuf {
    let safe: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || "-_.".contains(c) { c } else { '-' }).collect();
    config::state_dir().join("deploy").join(format!("{}--{safe}-{what}.sh", path_key(root)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn pi() -> Deploy {
        Deploy {
            name: "pi".into(),
            host: "pi@raspberrypi.local".into(),
            remote_path: "/usr/local/bin/my app".into(),
            build: "cargo build --release && echo built".into(),
            artifact: "target/aarch64-unknown-linux-gnu/release/myapp".into(),
            sudo: true,
            restart: "sudo systemctl restart myapp".into(),
            ..Deploy::default()
        }
    }

    #[test]
    fn appended_tables_parse_back() {
        let d = Deploy { key: "F9".into(), keep: 5, interactive: true, ..pi() };
        let text = template(Path::new("/x")) + &render_deploy(&d) + &render_deploy(&Deploy { name: "zero".into(), ..pi() });
        let s: ProjectSettings = toml::from_str(&text).unwrap();
        assert_eq!(s.deploy, vec![d, Deploy { name: "zero".into(), ..pi() }]);
        assert!(s.problem().is_none());
    }

    #[test]
    fn template_parses_empty() {
        let s: ProjectSettings = toml::from_str(&template(Path::new("/x"))).unwrap();
        assert!(s.deploy.is_empty() && s.actions.is_empty());
    }

    #[test]
    fn problems() {
        assert!(Deploy { remote_path: "bin/x".into(), ..pi() }.problem().is_some());
        assert!(Deploy { remote_path: "/usr/local/bin/".into(), ..pi() }.problem().is_some());
        assert!(Deploy { host: " ".into(), ..pi() }.problem().is_some());
        assert!(pi().problem().is_none());
    }

    fn sh_syntax_ok(script: &str) -> bool {
        Command::new("sh").args(["-n", "-c", script]).status().unwrap().success()
    }

    #[test]
    fn scripts_are_valid_sh() {
        let root = Path::new("/home/u/proj");
        let from = Deploy { from: "main".into(), ..pi() };
        for d in [pi(), from, Deploy { sudo: false, interactive: true, build: String::new(), restart: String::new(), keep: 0, ..pi() }] {
            assert!(sh_syntax_ok(&d.script(root, false)), "{}", d.script(root, false));
            assert!(sh_syntax_ok(&d.script(root, true)), "{}", d.script(root, true));
            assert!(sh_syntax_ok(&d.rollback_script()));
            assert!(sh_syntax_ok(&d.backups_script()));
        }
    }

    /// Runs the remote half of a deploy and a rollback against a temp dir, with `ssh`
    /// replaced by a stub that just runs the command locally.
    #[test]
    fn replaces_with_backup_and_rolls_back() {
        let base = std::env::temp_dir().join(format!("mainframe-deploy-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let bin = base.join("bin");
        fs::create_dir_all(&bin).unwrap();
        let stub = bin.join("ssh");
        // Drops options and the host, then runs the remote command through sh like sshd does.
        fs::write(&stub, "#!/bin/sh\nwhile [ $# -gt 1 ]; do case $1 in -o) shift 2;; -t) shift;; *) shift; break;; esac; done\nexec sh -c \"$*\"\n").unwrap();
        Command::new("chmod").arg("+x").arg(&stub).status().unwrap();

        let remote = base.join("remote/app");
        fs::create_dir_all(remote.parent().unwrap()).unwrap();
        fs::write(&remote, "old").unwrap();
        Command::new("chmod").arg("750").arg(&remote).status().unwrap();
        fs::write(base.join("artifact"), "new").unwrap();

        let d = Deploy {
            name: "t".into(),
            host: "h".into(),
            remote_path: remote.to_string_lossy().into(),
            build: "echo building".into(),
            artifact: "artifact".into(),
            keep: 1,
            ..Deploy::default()
        };
        let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
        let run = |script: String| {
            let out = Command::new("sh").arg("-c").arg(script).current_dir(&base).env("PATH", &path).output().unwrap();
            assert!(out.status.success(), "{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
            String::from_utf8_lossy(&out.stdout).into_owned()
        };
        let backups = || {
            let mut b: Vec<_> = fs::read_dir(remote.parent().unwrap())
                .unwrap()
                .filter_map(|e| e.ok()?.file_name().into_string().ok())
                .filter(|n| n.starts_with("app.bak-"))
                .collect();
            b.sort();
            b
        };

        let out = run(d.script(&base, false));
        assert!(out.contains("deployed"), "{out}");
        assert_eq!(fs::read_to_string(&remote).unwrap(), "new");
        let mode = Command::new("stat").args(["-c", "%a"]).arg(&remote).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&mode.stdout).trim(), "750");
        assert_eq!(backups().len(), 1);

        // Same build again: nothing replaced, no extra backup.
        assert!(run(d.script(&base, false)).contains("already this build"));
        assert_eq!(backups().len(), 1);

        // A newer build: the backup of "new" stays, the older one goes (keep = 1).
        fs::write(base.join("artifact"), "newer").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        run(d.script(&base, false));
        assert_eq!(fs::read_to_string(&remote).unwrap(), "newer");
        assert_eq!(backups().len(), 1);

        run(d.rollback_script());
        assert_eq!(fs::read_to_string(&remote).unwrap(), "new");
        assert!(backups().is_empty());
        assert!(run(d.backups_script()).contains("app"));

        let _ = fs::remove_dir_all(&base);
    }

    /// `from = "main"` while another branch with uncommitted edits is checked out, in a
    /// project that's a subfolder of its repo: main's build goes out, the checkout is
    /// removed afterwards (or kept when asked), the working copy stays as it was.
    #[test]
    fn from_builds_the_branch_in_a_separate_checkout() {
        let base = std::env::temp_dir().join(format!("mainframe-deploy-from-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let (bin, repo, work) = (base.join("bin"), base.join("repo"), base.join("cache/work"));
        let proj = repo.join("app");
        fs::create_dir_all(&bin).unwrap();
        fs::create_dir_all(&proj).unwrap();
        let stub = bin.join("ssh");
        fs::write(&stub, "#!/bin/sh\nwhile [ $# -gt 1 ]; do case $1 in -o) shift 2;; -t) shift;; *) shift; break;; esac; done\nexec sh -c \"$*\"\n").unwrap();
        Command::new("chmod").arg("+x").arg(&stub).status().unwrap();
        let git = |args: &[&str]| {
            let out = Command::new("git").args(["-c", "user.name=t", "-c", "user.email=t@t"]).args(args).current_dir(&repo).output().unwrap();
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        };
        git(&["init", "-q", "-b", "main"]);
        fs::write(proj.join("version.txt"), "main").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "main"]);
        git(&["switch", "-q", "-c", "dev"]);
        fs::write(proj.join("version.txt"), "dev").unwrap();
        git(&["commit", "-q", "-am", "dev"]);
        fs::write(proj.join("version.txt"), "dev, half done").unwrap();

        // Not "app": the other deploy test uploads to /tmp/mainframe-<uid>-app at the same time.
        let remote = base.join("remote/app-from");
        let d = Deploy {
            name: "t".into(),
            host: "h".into(),
            remote_path: remote.to_string_lossy().into(),
            build: "mkdir -p out && cp version.txt out/app".into(),
            from: "main".into(),
            artifact: "out/app".into(),
            ..Deploy::default()
        };
        let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
        let run = |script: String| {
            let out = Command::new("sh").arg("-c").arg(script).current_dir(&proj).env("PATH", &path).output().unwrap();
            let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
            (out.status.success(), text)
        };

        let (ok, out) = run(d.script_in(&proj, &work, false));
        let deployed = fs::read_to_string(&remote).ok();
        let left_behind = work.exists();
        let (ok_kept, out_kept) = run(d.script_in(&proj, &work, true));
        let kept = work.join("app/out/app").exists();
        // A missing branch fails before anything is built or copied.
        let (ok_missing, out_missing) = run(Deploy { from: "nope".into(), ..d.clone() }.script_in(&proj, &work, false));
        let wc = fs::read_to_string(proj.join("version.txt")).unwrap();
        let _ = fs::remove_dir_all(&base);

        assert!(ok, "{out}");
        assert_eq!(deployed.as_deref(), Some("main"), "{out}");
        assert!(!left_behind, "{out}");
        assert!(out.contains("removed"), "{out}");
        assert!(ok_kept, "{out_kept}");
        assert!(kept, "{out_kept}");
        assert!(!ok_missing && out_missing.contains("no branch or commit nope"), "{out_missing}");
        assert_eq!(wc, "dev, half done");
    }
}
