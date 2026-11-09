# Introduction

Mainframe is a file browser for the terminal that knows what your projects
are. Inside a project, single keys build it, run it, test it, install it,
deploy it and commit it. Everything it runs shows up in an output pane, with
the exact command, so you always see what happened.

## The screen

Roughly like this, with the output pane open:

```
 ~/Projekte/watcher                          main ↑1 ●    cargo · watcher   ☐ 3
┌ files ──────────────────────────┐┌ preview ─────────────────────────────┐
│ ▌ crates/                       ││ [workspace]                          │
│   docs/                         ││ members = ["crates/*"]               │
│   web/                          ││ ...                                  │
│ M Cargo.toml                    ││                                      │
│   README.md                     ││                                      │
└─────────────────────────────────┘└──────────────────────────────────────┘
┌ cargo build ─ cargo test ──────────────────────────────────────────────────┐
│ $ cargo test   in ~/Projekte/watcher                                       │
│ test result: ok. 214 passed; 0 failed                                      │
└────────────────────────────────────────────────────────────────────────────┘
 ? for help
```

- **Header:** where you are, the git branch with commits to push (↑) or pull
  (↓) and a dot (●) for uncommitted changes, the project type and name, and
  the number of open todos.
- **File list:** the current folder, with the git status next to each entry:
  `M` modified (yellow, green once staged), `A` added, `R` renamed, `D`
  deleted, `?` untracked, `!` conflict, and `•` for a folder with changes
  inside.
- **Preview:** the selected file, a folder's contents, or for a modified file
  its `git diff`.
- **Output pane:** opens when something runs. Each task gets its own tab.
- **Footer:** messages, prompts and questions like "Delete …? [y/N]".

The file list, git markers and branch badge update by themselves when
something changes outside Mainframe: files you edit elsewhere, commits,
checkouts, `git pull`, also in other worktrees of the repo.

## Projects

A folder is a project when it has one of these files. The keys `r`, `b`, `B`
and `t` then do what that build tool usually does:

| Build tool | Found by | `b` build | `B` release build | `t` test |
|---|---|---|---|---|
| Cargo | `Cargo.toml` | `cargo build` | `cargo build --release` | `cargo test` |
| Go | `go.mod` | `go build ./...` | same | `go test ./...` |
| npm, pnpm, yarn, bun | `package.json` | `npm run build` | same | `npm test` |
| Python (uv, poetry, pip) | `pyproject.toml`, `setup.py` | `uv build`, `poetry build` or `python -m build` | same | `pytest` (through uv or poetry when the project uses them) |
| Zig | `build.zig` | `zig build` | `zig build -Doptimize=ReleaseFast` | `zig build test` |
| CMake | `CMakeLists.txt` | configure + build in `build/` | in `build-release/` | `ctest` |
| Meson | `meson.build` | `meson compile -C build` | in `build-release/` | `meson test` |
| Make | `Makefile` | `make` | same | `make test` |
| just | `justfile` | `just build` | same | `just test` |

Mainframe looks for these files in the current folder and the folders above
it, but not above your home folder. So in `~/Projekte/watcher/crates/api/src`
the project is the nearest folder with a `Cargo.toml`.

`T` lists everything else a project can run: package.json scripts, make
targets, just recipes, every cargo binary and example, and more. See
[Running things](guides/running.md).

## Your projects folder

`J` lists the projects in your projects folder: `~/Projekte`, `~/Projects`,
`~/projects`, `~/Code`, `~/code`, `~/dev`, `~/src` or `~/repos`, whichever
exists first, or the folder you set as `projects_dir` in the
[configuration](reference/config.md). Group folders like `Games/` are opened
up into the projects inside them.

Each project there has its own output tabs, notes and todos, and settings.
Moving into another project, with `J` or by just browsing there, brings back
the output it had when you left. See
[Projects, notes and Claude](guides/projects.md).

## Where Mainframe keeps things

Nothing is written into your projects unless you ask for it.

| What | Where |
|---|---|
| Configuration | `~/.config/mainframe/config.toml` |
| Per-project settings (deploy targets, actions) | `~/.config/mainframe/projects/` |
| Your CLAUDE.md template | `~/.config/mainframe/CLAUDE.md` |
| Notes and todos | `~/.local/share/mainframe/notes/` |
| Saved output, command history, scripts that ran | `~/.local/state/mainframe/` |
| Deploy build checkouts | `~/.cache/mainframe/deploy/` |

The `XDG_CONFIG_HOME`, `XDG_DATA_HOME`, `XDG_STATE_HOME` and `XDG_CACHE_HOME`
variables move these the usual way.
