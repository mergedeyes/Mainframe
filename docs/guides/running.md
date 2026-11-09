# Running things

Everything Mainframe runs, it runs in the output pane: builds, tests, your
program, git commands, deploys. Each task shows the exact command first, so
you can always see what ran.

## Build, run and test

| Key | What it does |
|---|---|
| `b` | build |
| `B` | release build |
| `t` | test |
| `r` | run |
| `R` | run in the full terminal, for programs that ask for input |

What these mean depends on the project; see the table in the
[introduction](../introduction.md#projects). With a file selected, `r`
runs that file instead:

- a Rust file in `src/bin/` or `examples/`: `cargo run --bin …` or `--example …`
- a script: `.py`, `.sh`, `.bash`, `.zsh`, `.fish`, `.js`, `.ts`, `.rb`, `.lua`, `.pl`, `.php`, `.go`
- a single `.rs`, `.c` or `.cpp` file: compiled to a temporary program and run
- anything executable

The `runners` setting in the [configuration](../reference/config.md)
changes or adds commands per file extension.

## The task picker

`T` lists everything the project can run: the build tool's usual commands,
every cargo binary and example, `go run ./cmd/…`, Python entry points,
package.json scripts, make targets and just recipes. Type to filter,
`Enter` runs the selected one, `Ctrl-R` runs it in the full terminal.

## Any command

`:` runs a shell command in the current folder, with Tab completing
programs and paths. ↑ and ↓ go through the commands you ran before; type the
start of one first to see only those. `!` opens your shell in the current
folder; leave it with `exit` to come back.

For commands you run often, give them a key with a
[custom action](../reference/config.md#custom-actions).

## Output tabs

Starting a task while another one is running opens a new tab, so a dev
server can keep running while you build. Up to 9 tasks run at once.

| Key | What it does |
|---|---|
| `[` `]` | previous, next tab |
| `x` | stop the shown task; once it's done, close the tab |
| `Ctrl-C` | stop the shown task (it doesn't quit Mainframe; `q` does) |
| `@` | run the shown tab's command again |
| `w` | hide or show the output pane |
| `z` | make the output fill the screen, and back |
| `+` `-` | make the output pane bigger, smaller |
| `Tab` | move the focus between the file list and the output |

With the output focused, `j` `k`, `PgUp` `PgDn`, `Home` and `End` scroll
it. `End` follows new output again.

Tasks run in the background without a terminal. A command that asks for a
password fails right away instead of waiting forever. Use `R`, `!` or an
`interactive` custom action for those.

## Each project keeps its output

Every project in your [projects folder](../introduction.md#your-projects-folder)
has its own tabs. Move to another project, with `J` or by browsing there,
and you see the output it had when you left. The tasks of the project you
left keep running in the background.

The tabs are saved with their colors (the last 5,000 lines of each), so
after a restart every project shows its output again, and `@`, `n` and `N`
still work. Tasks that were still running when you quit are marked as
stopped.

## When a task finishes

A task that ran for at least 10 seconds rings the terminal bell and sends a
desktop notification when it ends, also when it belongs to a project you
moved away from. `notify_after` in the configuration changes the 10
seconds, 0 turns it off.

## Jumping to errors

`n` jumps to the next `file:line` in the output: compiler errors, warnings,
panics, Python tracebacks. `N` goes to the previous one. With the output
focused (`Tab`), `Enter` opens the file in your editor at that line, `o` in
VS Code.

## Watch mode

`W` turns on watch mode for the shown tab: whenever a file in the project
changes, its command runs again. A task that is still running, like a
server, is restarted. `W` again turns it off.

Watch mode pauses while you're in another project and catches up when you
come back.

## Searching and saving output

With the output focused (`Tab`):

- `/` searches it; `Enter` keeps the search, then `n` and `N` step through
  the matches, `Esc` clears it
- `Ctrl-S` saves the whole output to a file; it asks for the name
