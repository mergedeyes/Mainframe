//! Deploy targets (`D`): build, copy the result to another machine over ssh and swap it
//! in for the binary there, keeping the old one as a backup. Here runs in Rust; there, a
//! short sh script per step does the work, since the other end is a Unix machine.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{
    browser::human_size,
    cmd::{Step, split_words},
    config::{self, path_key},
    project::quote_str,
    run::{Ctx, Exit},
};

/// What `D` does with a target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Run {
    Deploy,
    Rollback,
    Backups,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    /// A systemd service on the remote, restarted (and created if missing) after the swap.
    pub service: String,
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
            service: String::new(),
            restart: String::new(),
            keep: 3,
            ssh: String::new(),
            interactive: false,
            key: String::new(),
        }
    }
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
        if !self.service.trim().chars().all(|c| c.is_ascii_alphanumeric() || "-_.@:".contains(c)) {
            return Some("service must be a systemd unit name, like myapp".into());
        }
        if !self.key.trim().is_empty() && !config::valid_key(self.key.trim()) {
            return Some(format!("key {:?} isn't a key mainframe knows, like F9 or ctrl-d", self.key.trim()));
        }
        None
    }

    /// Where a `from` build happens: one checkout per project and target.
    pub fn build_dir(&self, root: &Path) -> PathBuf {
        config::cache_dir().join("deploy").join(format!("{}--{}", path_key(root), file_safe(&self.name)))
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
        let mut shown =
            artifact.strip_prefix(root).map_or_else(|_| config::pretty_path(&artifact), |r| r.display().to_string());
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
        if let Some(svc) = self.unit() {
            steps.push(format!("restart {svc} and check that it runs (created if it doesn't exist)"));
        }
        if !self.restart.trim().is_empty() {
            steps.push(format!("on {}: {}", self.host, self.restart.trim()));
        }
        steps
    }

    /// `service` as a unit name: `myapp` → `myapp.service`.
    fn unit(&self) -> Option<String> {
        let s = self.service.trim();
        if s.is_empty() {
            None
        } else if s.contains('.') {
            Some(s.to_string())
        } else {
            Some(format!("{s}.service"))
        }
    }

    fn sudo_prefix(&self) -> &'static str {
        match (self.sudo, self.interactive) {
            (false, _) => "",
            // Background tasks have no terminal: fail right away instead of waiting for a password.
            (true, false) => "sudo -n ",
            (true, true) => "sudo ",
        }
    }

    /// ssh with this target's options, up to the host.
    fn ssh(&self, tty: bool) -> Step {
        let mut s = Step::new(ssh_program()).args(["-o", "ConnectTimeout=10"]);
        if self.interactive {
            // Several connections, one password: they share the first one. Windows' ssh can't.
            if cfg!(unix) {
                let dir = std::env::var_os("XDG_RUNTIME_DIR").map_or_else(std::env::temp_dir, PathBuf::from);
                let sock = dir.join("mainframe-ssh-%C");
                s = s.args(["-o", "ControlMaster=auto", "-o"]).arg(format!("ControlPath={}", sock.display()));
                s = s.args(["-o", "ControlPersist=60"]);
            }
            if tty {
                s = s.arg("-t");
            }
        } else {
            // No terminal to ask on; a key (or agent) has to do.
            s = s.args(["-o", "BatchMode=yes"]);
        }
        s.args(split_words(self.ssh.trim())).arg(self.host.trim())
    }

    /// `ssh … host 'sh -c …'`: the remote login shell may be fish or anything, so the
    /// script always runs under sh there.
    fn remote(&self, script: &str, tty: bool) -> Step {
        self.ssh(tty).arg(format!("sh -c {}", quote_str(script)))
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
        if self.sudo && !self.interactive { " (sudo without password? else set interactive = true)" } else { "" }
    }

    /// Remote: stdin into the upload's place in /tmp.
    fn upload_script(&self) -> String {
        format!("set -e\n{}\ncat > \"$tmp\"", self.remote_tmp())
    }

    /// Remote: the upload swapped in for `remote_path`, which is backed up first, with its
    /// mode and owner kept; older backups beyond `keep` go.
    fn replace_script(&self) -> String {
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
        format!(
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
        )
    }

    /// Remote: the newest backup back in place. It's moved, not copied, so rolling back again
    /// goes one further.
    fn restore_script(&self) -> String {
        let s = self.sudo_prefix();
        format!(
            r#"set -e
dst={dst}
bak=$(ls -1d "$dst".bak-* 2>/dev/null | sort -r | head -n 1)
[ -n "$bak" ] || {{ echo "no backup of $dst"; exit 1; }}
{s}mv -f "$bak" "$dst"
echo "✔ restored $bak"
echo "$(ls -1d "$dst".bak-* 2>/dev/null | wc -l) older backup(s) left"
"#,
            dst = quote_str(&self.remote_path),
        )
    }

    /// Remote: the binary and its backups.
    fn list_script(&self) -> String {
        format!(
            "dst={}\nls -lh \"$dst\" \"$dst\".bak-* 2>/dev/null || echo \"nothing at $dst yet\"",
            quote_str(&self.remote_path)
        )
    }

    /// [`Builtin::Deploy`]: deploys, rolls back or lists backups, from the project at `root`.
    pub fn run(&self, ctx: &mut Ctx, root: &Path, keep_build: bool, what: Run) -> Result<(), String> {
        self.run_in(ctx, root, &self.build_dir(root), keep_build, what)
    }

    /// `run`, with the checkout for a `from` build at `work`.
    fn run_in(&self, ctx: &mut Ctx, root: &Path, work: &Path, keep_build: bool, what: Run) -> Result<(), String> {
        match what {
            Run::Deploy => {}
            Run::Rollback => {
                ctx.heading(format!("roll back {} on {}", self.remote_path, self.host.trim()));
                let failed = format!("roll back failed{}", self.hint_sudo());
                self.on_remote(ctx, &self.restore_script(), true, failed)?;
                return self.restart(ctx, "the backup is back in place");
            }
            Run::Backups => return self.on_remote(ctx, &self.list_script(), false, self.hint_ssh().into()),
        }
        let from = self.from.trim();
        if from.is_empty() {
            return self.build_and_ship(ctx, root, &self.artifact_path(root));
        }
        let prefix = self.checkout(ctx, root, from, work)?;
        let result = self.build_checkout(ctx, work, &prefix);
        if !keep_build {
            ctx.heading("remove build checkout");
            let remove = git_in(root).args(["worktree", "remove", "--force"]).path(work);
            if ctx.ask(&remove).is_some() {
                ctx.say(format!("removed {}", work.display()));
            } else {
                ctx.failure(format!("couldn't remove {} — git worktree remove --force does", work.display()));
            }
        }
        result
    }

    /// Checks `from` out in its own worktree at `work` (reused if it's there). The project's
    /// folder inside the repo, which is the same folder in there.
    fn checkout(&self, ctx: &mut Ctx, root: &Path, from: &str, work: &Path) -> Result<String, String> {
        let git = || git_in(root);
        let prefix = ctx
            .ask(&git().args(["rev-parse", "--show-prefix"]))
            .ok_or_else(|| format!("from = {from} needs a git repository"))?;
        ctx.ask(&git().args(["rev-parse", "--verify", "--quiet", &format!("{from}^{{commit}}")]))
            .ok_or_else(|| format!("no branch or commit {from} here"))?;
        ctx.heading(format!("check out {from} → {}", config::pretty_path(work)));
        if work.join(".git").exists() {
            let update = || "could not update the build checkout".to_string();
            ctx.must(&git_in(work).args(["checkout", "-q", "--detach", "--force", from]), update)?;
            ctx.must(&git_in(work).args(["clean", "-q", "-fd"]), update)?;
        } else {
            let _ = std::fs::remove_dir_all(work);
            let _ = ctx.ask(&git().args(["worktree", "prune"]));
            if let Some(parent) = work.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let add = git().args(["worktree", "add", "-q", "--detach"]).path(work).arg(from);
            ctx.must(&add, || "could not create the build checkout".into())?;
        }
        Ok(prefix)
    }

    fn build_checkout(&self, ctx: &mut Ctx, work: &Path, prefix: &str) -> Result<(), String> {
        if work.join(".gitmodules").is_file() {
            let update = git_in(work).args(["submodule", "update", "-q", "--init", "--recursive"]);
            ctx.must(&update, || "could not fetch submodules".into())?;
        }
        if let Some(head) = ctx.ask(&git_in(work).args(["log", "-1", "--format=%h %s"])) {
            ctx.say(format!("building {head}"));
        }
        let dir = work.join(prefix);
        if !dir.is_dir() {
            return Err(format!("no {prefix} in the checkout"));
        }
        let p = config::expand_tilde(self.artifact.trim());
        let artifact = if p.is_absolute() { p } else { dir.join(p) };
        self.build_and_ship(ctx, &dir, &artifact)
    }

    /// Builds in `dir`, uploads `artifact` and swaps it in, then restarts what's set up.
    fn build_and_ship(&self, ctx: &mut Ctx, dir: &Path, artifact: &Path) -> Result<(), String> {
        let build = self.build.trim();
        if !build.is_empty() {
            ctx.heading(format!("build · {build}"));
            // Written by the user, so it's their shell's.
            let mut c = ctx.shell.command(build);
            c.current_dir(dir);
            if runs_cross(build) {
                c.env("CROSS_CONTAINER_OPTS", cross_container_opts());
            }
            let shell = ctx.shell.program.to_string_lossy().into_owned();
            match ctx.spawn(c, &shell) {
                e if e.ok() => {}
                Exit::Killed => return Err("stopped".into()),
                _ => return Err("build failed — nothing was copied".into()),
            }
        }
        let Some(meta) = std::fs::metadata(artifact).ok().filter(|m| m.is_file()) else {
            return Err(format!("no file at {} — check artifact in the deploy settings", artifact.display()));
        };
        ctx.heading(format!("copy {} → {}", human_size(meta.len()), self.host.trim()));
        let file = std::fs::File::open(artifact).map_err(|e| format!("can't read {}: {e}", artifact.display()))?;
        let mut upload = self.remote(&self.upload_script(), false).command();
        upload.current_dir(&ctx.cwd);
        match ctx.spawn_input(upload, "ssh", file) {
            e if e.ok() => {}
            Exit::Killed => return Err("stopped".into()),
            _ => return Err(self.hint_ssh().into()),
        }
        ctx.heading(format!("swap in {}", self.remote_path));
        let failed = format!("replacing failed{} — the binary there is the old one", self.hint_sudo());
        self.on_remote(ctx, &self.replace_script(), true, failed)?;
        self.restart(ctx, "the new binary is in place, D then u rolls back")?;
        ctx.success("deployed");
        Ok(())
    }

    /// Runs `script` on the host; a failure is `failed`.
    fn on_remote(&self, ctx: &mut Ctx, script: &str, tty: bool, failed: String) -> Result<(), String> {
        let mut c = self.remote(script, tty).command();
        c.current_dir(&ctx.cwd);
        match ctx.spawn(c, "ssh") {
            e if e.ok() => Ok(()),
            Exit::Killed => Err("stopped".into()),
            _ => Err(failed),
        }
    }

    fn restart(&self, ctx: &mut Ctx, after: &str) -> Result<(), String> {
        if let Some(svc) = self.unit() {
            ctx.heading(format!("restart {svc}"));
            self.on_remote(ctx, &self.service_script(&svc), true, format!("{svc} isn't running — {after}"))?;
        }
        let restart = self.restart.trim();
        if !restart.is_empty() {
            ctx.heading(format!("restart · {restart}"));
            self.on_remote(ctx, restart, true, format!("restart failed — {after}"))?;
        }
        Ok(())
    }
}

