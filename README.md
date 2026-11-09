# Mainframe

A terminal control room: browse files, edit, build, run, install and git — one key each.

```
cargo install --path . --root ~/.local   # → ~/.local/bin; or run mainframe, press i, Enter on the release build
mainframe [DIR]
```

The file browser fills the screen; an output pane opens underneath when something runs.
Starting a task while another runs opens it in a new output tab, so a dev server can keep going while you build.
Each project keeps its own output tabs: moving into another project (with `J` or just by browsing there) brings
back the output it had when you left, while the previous project's tasks keep running and notify when they finish.
Folders outside any project keep whatever tabs you had. Watch mode (`W`) pauses while its project isn't shown and
catches up when you return. The tabs are saved with their colors (in `~/.local/state/mainframe/output/`, the last
5000 lines per tab), so after a restart each project shows its output again, `@` reruns and `n N` still jump;
tasks still running when you quit are marked as stopped.
The file list, git markers and the branch badge (↑ ↓ ●) refresh by themselves when something changes elsewhere: files anywhere in the repo, commits, checkouts, fetch, push and pull, also in linked worktrees.
Press `?` inside for all keys.

## Keys

| key | does |
|---|---|
| `j k` / arrows, `h l` | move, parent, enter |
| `Enter` | enter dir / edit text file / run executable |
| `/` · `c` · `Z` · `~` · `.` | filter · go to path or zoxide keywords · zoxide picker · home · toggle hidden |
| `J` | jump to a project in your projects folder (`~/Projekte`, `~/Projects`, … or `projects_dir`): most recently active first, with kind, running tasks and open todos; group folders like `Games/` are opened up into their projects; type to filter, `Enter` goes there, `^T` its notes, `^O` VS Code, `^G` makes a new project (see below) |
| `M` | notes and todos for the project you're in: `a` todo, `n` note line, `Enter` edit, `Space` checks a todo off, `t` todo ⇄ note, `d` delete, `u` undo, `J K` move, `C` clears checked ones, `E` the whole file in the editor, `Esc` closes. The header shows open todos (`☐ 3`) |
| `Ctrl-P` | fuzzy-find any file in the repo / project (`git ls-files`, so ignored files stay out); `Enter` shows it in the list, `^E` edits it |
| `e` | edit in nano (configurable) |
| `o` / `O` | open file / whole project in VS Code |
| `r` / `R` | run in output pane / in the full terminal (for interactive programs) |
| `b` / `B` / `t` | build / release build / test |
| `T` | task picker: the build tool's usual commands, cargo bins and examples, `go run ./cmd/…`, Python entry points, package.json scripts, make targets and just recipes — type to filter, `Enter` runs, `^R` runs in the full terminal |
| `i` | install picker: every executable in the project with profile, target, arch, age, install state; also updates the project's `.desktop` entry (shows a diff first, `d` to review it alone) |
| `D` | deploy to another machine (a Pi, a server): builds, copies the result over ssh and swaps it in for the binary there, keeping the old one as `<file>.bak-<date-time>`; `u` puts the newest backup back, `l` lists them, `a` adds a target, `e` edits the project's settings. With no target yet it asks for one, with guesses for a Pi (ARM build, `cross` if installed) |
| `C` | clean up the project: what its build tools left lying around, with sizes — cargo's debug builds (all targets), the incremental cache alone, release builds, docs or all of `target`; `node_modules` and framework build folders; virtualenvs, `__pycache__` and tool caches; `zig-out`, CMake/Meson build trees, Go's build cache; and the project's own `clean` target. The same project in the repo's other worktrees is listed too, prefixed with the worktree's folder (`windows · debug builds`). Folders git tracks files in are never offered. `Space` marks several, `Enter` asks, then deletes in the output pane |
| `g` | git menu: status, diff, log, stage, commit, push/pull, branches, stash, `gh repo create`; `g r` makes a release: bumps the version in `Cargo.toml` / `package.json` / `pyproject.toml` (and the lock file), commits it on main or through a PR (merged right away, or, when `main` requires checks or reviews or CI runs on PRs, left for you to merge on GitHub once CI passes; the next `g r` then tags it), tags `vX.Y.Z`, pushes the tag and watches the release workflow it starts, or runs `gh release create` when there is none; `g R` lists releases and CI runs |
| `:` / `!` | run a shell command / open your shell here |
| `x` | kill the shown task, or close its tab once it's done (the last tab is cleared, its saved output too) |
| `[` `]` | previous / next output tab |
| `/` `^S` | with the output focused (`Tab`): search it (`n N` then step through matches, `Esc` clears) / save it to a file |
| `@` / `W` | rerun the shown tab's command / watch mode: rerun (restart) it whenever a file in the project changes |
| `n` `N` | jump to the next / previous `file:line` in the output (compiler errors, panics, tracebacks); `Enter` opens it in the editor at that line, `o` in VS Code |
| `a A m d` | new file, new dir, rename/move, trash |
| `Space` | mark entries (across folders); `y d s u` then act on all marked, `Esc` clears |
| `y p P` | yank, paste copy, paste move |
| `s` / `u` | git stage / unstage right in the list (no output pane) |
| `v w z Tab + -` | preview, output pane, zoom output, focus output, resize |
| `V` | modified files preview as their `git diff` (default) or their contents |
| `S` | sort by name, newest first (shows ages) or largest first |
| `,` | edit config (custom key-bound commands live there) |

