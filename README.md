<h1 align="center">Mainframe</h1>

<p align="center">
  <b>A terminal control room for your projects:<br>
  browse, edit, build, run, test, install, deploy and commit, one key each.</b>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/platform-Linux-blue" alt="Platform: Linux">
  <img src="https://img.shields.io/badge/status-early%20development-orange" alt="Status: early development">
</p>

<p align="center">
  <a href="#getting-started">Getting started</a> ·
  <a href="docs/README.md">Documentation</a> ·
  <a href="docs/reference/keys.md">Keys</a> ·
  <a href="https://github.com/mergedeyes/Mainframe/issues">Issues</a>
</p>

Mainframe is a file browser for the terminal that knows what your projects
are. Open a Rust, Go, Node, Python, Zig, CMake, Meson, Make or just project,
and `b` builds it, `t` tests it, `r` runs it. Errors are one key away from
your editor, every project keeps its own output, and your work goes to your
PATH, a Raspberry Pi or a GitHub release without leaving the window.

Everything it runs shows up in an output pane with the exact command first.
Nothing happens behind your back, and nothing is written into your projects
unless you ask for it.

## Why Mainframe

- **One key per job, in every project.** The same keys build, test and run a
  Cargo crate, an npm app or a Python package. `T` lists everything else a
  project can run: scripts, make targets, binaries, examples.
- **Output that stays with its project.** Each project keeps its own output
  tabs. Move on to another one and its builds keep running; come back and
  everything is where you left it, even after a restart. A long task
  notifies you when it's done.
- **From error to fix in two keys.** `n` jumps to the next `file:line` in
  the output, `Enter` opens your editor right there. Watch mode reruns the
  tests whenever you save.
- **Ship it without switching tools.** `i` installs a build on your PATH
  and updates its desktop entry. `D` builds, copies it to a Pi or a server
  over ssh and swaps it in, keeping the old one as a backup. It can build
  a clean `main` while you're in the middle of a feature.
- **Releases in one go.** `g r` bumps the version, commits it (or opens a
  pull request), tags it, pushes and follows the release workflow.
- **Your disk back.** `C` finds debug builds, `node_modules`, virtualenvs and
  caches in all your projects and worktrees, with their real size on disk,
  and clears what you pick.
- **Your projects at a glance.** `J` lists every project, most recently used
  first, with running tasks and open todos. Notes and todos per project stay
  outside the repo, so they never end up in a commit.

## Features

- **Files:** fuzzy file finder, filter, zoxide jumps, marks across folders,
  copy, move, trash, git status on every entry, preview with diffs.
- **Running:** build, release build, test, run single files, task picker,
  output tabs, watch mode, search and save output, jump to `file:line`.
- **Git:** stage from the list, commit, amend, push another branch, branches,
  worktrees, stash, a GitHub repo for any folder, releases with CI.
- **Install and deploy:** install picker with build info and PATH checks,
  desktop entries, deploy targets with backups and rollback.
- **Projects:** project picker, notes and todos, new projects with
  `git init`, a CLAUDE.md template for Claude Code.
- **Cleanup:** build output, dependencies and caches, across projects and
  worktrees, measured as on disk on compressed btrfs.

## Getting started

There are no prebuilt releases yet. Build Mainframe with Rust 1.88 or newer:

```sh
git clone https://github.com/mergedeyes/Mainframe && cd Mainframe
cargo install --path . --root ~/.local
mainframe ~/Projekte
```

Press `J` to pick a project, `b` to build it and `?` for the keys.
[Installation](docs/install.md) lists the optional tools and how to land in
the last folder when you quit. [First steps](docs/guides/first-steps.md) is
a ten-minute tour.

## Documentation

- [Introduction](docs/introduction.md): the screen, which projects it understands, where it keeps things
- [Installation](docs/install.md)
- [First steps](docs/guides/first-steps.md)
- [Keys](docs/reference/keys.md) and [Configuration](docs/reference/config.md)
- [All guides](docs/README.md)

## Status

Mainframe is in early development and runs on Linux. It's used daily, but
keys and settings may still change.

## Contributing

Bug reports and ideas are welcome. Please
[open an issue](https://github.com/mergedeyes/Mainframe/issues) first for
larger changes.
