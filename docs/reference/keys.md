# Keys

Every key, by view. Inside Mainframe, `?` shows the most important ones.
Custom actions from the [configuration](config.md) come first: a custom
action on a key replaces what the key does here.

## File list

### Moving

| Key | What it does |
|---|---|
| `j` `k`, ↓ ↑ | down, up |
| `l`, → | open the folder |
| `h`, ←, Backspace | parent folder |
| `PgDn` `PgUp` | a page down, up |
| `Ctrl-D` `Ctrl-U` | half a page down, up |
| `Home` `End` | first, last entry |
| `Enter` | open a folder, edit a text file, run a program, open anything else with the default app |
| `/` | filter the list; `Enter` keeps it, `Esc` clears it |
| `c` | go to a path, or zoxide keywords |
| `Z` | zoxide's picker |
| `Ctrl-P` | find a file in the project |
| `J` | project picker |
| `~` | home folder |
| `.` | show or hide hidden files |
| `S` | sort by name, newest first, largest first |
| `Ctrl-R` | reload |
| `Esc` | clear the filter, then the marks |

### Files

| Key | What it does |
|---|---|
| `a` | new file |
| `A` | new folder |
| `m`, F2 | rename or move |
| `d`, Delete | move to the trash |
| `Space` | mark (for `y`, `d`, `s`, `u`) |
| `y` | yank the marked or selected entries |
| `p` / `P` | paste a copy / move here |
| `s` / `u` | git stage / unstage |

### Doing things

| Key | What it does |
|---|---|
| `e` | edit in your editor |
| `o` / `O` | open the file / the project in VS Code |
| `r` / `R` | run here / in the full terminal |
| `b` / `B` | build / release build |
| `t` | test |
| `T` | task picker |
| `i` | install picker |
| `D` | deploy picker |
| `C` | cleanup |
| `g` | git menu |
| `M` | notes and todos |
| `:` | run a shell command |
| `!` | open your shell here |
| `,` | edit the configuration |
| `?` | help |
| `q` | quit |

### Output

| Key | What it does |
|---|---|
| `[` `]` | previous, next tab |
| `x` | stop the shown task, or close its tab when it's done |
| `Ctrl-C` | stop the shown task |
| `@` | run the shown tab's command again |
| `W` | watch mode on or off |
| `n` `N` | next, previous `file:line` in the output |
| `w` | show or hide the output pane |
| `z` | output fills the screen, and back |
| `+` `-` | output pane bigger, smaller |
| `Tab` | focus the output pane, and back |
| `v` | show or hide the preview |
| `V` | changed files preview as diff or contents |

## Output pane, focused (`Tab`)

| Key | What it does |
|---|---|
| `j` `k`, ↓ ↑ | scroll |
| `PgDn` `PgUp` | a page |
| `Home` `End` | top, bottom (`End` follows new output) |
| `Enter` | open the `file:line` that `n` jumped to, in your editor |
| `o` | the same in VS Code |
| `/` | search; then `n` `N` step through the matches |
| `Ctrl-S` | save the output to a file |
| `Esc` | clear the search, then back to the file list |

## Prompts

The footer prompts (`:`, `c`, `a`, `m`, commit message …):

| Key | What it does |
|---|---|
| `Tab` / Shift-Tab | complete; again to cycle through the suggestions |
| ↓ ↑ | step through the suggestions; in `:` without suggestions, the command history |
| `Enter` | done |
| `Esc`, `Ctrl-C` | cancel |

Tab completes paths, programs on your PATH in `:`, zoxide matches in `c`,
and branch names where a branch is asked for.

## Forms

The forms for new projects, deploy targets, releases and GitHub repos go
field by field. Every field has a default.

| Key | What it does |
|---|---|
| typing | replaces the default |
| `Enter`, ↓, `Tab` | next field (the default stays when you didn't type) |
| ↑, Shift-Tab | previous field; Backspace too, in an empty field |
| ← → or `Space` | change a choice |
| `Enter` or `y` on the last step | go |
| `Esc`, `Ctrl-C` | cancel |
| `Ctrl-T` | in the new-project form: edit your CLAUDE.md template |

## Git menu (`g`)

`s` status · `d` `D` diff, staged diff · `l` log · `L` file history ·
`w` blame · `a` `A` stage selected, all · `u` unstage · `c` commit · `C`
stage all and commit · `e` amend · `x` discard changes · `f` fetch · `p`
pull · `P` push · `B` push another branch · `b` branches · `o` switch · `n`
new branch · `W` remove a worktree · `z` `Z` stash, pop · `r` release ·
`R` releases and CI · `i` init · `h` GitHub repo · `g` lazygit, gitui or
tig. See [Git](../guides/git.md).

## Pickers

**Project picker (`J`):** type to filter · `Enter` go · `Ctrl-T` notes ·
`Ctrl-O` VS Code · `Ctrl-G` new project · `Ctrl-L` CLAUDE.md preset · `Esc`
close

**Task picker (`T`):** type to filter · `Enter` run · `Ctrl-R` run in the
full terminal · `Esc` close

**File finder (`Ctrl-P`):** type to filter · `Enter` show in the list ·
`Ctrl-E` edit · `Esc` close

In these three, ↓ ↑, `Tab` Shift-Tab and `Ctrl-N` `Ctrl-P` move.

**Install picker (`i`):** `Enter` or `i` install · `r` run · `d` desktop
entry · `u` remove a copy that shadows it on PATH · `Esc` close

**Desktop entry review:** `Enter` or `y` update · `n` skip · `j` `k` scroll
· `Esc` cancel all

**Deploy picker (`D`):** `Enter` or `D` deploy · `u` roll back · `l` list
backups · `a` add a target · `e` edit settings · `Esc` close

**Cleanup (`C`):** `Space` mark · `a` mark all like this · `Enter` delete ·
`Esc` close

## Notes (`M`)

| Key | What it does |
|---|---|
| `j` `k`, ↓ ↑ | move |
| `g` `G`, `Home` `End` | first, last line |
| `a` or `o` | add a todo |
| `n` | add a note line |
| `Enter`, `e` or `i` | edit the line |
| `Space` or `x` | check a todo off, or on |
| `t` | todo ⇄ note |
| `d`, Delete | delete |
| `u` | undo |
| `J` `K` | move the line down, up |
| `C` | remove checked todos |
| `E` | the whole file in your editor |
| `Esc`, `q` or `M` | close |
