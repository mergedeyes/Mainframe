# Mainframe

A terminal control room: browse files, edit, build, run, install and git — one key each.

```
cargo install --path . --root ~/.local   # → ~/.local/bin; or run mainframe, press i, Enter on the release build
mainframe [DIR]
```

The file browser fills the screen; an output pane opens underneath when something runs.
Starting a task while another runs opens it in a new output tab, so a dev server can keep going while you build.
The file list, git markers and the branch badge (↑ ↓ ●) refresh by themselves when something changes elsewhere: files anywhere in the repo, commits, checkouts, fetch, push and pull, also in linked worktrees.
Press `?` inside for all keys.

## Keys

| key | does |
|---|---|
| `j k` / arrows, `h l` | move, parent, enter |
| `Enter` | enter dir / edit text file / run executable |
| `/` · `c` · `Z` · `~` · `.` | filter · go to path or zoxide keywords · zoxide picker · home · toggle hidden |
| `J` | jump to a project in your projects folder (`~/Projekte`, `~/Projects`, … or `projects_dir`): most recently active first, with kind and open todos; group folders like `Games/` are opened up into their projects; type to filter, `Enter` goes there, `^T` its notes, `^O` VS Code |
| `M` | notes and todos for the project you're in: `a` todo, `n` note line, `Enter` edit, `Space` checks a todo off, `t` todo ⇄ note, `d` delete, `u` undo, `J K` move, `C` clears checked ones, `E` the whole file in the editor, `Esc` closes. The header shows open todos (`☐ 3`) |
| `Ctrl-P` | fuzzy-find any file in the repo / project (`git ls-files`, so ignored files stay out); `Enter` shows it in the list, `^E` edits it |
| `e` | edit in nano (configurable) |
| `o` / `O` | open file / whole project in VS Code |
| `r` / `R` | run in output pane / in the full terminal (for interactive programs) |
| `b` / `B` / `t` | build / release build / test |
| `T` | task picker: the build tool's usual commands, cargo bins and examples, `go run ./cmd/…`, Python entry points, package.json scripts, make targets and just recipes — type to filter, `Enter` runs, `^R` runs in the full terminal |
| `i` | install picker: every executable in the project with profile, target, arch, age, install state; also updates the project's `.desktop` entry (shows a diff first, `d` to review it alone) |
| `g` | git menu: status, diff, log, stage, commit, push/pull, branches, stash, `gh repo create`; `g r` makes a release: bumps the version in `Cargo.toml` / `package.json` / `pyproject.toml` (and the lock file), commits it on main or through a PR (merged right away, or, when `main` requires checks or reviews or CI runs on PRs, left for you to merge on GitHub once CI passes; the next `g r` then tags it), tags `vX.Y.Z`, pushes the tag and watches the release workflow it starts, or runs `gh release create` when there is none; `g R` lists releases and CI runs |
| `:` / `!` | run a shell command / open your shell here |
| `x` | kill the shown task, or close its tab once it's done |
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

## cd on exit (fish)

```fish
function mf
    set -l f (mktemp)
    MAINFRAME_CWD_FILE=$f mainframe $argv
    and cd (cat $f)
    rm -f $f
end
```
