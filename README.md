# Mainframe

A terminal control room: browse files, edit, build, run, install and git — one key each.

```
cargo install --path . --root ~/.local   # → ~/.local/bin; or run mainframe, press i, Enter on the release build
mainframe [DIR]
```

The file browser fills the screen; an output pane opens underneath when something runs.
Starting a task while another runs opens it in a new output tab, so a dev server can keep going while you build.
Press `?` inside for all keys.

## Keys

| key | does |
|---|---|
| `j k` / arrows, `h l` | move, parent, enter |
| `Enter` | enter dir / edit text file / run executable |
| `/` · `c` · `Z` · `~` · `.` | filter · go to path or zoxide keywords · zoxide picker · home · toggle hidden |
| `e` | edit in nano (configurable) |
| `o` / `O` | open file / whole project in VS Code |
| `r` / `R` | run in output pane / in the full terminal (for interactive programs) |
| `b` / `B` / `t` | build / release build / test |
| `T` | task picker: the build tool's usual commands, cargo bins and examples, `go run ./cmd/…`, Python entry points, package.json scripts, make targets and just recipes — type to filter, `Enter` runs, `^R` runs in the full terminal |
| `i` | install picker: every executable in the project with profile, target, arch, age, install state; also updates the project's `.desktop` entry (shows a diff first, `d` to review it alone) |
| `g` | git menu: status, diff, log, stage, commit, push/pull, branches, stash, `gh repo create` |
| `:` / `!` | run a shell command / open your shell here |
| `x` | kill the shown task, or close its tab once it's done |
| `[` `]` | previous / next output tab |
| `n` `N` | jump to the next / previous `file:line` in the output (compiler errors, panics, tracebacks); `Enter` opens it in the editor at that line, `o` in VS Code |
| `a A m d` | new file, new dir, rename/move, trash |
| `y p P` | yank, paste copy, paste move |
| `v w z Tab + -` | preview, output pane, zoom output, focus output, resize |
| `,` | edit config (custom key-bound commands live there) |

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
command, install dir, per-extension runners and custom actions:

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