/// git, working in `dir`.
fn git_in(dir: &Path) -> Step {
    Step::new("git").arg("-C").path(dir)
}

/// The ssh program; tests put a stand-in there.
fn ssh_program() -> String {
    #[cfg(test)]
    if let Some(p) = tests::SSH.with(|s| s.borrow().clone()) {
        return p;
    }
    "ssh".into()
}

impl Deploy {
    /// Remote half of `service`: a system unit with sudo or as root, else a user unit.
    /// A unit that's there is only restarted; a missing one gets a minimal unit that
    /// runs `remote_path`, which you can edit there afterwards.
    fn service_script(&self, svc: &str) -> String {
        let s = self.sudo_prefix();
        let exec = if self.remote_path.contains(char::is_whitespace) {
            format!("\"{}\"", self.remote_path)
        } else {
            self.remote_path.clone()
        };
        let lines = [
            "[Unit]".to_string(),
            format!("Description={} (deployed with mainframe)", self.remote_file_name()),
            "After=network-online.target".into(),
            "Wants=network-online.target".into(),
            String::new(),
            "[Service]".into(),
            format!("ExecStart={exec}"),
            "Restart=on-failure".into(),
            String::new(),
            "[Install]".into(),
            "WantedBy=$wanted".into(),
        ];
        // $wanted is filled in by the remote shell; everything else is literal.
        let unit_text = lines
            .iter()
            .map(|l| if l.contains("$wanted") { format!("\"{l}\"") } else { quote_str(l) })
            .collect::<Vec<_>>()
            .join(" ");
        format!(
            r#"set -e
svc={svc}
if [ -n "{s}" ] || [ "$(id -u)" = 0 ]; then
  sc="{s}systemctl"; unit="/etc/systemd/system/$svc"; wanted=multi-user.target; logs="{s}journalctl"
else
  sc="systemctl --user"; unit="$HOME/.config/systemd/user/$svc"; wanted=default.target; logs="journalctl --user"
  mkdir -p "$(dirname "$unit")"
fi
if ! $sc cat "$svc" >/dev/null 2>&1; then
  printf '%s\n' {unit_text} | {s}tee "$unit" >/dev/null
  $sc daemon-reload
  $sc enable -q "$svc"
  echo "created $unit — edit it there for arguments, user or environment"
fi
$sc restart "$svc"
sleep 2
if $sc is-active -q "$svc"; then
  echo "✔ $svc is running"
else
  $sc status --no-pager -l "$svc" || true
  $logs -u "$svc" -n 20 --no-pager -q 2>/dev/null || true
  exit 1
fi
"#,
            svc = quote_str(svc),
        )
    }
}

