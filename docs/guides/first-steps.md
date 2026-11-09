# First steps

A short tour with one of your own projects: open it, build and test it, fix
an error, commit. It takes about ten minutes. You need Mainframe
[installed](../install.md).

## 1. Open a project

Start Mainframe in your projects folder:

```sh
mainframe ~/Projekte
```

Press `J`. The project picker lists every project, the one you worked on
last at the top. Type a few letters of a name to filter, then press `Enter`.

The header now shows the project's type and name, for example
`cargo · watcher`, and its git branch.

## 2. Look around

| Key | What it does |
|---|---|
| `j` `k` or ↓ ↑ | move down, up |
| `l` or → | open the folder |
| `h` or ← | back to the parent folder |
| `Enter` | open a folder, edit a text file, run a program |
| `/` | filter the list as you type, `Esc` clears it |
| `Ctrl-P` | find any file in the project by a few letters of its path |
| `v` | show or hide the preview |
| `?` | all keys |

The preview on the right shows the selected file. For a file with
uncommitted changes it shows the `git diff` instead; `V` switches back to
the contents.

## 3. Build and test

Press `b`. The output pane opens at the bottom and shows the build, starting
with the exact command it runs. Press `t` to run the tests. The second task
opens in a new tab, so the build output stays where it is. `[` and `]`
switch between tabs.

| Key | What it does |
|---|---|
| `b` / `B` | build / release build |
| `t` | test |
| `r` | run the project, or the selected file (`.py`, `.sh`, `.rs`, …) |
| `R` | run in the full terminal, for programs that ask for input |
| `x` | stop the task in the shown tab, or close the tab once it's done |
| `w` / `z` | hide or show the output pane / make it fill the screen |

Mainframe knows the usual commands of Cargo, Go, npm, Python, Zig, CMake,
Meson, Make and just. For anything else the project defines, press `T`: the
task picker lists package.json scripts, make targets, cargo binaries and
more.

## 4. Jump to an error

When the build or the tests fail, press `n`. Mainframe jumps to the next
`file:line` in the output, like a compiler error or a panic, and highlights
it. Press `Tab` to focus the output pane, then `Enter` to open the file in
your editor at that line. `N` goes back to the previous one.

After fixing it, press `@` to run the same command again. Or press `W`
(watch mode): from then on the task runs again by itself whenever you save
a file in the project.

## 5. Commit

Select a changed file and press `s` to stage it (`u` unstages). With
`Space` you can mark several files first. Then press `g` for the git menu:

- `c` asks for a commit message and commits what is staged
- `C` stages everything and commits
- `P` pushes

The header's dot (●) disappears once everything is committed, and ↑1 shows
the commit waiting to be pushed.

## 6. Take a note

Press `M` for the project's notes and todos. `a` adds a todo, `Space`
checks it off. The header shows how many todos are open, and the `J` picker
shows it for every project. Notes are kept outside the project, so they
never end up in a commit.

## Next

- [Running things](running.md): output tabs per project, watch mode, saving and searching output
- [Git](git.md): branches, worktrees, releases
- [Installing and deploying](install-and-deploy.md): your binary on your PATH or on a Raspberry Pi
- [Keys](../reference/keys.md): everything at a glance