Notes are Markdown (`- [ ]` / `- [x]` lines are todos, the rest is notes) in
`~/.local/share/mainframe/notes/`, one file per project, so they never show up in `git status`.

In `:`, ↑↓ walk the command history (kept in `~/.local/state/mainframe/history`), narrowed to
what you typed before pressing ↑.

Every prompt completes with Tab (Shift-Tab / ↑↓ cycle): paths in go-to, new, rename and gh's
source folder; programs from `$PATH` and paths in `:` commands; zoxide matches for keywords in
`c`; branch names when switching.

"Run" knows Cargo (incl. `src/bin/*.rs` and `examples/*.rs`), Go, npm, Python (uv / poetry), Zig, CMake,
Meson, Make, just, and single files
(`.rs .c .cpp .py .sh .js .go …`, or anything executable).

Background tasks have no terminal, so anything asking for a password fails fast instead of hanging.
Use `R`, `!` or an `interactive = true` custom action for those.

## New projects

`^G` in the `J` picker asks for a name, a location (the projects folder, or a group folder in it like
`Games/`), a stack (none, rust, node, python) and whether to write a CLAUDE.md. It creates the folder,
runs `git init -b main` and goes there. The CLAUDE.md is local only: it is listed in
`.git/info/exclude`, so it never shows up in `git status` or gets committed.

The CLAUDE.md comes from a built-in template, or from `~/.config/mainframe/CLAUDE.md` when that file
exists. `^T` in the form opens that file in the editor, copying the built-in template there first.
In the template, `{name}` becomes the folder name, and lines between `<!-- if rust -->` and
`<!-- end -->` (likewise `node`, `python`, `none`) only stay when that stack is picked.

## Config

`~/.config/mainframe/config.toml`. Press `,` to create and open it. It has editor, VS Code
command, install dir, projects folder (`projects_dir`), when to notify about finished tasks (`notify_after`, seconds), per-extension
runners and custom actions:

```toml
[[actions]]
key = "F5"
name = "clippy"
cmd = "cargo clippy --all-targets"
```

## Per-project settings and deploy

Each project can have `~/.config/mainframe/projects/<project>.toml` (`D` then `e` creates
and opens it), kept outside the project so hosts never land in a commit. It holds deploy
targets and `[[actions]]` that only exist in that project (they win over the global ones):

```toml
[[deploy]]
name = "pi"
host = "pi@raspberrypi.local"          # anything ssh accepts, incl. ~/.ssh/config aliases
remote_path = "/usr/local/bin/myapp"   # replaced in place, mode and owner kept
build = "cross build --release --target aarch64-unknown-linux-gnu"   # "" skips it
artifact = "target/aarch64-unknown-linux-gnu/release/myapp"
sudo = true
restart = "sudo systemctl restart myapp"   # optional, runs there afterwards
keep = 3            # backups kept on the remote, 0 keeps all
ssh = ""            # extra ssh options, e.g. "-p 2222"
interactive = false # true: runs in the real terminal, for ssh / sudo passwords
key = "F9"          # optional: deploy with one key
```

The upload goes to `/tmp` first and the swap is a rename, so a running binary can be
replaced. An identical binary is left alone (no extra backup). Deploys run in the output pane,
so they need an ssh key that doesn't prompt and, with `sudo = true`, sudo without a password
(Raspberry Pi OS's default user has that); otherwise set `interactive = true`. The script
that ran is in `~/.local/state/mainframe/deploy/`.

## cd on exit (fish)

```fish
function mf
    set -l f (mktemp)
    MAINFRAME_CWD_FILE=$f mainframe $argv
    and cd (cat $f)
    rm -f $f
end
```