/// cross mounts your cargo home into its container, config included, and settings made
/// for this machine (an sccache wrapper, `linker = "clang"`, mold) break the build in
/// there. The container sees an empty file in place of the config; yours isn't touched.
fn cross_container_opts() -> String {
    let home = std::env::var_os("CARGO_HOME").map_or_else(|| config::home().join(".cargo"), PathBuf::from);
    let mut opts = std::env::var("CROSS_CONTAINER_OPTS").unwrap_or_default();
    for f in ["config.toml", "config"] {
        if home.join(f).is_file() {
            if !opts.is_empty() {
                opts.push(' ');
            }
            opts.push_str(&format!("-v /dev/null:/cargo/{f}:ro"));
        }
    }
    opts
}

/// Whether a build command calls `cross`, like `RUSTFLAGS=… cross build` or `npm ci && cross build`.
fn runs_cross(build: &str) -> bool {
    build.split(|c: char| c.is_whitespace() || "&|;()".contains(c)).any(|w| w == "cross")
}

/// A target's name as part of a file name: anything odd becomes `-`.
fn file_safe(name: &str) -> String {
    name.chars().map(|c| if c.is_ascii_alphanumeric() || "-_.".contains(c) { c } else { '-' }).collect()
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    #[cfg(unix)]
    use crate::run::testing::with_ctx;
    #[cfg(unix)]
    use crate::testutil::stub;

    thread_local! {
        /// The ssh a test's deploy uses, in place of the real one.
        pub static SSH: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
    }

    #[cfg(unix)]
    fn use_ssh_stub(bin: &Path) {
        stub(bin, "ssh", SSH_STUB);
        SSH.with(|s| *s.borrow_mut() = Some(bin.join("ssh").to_string_lossy().into_owned()));
    }
    #[cfg_attr(windows, allow(unused_imports))]
    use crate::testutil::{git, temp_dir};
    #[cfg_attr(windows, allow(unused_imports))]
    use std::{fs, process::Command};

    /// Drops ssh's options and the host, then runs the remote command through sh like sshd does.
    #[cfg(unix)]
    const SSH_STUB: &str = "#!/bin/sh\nwhile [ $# -gt 1 ]; do case $1 in -o) shift 2;; -t) shift;; *) shift; break;; esac; done\nexec sh -c \"$*\"\n";

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

    #[cfg(unix)]
    fn sh_syntax_ok(script: &str) -> bool {
        Command::new("sh").args(["-n", "-c", script]).status().unwrap().success()
    }

    #[test]
    fn spots_cross() {
        assert!(runs_cross("RUSTFLAGS='-C x' cross build --release"));
        assert!(runs_cross("npm ci&&cross build"));
        assert!(!runs_cross("cargo build && echo across"));
    }

    /// The remote half of `service` against stub systemctl/journalctl: creates the unit
    /// when it's missing, only restarts it after that, fails when it doesn't come up.
    // Runs a sh script; Windows gets these once the script is Rust.
    #[cfg(unix)]
    #[test]
    fn service_is_created_once_then_restarted() {
        let base = temp_dir("service-test");
        let bin = base.join("bin");
        let home = base.join("home");
        fs::create_dir_all(&home).unwrap();
        let log = base.join("log");
        // `cat` works once the unit file exists; `is-active` fails when $DOWN is set.
        let systemctl = format!(
            "#!/bin/sh\n[ \"$1\" = --user ] && shift\necho \"$*\" >> {log}\ncase $1 in\n  cat) [ -f \"$HOME/.config/systemd/user/$2\" ];;\n  is-active) [ -z \"$DOWN\" ];;\nesac\n",
            log = log.display()
        );
        stub(&bin, "systemctl", &systemctl);
        stub(&bin, "journalctl", "#!/bin/sh\necho logs\n");
        stub(&bin, "sleep", "#!/bin/sh\n");

        let d =
            Deploy { sudo: false, service: "my app".replace(' ', "-"), remote_path: "/opt/my app/run".into(), ..pi() };
        let script = d.service_script(&d.unit().unwrap());
        let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
        let run = |down: bool| {
            let mut c = Command::new("sh");
            c.args(["-c", &script]).env("PATH", &path).env("HOME", &home);
            if down {
                c.env("DOWN", "1");
            }
            c.output().unwrap()
        };
        if String::from_utf8_lossy(&Command::new("id").arg("-u").output().unwrap().stdout).trim() == "0" {
            return; // as root it'd write to /etc/systemd/system
        }

        let first = run(false);
        assert!(first.status.success(), "{}", String::from_utf8_lossy(&first.stderr));
        let unit = fs::read_to_string(home.join(".config/systemd/user/my-app.service")).unwrap();
        assert!(unit.contains("ExecStart=\"/opt/my app/run\"\n") && unit.contains("WantedBy=default.target"), "{unit}");
        let calls = fs::read_to_string(&log).unwrap();
        assert!(
            calls.contains("daemon-reload")
                && calls.contains("enable -q my-app.service")
                && calls.contains("restart my-app.service")
        );

        fs::write(&log, "").unwrap();
        assert!(run(false).status.success());
        let calls = fs::read_to_string(&log).unwrap();
        assert!(!calls.contains("enable") && calls.contains("restart my-app.service"), "{calls}");

        let down = run(true);
        assert!(!down.status.success());
        assert!(String::from_utf8_lossy(&down.stdout).contains("logs"));
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn problems() {
        assert!(Deploy { remote_path: "bin/x".into(), ..pi() }.problem().is_some());
        assert!(Deploy { remote_path: "/usr/local/bin/".into(), ..pi() }.problem().is_some());
        assert!(Deploy { host: " ".into(), ..pi() }.problem().is_some());
        assert!(pi().problem().is_none());
    }

    // Runs a sh script; Windows gets these once the script is Rust.
    #[cfg(unix)]
    #[test]
    fn remote_scripts_are_valid_sh() {
        let service = Deploy { service: "myapp".into(), ..pi() };
        for d in [pi(), service, Deploy { sudo: false, interactive: true, restart: String::new(), keep: 0, ..pi() }] {
            for script in [d.upload_script(), d.replace_script(), d.restore_script(), d.list_script()] {
                assert!(sh_syntax_ok(&script), "{script}");
            }
            if let Some(unit) = d.unit() {
                assert!(sh_syntax_ok(&d.service_script(&unit)));
            }
        }
    }

    /// Runs a deploy and a rollback against a temp dir, with `ssh` replaced by a stub that
    /// just runs the command locally (so it needs a Unix machine, like the real other end).
    #[cfg(unix)]
    #[test]
    fn replaces_with_backup_and_rolls_back() {
        let base = temp_dir("deploy-test");
        let bin = base.join("bin");
        use_ssh_stub(&bin);

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
        let run = |what: Run| {
            let (result, out) = with_ctx(&base, |ctx| d.run(ctx, &base, false, what));
            assert!(result.is_ok(), "{result:?}\n{out}");
            out
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

        let out = run(Run::Deploy);
        assert!(out.contains("deployed"), "{out}");
        assert_eq!(fs::read_to_string(&remote).unwrap(), "new");
        let mode = Command::new("stat").args(["-c", "%a"]).arg(&remote).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&mode.stdout).trim(), "750");
        assert_eq!(backups().len(), 1);

        // Same build again: nothing replaced, no extra backup.
        assert!(run(Run::Deploy).contains("already this build"));
        assert_eq!(backups().len(), 1);

        // A newer build: the backup of "new" stays, the older one goes (keep = 1).
        fs::write(base.join("artifact"), "newer").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        run(Run::Deploy);
        assert_eq!(fs::read_to_string(&remote).unwrap(), "newer");
        assert_eq!(backups().len(), 1);

        run(Run::Rollback);
        assert_eq!(fs::read_to_string(&remote).unwrap(), "new");
        assert!(backups().is_empty());
        assert!(run(Run::Backups).contains("app"));

        let _ = fs::remove_dir_all(&base);
    }

    /// `from = "main"` while another branch with uncommitted edits is checked out, in a
    /// project that's a subfolder of its repo: main's build goes out, the checkout is
    /// removed afterwards (or kept when asked), the working copy stays as it was.
    #[cfg(unix)]
    #[test]
    fn from_builds_the_branch_in_a_separate_checkout() {
        let base = temp_dir("deploy-from");
        let (bin, repo, work) = (base.join("bin"), base.join("repo"), base.join("cache/work"));
        let proj = repo.join("app");
        fs::create_dir_all(&proj).unwrap();
        use_ssh_stub(&bin);
        let git = |args: &[&str]| drop(git(&repo, args));
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
        let run = |d: &Deploy, keep: bool| {
            let (result, out) = with_ctx(&proj, |ctx| d.run_in(ctx, &proj, &work, keep, Run::Deploy));
            match result {
                Ok(()) => (true, out),
                Err(e) => (false, format!("{out}{e}")),
            }
        };

        let (ok, out) = run(&d, false);
        let deployed = fs::read_to_string(&remote).ok();
        let left_behind = work.exists();
        let (ok_kept, out_kept) = run(&d, true);
        let kept = work.join("app/out/app").exists();
        // A missing branch fails before anything is built or copied.
        let (ok_missing, out_missing) = run(&Deploy { from: "nope".into(), ..d.clone() }, false);
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
