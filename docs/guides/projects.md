# Projects, notes and Claude

Mainframe keeps a few things per project: output tabs, notes and todos, and
settings like deploy targets. All of it lives outside the project, so none
of it ends up in a commit.

## The project picker

`J` lists the projects in your
[projects folder](../introduction.md#your-projects-folder), the most
recently active first. Group folders like `Games/` are opened up into the
projects inside them. Each row shows the project type, tasks still running
in it, and open todos.

Type a few letters to filter, then:

| Key | What it does |
|---|---|
| `Enter` | go there |
| `Ctrl-T` | its notes and todos |
| `Ctrl-O` | open it in VS Code |
| `Ctrl-G` | make a new project |
| `Ctrl-L` | bring the CLAUDE.md preset into it |
| `Esc` | close |

## Notes and todos

`M` opens the notes of the project you're in. Lines with a checkbox are
todos, everything else is a note. Every change is saved right away.

| Key | What it does |
|---|---|
| `a` | add a todo |
| `n` | add a note line |
| `Enter` | edit the line |
| `Space` | check a todo off, or on again |
| `t` | turn a todo into a note, or back |
| `d` | delete the line |
| `u` | undo |
| `J` `K` | move the line down, up |
| `C` | remove all checked todos |
| `E` | the whole file in your editor |
| `Esc` | close |

The header shows the open todos of the current project (☐ 3).

The notes are plain Markdown files in `~/.local/share/mainframe/notes/`, one
per project. `- [ ]` and `- [x]` lines are todos. You can edit them by hand
or with any other tool.

## New projects

`Ctrl-G` in the `J` picker asks for:

1. **name:** the folder name
2. **location:** the projects folder, or a group folder in it like `Games/`
3. **stack:** none, rust, node or python
4. **CLAUDE.md:** whether to write one, see below

Mainframe creates the folder, runs `git init -b main` there and takes you
in.

## CLAUDE.md for Claude Code

[Claude Code](https://claude.com/claude-code) reads a project's `CLAUDE.md`
at every start. Mainframe can write one from a template: the project's
name, the checks to run for its stack (for Rust `cargo fmt`, `cargo clippy`
and `cargo test`), and rules for working on the project, keeping its README
and docs in good shape, and git.

The CLAUDE.md of a new project stays local: it's listed in
`.git/info/exclude`, so it never shows up in `git status` and never gets
committed.

### Your own template

The built-in template is a starting point. To change it, press `Ctrl-T` in
the new-project form. Mainframe copies the built-in template to
`~/.config/mainframe/CLAUDE.md` and opens it in your editor. From then on,
new projects get your version.

In the template, `{name}` becomes the project's folder name. Lines between
`<!-- if rust -->` and `<!-- end -->` only stay for Rust projects; the same
works for `node`, `python` and `none`. So one template can hold the checks
for every stack.

### Bringing the preset into an existing project

`Ctrl-L` in the `J` picker brings the template into the selected project,
with the checks for the build tools it uses. A Tauri app, for example, gets
both the Rust and the Node checks. Mainframe asks first, then:

- **The project has no instructions yet** (no `CLAUDE.md`,
  `.claude/CLAUDE.md`, `CLAUDE.local.md` or `AGENTS.md`): the template
  becomes its local CLAUDE.md, like for a new project.
- **It has instructions already:** Mainframe doesn't touch them. It writes
  the template to `.claude/rules/mainframe-preset.md`, which Claude Code
  loads at its next start, with a note asking Claude to merge it into the
  project's own instructions. What's already there wins where the two
  disagree, and Claude points out each conflict. Then Claude deletes the
  note and tells you what it added and changed.

Git ignores whatever `Ctrl-L` writes.
