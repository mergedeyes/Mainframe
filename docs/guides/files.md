# Files

The file list is where everything starts. This guide covers moving around,
finding files, and creating, renaming, copying and deleting them.

## Moving around

| Key | What it does |
|---|---|
| `j` `k` or ↓ ↑ | move down, up |
| `l` or → | open the folder |
| `h`, ← or Backspace | parent folder |
| `PgDn` `PgUp`, `Ctrl-D` `Ctrl-U` | a page, half a page |
| `Home` `End` | first, last entry |
| `~` | your home folder |
| `.` | show or hide hidden files |
| `S` | sort by name, newest first (with ages) or largest first |
| `Ctrl-R` | reload the list |

`Enter` opens what's selected:

- a folder: goes into it
- a text file: opens it in your editor (`e` does that too)
- a program: runs it
- anything else, like an image or a PDF: opens it with your desktop's default app

`o` opens the selected file in VS Code, `O` the whole project.

## Finding things

**Filter (`/`):** type to narrow the current folder to matching names.
`Enter` keeps the filter; when only one folder is left, it goes straight in.
`Esc` clears it.

**Go to (`c`):** type a path, with Tab to complete it. With
[zoxide](https://github.com/ajeetdsouza/zoxide) installed, keywords work
too: `c watch` goes to the folder you visit most that matches "watch". `Z`
opens zoxide's own picker.

**Find a file (`Ctrl-P`):** search every file in the project or git repo by
a few letters of its path. `apmo` finds `src/app/mod.rs`. Files that git
ignores, like build output, stay out. `Enter` shows the file in the list,
`Ctrl-E` opens it in the editor.

**Jump to a project (`J`):** see
[Projects, notes and Claude](projects.md).

## Creating and renaming

| Key | What it does |
|---|---|
| `a` | new file; a path like `src/new/mod.rs` creates the folders too |
| `A` | new folder |
| `m` or F2 | rename or move; a path moves it, an existing folder moves it into that folder |

All prompts complete paths with Tab. Shift-Tab, ↑ and ↓ step through the
suggestions.

## Copying, moving and deleting

| Key | What it does |
|---|---|
| `Space` | mark the entry (marks stay while you move to other folders) |
| `y` | yank: remember the marked entries, or the selected one |
| `p` | paste a copy here |
| `P` | move them here |
| `d` or Delete | move to the trash, after asking |
| `Esc` | clear the marks |

`d` uses `gio trash` or `trash-put`, so you can restore files from your
desktop's trash. When neither is installed, Mainframe says so in the
question and deletes for good.

## Git right in the list

`s` stages the marked entries (or the selected one), `u` unstages them,
without opening the output pane. The markers next to the names show the git
status: `M` modified (yellow, green once staged), `A` added, `R` renamed,
`D` deleted, `?` untracked, `!` conflict, `•` a folder with changes inside.

For everything else git, see [Git](git.md).

## Preview

The right half shows the selected file, or a folder's contents. For a file
with uncommitted changes it shows its `git diff`.

| Key | What it does |
|---|---|
| `v` | show or hide the preview |
| `V` | changed files: diff or contents |

The preview only shows when the terminal is at least 70 columns wide.
