# Configuration

Mainframe works without any configuration. Two kinds of files change what it
does:

- `~/.config/mainframe/config.toml`: your settings and custom keys, for
  every project
- `~/.config/mainframe/projects/<project>.toml`: deploy targets and keys for
  one project

Both are [TOML](https://toml.io). A mistake in a file shows up as a message
when Mainframe starts, and the defaults apply until it's fixed.

## config.toml

Press `,` to open it. The first time, Mainframe writes it with every option
explained.

| Option | Default | What it does |
|---|---|---|
| `editor` | `"nano"` | the editor for `e` and `Enter`; anything that waits until you close it, like `"vim"` or `"micro"` |
| `vscode` | `"code"` | the command for `o` and `O` |
| `install_dir` | `"~/.local/bin"` | where `i` installs programs |
| `projects_dir` | `""` | the projects folder for `J`; empty uses the first that exists of `~/Projekte`, `~/Projects`, `~/projects`, `~/Code`, `~/code`, `~/dev`, `~/src`, `~/repos` |
| `show_hidden` | `true` | show hidden files at start (`.` toggles) |
| `preview` | `true` | show the preview at start (`v` toggles) |
| `zoxide` | `true` | use [zoxide](https://github.com/ajeetdsouza/zoxide) when it's installed: keywords in `c`, the `Z` picker, and folders you visit are added to its list |
| `desktop_entries` | `"ask"` | when `i` installs a program with a `.desktop` entry: `"ask"` shows the change first, `"always"` updates it, `"never"` leaves it alone |
| `notify_after` | `10` | ring the bell and send a desktop notification when a task that ran this many seconds finishes; `0` turns it off |
| `keep_deploy_builds` | `false` | keep the build checkout of deploys with `from` for faster rebuilds; see [Deploying a branch](../guides/install-and-deploy.md#deploying-a-branch-not-your-work-in-progress) |

### Runners

`r` on a single file runs it by its extension. `[runners]` changes or adds
those commands. `{file}` becomes the file's full path, `{name}` its name;
the command runs in the file's folder.

```toml
[runners]
py = "python3 -X dev {file}"
lua = "luajit {file}"
```

### Custom actions

A custom action puts a command on a key. It works everywhere in the file
list and comes before the built-in key.

```toml
[[actions]]
key = "F5"
name = "clippy"
cmd = "cargo clippy --all-targets"

[[actions]]
key = "ctrl-g"
name = "lazygit"
cmd = "lazygit"
interactive = true
```

| Field | What it means |
|---|---|
| `key` | a character (`"Z"`), `"F1"` to `"F12"`, `"enter"`, `"tab"`, `"space"`, `"esc"`, `"delete"`, `"insert"`, or one of these with `ctrl-` or `alt-` in front, like `"ctrl-g"` or `"alt-x"` |
| `name` | what the output tab is called |
| `cmd` | the shell command; it runs in the current folder |
| `interactive` | `true` runs it in the full terminal instead of the output pane, for programs that need your input |

These placeholders in `cmd` are replaced, already quoted for the shell:

| Placeholder | Becomes |
|---|---|
| `{file}` | the selected entry, full path |
| `{name}` | the selected entry's name |
| `{dir}` | the folder you're in |
| `{root}` | the project's folder (the folder you're in when there's no project) |

## Per-project settings

Each project can have its own file in `~/.config/mainframe/projects/`. It's
kept outside the project, so host names and the like never end up in a
commit. The file is named after the project's path below your home folder:
`~/Projekte/watcher` has `Projekte%2Fwatcher.toml`.

`D` then `e` creates it, with every setting explained, and opens it.

### Custom actions for one project

`[[actions]]` work like in `config.toml`, but only in this project. On the
same key, the project's action wins.

### Deploy targets

Each `[[deploy]]` is one target for `D`. The form behind `D` → `a` writes
these for you; see [Installing and deploying](../guides/install-and-deploy.md).

```toml
[[deploy]]
name = "pi"
host = "pi@raspberrypi.local"
remote_path = "/usr/local/bin/myapp"
build = "cross build --release --target aarch64-unknown-linux-gnu"
artifact = "target/aarch64-unknown-linux-gnu/release/myapp"
sudo = true
restart = "sudo systemctl restart myapp"
```

| Field | Default | What it means |
|---|---|---|
| `name` | | what the picker calls the target |
| `host` | | where it goes: `user@host`, or a name from `~/.ssh/config` |
| `remote_path` | | the program there that gets replaced; an absolute path |
| `build` | `""` | the build command, run in the project folder; empty skips the build |
| `from` | `""` | a branch or commit to build in a separate checkout instead of the project folder as it is |
| `artifact` | | the built file to copy, relative to the project folder |
| `sudo` | `false` | replace it as root |
| `restart` | `""` | a command to run on the machine afterwards |
| `keep` | `3` | how many backups to keep there; `0` keeps all |
| `ssh` | `""` | extra ssh options, like `"-p 2222 -i ~/.ssh/pi"` |
| `interactive` | `false` | run in the full terminal, so ssh and sudo can ask for passwords |
| `key` | `""` | a key that deploys to this target right away, like `"F9"` |
