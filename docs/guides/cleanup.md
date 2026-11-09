# Cleaning up

Build output grows quietly: a Rust project's `target/debug` easily takes
several gigabytes, and every project has its `node_modules` or virtualenv.
`C` finds all of it, shows how much space each part takes, and deletes what
you pick. Your build tools bring everything back the next time you build.

## What it finds

Press `C`. In a project it scans that project. In a folder that isn't a
project, like `~/Projekte` or a group folder, it scans every project below
it.

| Build tool | What it offers |
|---|---|
| Cargo | debug builds (for every target, also cross builds), the incremental cache on its own, release builds, other profiles, docs, or all of `target` with `cargo clean` |
| npm and others | `node_modules`, `dist`, `build`, `out`, `coverage`, `.next`, `.nuxt`, `.output`, `.svelte-kit`, `.turbo`, `.parcel-cache`, `.angular` |
| Python | `.venv`, `venv`, every `__pycache__`, `.pytest_cache`, `.mypy_cache`, `.ruff_cache`, `.tox`, `.nox`, coverage, `build`, `dist`, `*.egg-info` |
| Zig | `zig-out`, `.zig-cache` |
| CMake, Meson | `build`, `build-release`, `builddir`, `cmake-build-*` |
| Go | `build`, `bin`, and Go's build cache (shared by all your Go projects) |
| any | the project's own `clean` target (make, just, npm script, CMake), and folders marked as a cache (`CACHEDIR.TAG`), like a `target` whose `Cargo.toml` is gone |

It also looks at:

- **Projects inside the project,** like a Tauri app's `src-tauri` or the
  members of a Cargo workspace. Their rows start with where they are:
  `src-tauri · debug builds`.
- **The repo's other worktrees,** wherever they are:
  `windows · debug builds`, or from `~/Projekte`,
  `Terminal ⎇ windows · debug builds`.

Folders that git tracks files in are never offered, so a `build/` folder
with your own scripts in it is safe. Hidden folders, `node_modules`,
`target` and virtualenvs aren't searched for projects, which keeps the scan
fast.

## Deleting

| Key | What it does |
|---|---|
| `j` `k` | move |
| `Space` | mark the row |
| `a` | mark every row of the same kind, like all debug builds; again to unmark |
| `Enter` | delete the marked rows, or the selected one, after asking |
| `Esc` | close |

The question says what you're about to delete and how much space it frees,
like `Delete debug builds ×14 (~44.6G on disk, 71.0G of files)? [y/N]`. The
deletion runs in the output pane and ends with what was really freed:

```
cleaned debug builds ×14 — 44.8G freed on disk
```

**Getting all your debug builds at once:** go to your projects folder, press
`C`, then `a` on any "debug builds" row and `Enter`. Each project compiles
from scratch the next time you build it; release builds and installed
programs stay.

The **incremental cache** is a lighter option: the part of a debug build
that grows with every edit. Deleting only that keeps your dependencies
compiled, so the next build is quicker.

## Sizes on btrfs

btrfs can store files compressed. `du` and file managers then show how big
the files are, not how much disk they take, and deleting 71G of files may
free only 45G. On a compressed btrfs, Mainframe shows both: the size column
shows an estimate of the space on disk (`~44.6G`), and the details show the
file size next to it. The estimate comes from compressing a sample of each
folder the way btrfs does. For build output it's usually within a few
percent; folders with many tiny files, like `node_modules`, take a bit less
than estimated.

The line at the end of a deletion is measured, not estimated: Mainframe
checks the free space before and after. If it's far below the estimate,
it says so. The usual reason is btrfs snapshots: a snapshot keeps deleted
files on disk until the snapshot itself is deleted.
