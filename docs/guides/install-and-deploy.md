# Installing and deploying

`i` puts a program you built onto your own PATH. `D` puts it onto another
machine, like a Raspberry Pi or a server, and keeps the old version there as
a backup.

## Installing on this machine

Press `i` in a project. The install picker lists every program in it: cargo
builds in `target/` (release, debug, examples, cross builds), scripts and
other executables. For each one it shows:

- what it is (release build, debug build, script …) and what it was built
  for (`x86_64 · dynamic`, `aarch64 · static`, …), its size and age
- whether it's installed: ✓ installed, ↑ newer than the installed copy, ↓ older
- a warning when the sources changed after it was built (`b` or `B` builds
  it again)
- a warning when another copy comes earlier on your PATH and would run
  instead

| Key | What it does |
|---|---|
| `Enter` or `i` | install it |
| `r` | run it |
| `d` | review the project's `.desktop` entry for it |
| `u` | remove the other copy that comes earlier on your PATH |
| `Esc` | close |

Installing copies the program to `~/.local/bin` (`install_dir` in the
[configuration](../reference/config.md)). In a Cargo project, the first row
runs `cargo install --path .` instead.

### Desktop entries

When the project has a `.desktop` file that starts the program, installing
also installs or updates that entry, so your app menu starts the version you
just installed. Its `Exec` points at the installed program, and a relative
`Icon` becomes an absolute path. Before changing an entry, Mainframe shows
the diff: `Enter` or `y` updates it, `n` skips it, `Esc` cancels.

`desktop_entries` in the configuration changes this: `"ask"` (the default)
shows the diff, `"always"` updates without asking, `"never"` leaves desktop
entries alone.

## Deploying to another machine

`D` builds the project, copies the result to another machine over ssh and
swaps it in for the program there. The old program stays next to it as
`<name>.bak-<date-time>`, so you can go back.

### The first deploy target

The first time you press `D` in a project, a form asks for the target:

| Field | What it means |
|---|---|
| name | what the picker calls it, like `pi` |
| host | `user@host`, or a name from `~/.ssh/config` |
| remote path | the program on that machine that gets replaced, like `/usr/local/bin/myapp` |
| build | the command that builds it; `-` for none |
| from | a branch to build, like `main`; empty builds your project folder as it is |
| artifact | the built file, relative to the project, like `target/aarch64-unknown-linux-gnu/release/myapp` |
| sudo | replace it as root, needed for `/usr/local/bin` and the like |
| restart | a command to run there afterwards, like `sudo systemctl restart myapp` |
| run | *background* in the output pane, or *terminal* when ssh or sudo ask for a password |

For a Rust project, the form suggests an ARM build for a Raspberry Pi, with
[cross](https://github.com/cross-rs/cross) when it's installed.

The target is saved in `~/.config/mainframe/projects/`, outside the project,
so host names never end up in a commit. `D` then `e` opens that file; see
[Configuration](../reference/config.md#per-project-settings) for every
setting.

### Deploying

`D` opens the picker with your targets. It shows what a deploy will do,
step by step.

| Key | What it does |
|---|---|
| `Enter` or `D` | deploy |
| `u` | roll back: put the newest backup back |
| `l` | list the program and its backups on the machine |
| `a` | add another target |
| `e` | edit the targets in your editor |
| `Esc` | close |

A deploy runs these steps in the output pane:

1. **Build**, with the build command.
2. **Copy** the artifact to `/tmp` on the machine.
3. **Swap it in:** back up the current program, then rename the new one into
   its place. Because it's a rename, a program that is running can be
   replaced. Mode and owner stay the same. If the new build is identical to
   what's there, nothing changes and no backup is made.
4. **Restart**, if you set a restart command.

Only the newest 3 backups are kept (`keep` in the settings). `u` puts the
newest one back; press it again to go back one more.

The script that ran is kept in `~/.local/state/mainframe/deploy/`, so you
can read exactly what happened.

### ssh keys and sudo

Deploys run in the background, where nothing can ask for a password. So the
machine needs your ssh key (`ssh-copy-id user@host`), and with sudo, sudo
without a password. Raspberry Pi OS's default user has that. If not, choose
*terminal* for "run" (`interactive = true`): the deploy then runs in the full
terminal, where ssh and sudo can ask, and you type the password once.

### Deploying a branch, not your work in progress

Usually `D` builds your project folder as it is, including changes you
haven't committed. When you're in the middle of a feature but want to ship
`main`, set `from = "main"` on the target.

Mainframe then checks `main` out in a separate folder under
`~/.cache/mainframe/deploy/`, builds it there and deploys that build. Your
own folder, its branch and your uncommitted changes aren't touched. It
builds your local `main`; pull first to get what's on GitHub, or use
`from = "origin/main"` after a fetch.

**The separate checkout only has what's committed.** Files that git ignores,
like a web UI built into `web/build`, a `.env` or anything generated, aren't
there. Let the build command make them. For a program that embeds a web UI,
for example:

```toml
build = "npm --prefix web ci && npm --prefix web run build && cross build --release --target aarch64-unknown-linux-musl"
```

After the deploy, worked or not, the checkout and its build output are
deleted again, so they don't fill your disk. The next deploy builds from
scratch. With `keep_deploy_builds = true` in the
[configuration](../reference/config.md), the checkout stays, and later
deploys only rebuild what changed.
