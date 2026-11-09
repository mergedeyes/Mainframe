# Installation

Mainframe runs on Linux. There are no prebuilt releases yet, so you build it
from source.

## Build and install

You need Rust 1.88 or newer ([rustup.rs](https://rustup.rs)) and git.

```sh
git clone https://github.com/mergedeyes/Mainframe && cd Mainframe
cargo install --path . --root ~/.local
```

This puts `mainframe` into `~/.local/bin`. Make sure that folder is on your
`PATH`. Then start it in any folder:

```sh
mainframe            # the current folder
mainframe ~/Projekte # or any other
```

Press `?` for the keys and `q` to quit.

**Updating:** pull the new version, then either run the `cargo install` line
again, or start Mainframe in its own folder, press `i` and pick the release
build. See [Installing and deploying](guides/install-and-deploy.md).

## Optional tools

Mainframe works without these. Each one switches on a feature:

| Tool | What it adds |
|---|---|
| `nano` or any editor | `e` and `Enter` edit files. nano is the default; set `editor` in the [configuration](reference/config.md). |
| VS Code (`code`) | `o` and `O` open a file or the whole project. |
| [zoxide](https://github.com/ajeetdsouza/zoxide) | `c` accepts keywords like `c watch`, `Z` opens zoxide's picker. |
| `gio` (GLib) or `trash-put` | `d` moves files to the trash instead of deleting them. |
| `notify-send` | A desktop notification when a long task finishes. |
| [GitHub CLI](https://cli.github.com) (`gh`) | Creating GitHub repos, pull requests in releases, release workflows. |
| lazygit, gitui or tig | `g g` opens the first one installed. |
| [cross](https://github.com/cross-rs/cross) | Deploy builds for a Raspberry Pi; suggested automatically when installed. |

## `cd` on exit

When you quit, your shell stays in the folder you started in. To end up in
the folder you were browsing instead, start Mainframe through a small shell
function. Mainframe writes its last folder to the file named in
`MAINFRAME_CWD_FILE`.

**fish** (`~/.config/fish/functions/mf.fish`):

```fish
function mf
    set -l f (mktemp)
    MAINFRAME_CWD_FILE=$f mainframe $argv
    and cd (cat $f)
    rm -f $f
end
```

**bash or zsh** (`~/.bashrc` or `~/.zshrc`):

```sh
mf() {
    local f
    f=$(mktemp)
    MAINFRAME_CWD_FILE=$f mainframe "$@" && cd "$(cat "$f")"
    rm -f "$f"
}
```

Then start it with `mf`.

## Configuration

Press `,` inside Mainframe. It creates `~/.config/mainframe/config.toml` with
every option explained and opens it in your editor. See
[Configuration](reference/config.md).
