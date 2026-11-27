# Installation

Mainframe runs on Linux and Windows. There are no prebuilt releases yet, so
you build it from source.

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

### On Windows

Rust on Windows needs Visual Studio's C++ build tools: the "Desktop development
with C++" workload of Visual Studio or of the free
[Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/).
rustup offers to install them. You also need
[Git for Windows](https://gitforwindows.org). In PowerShell:

```powershell
winget install Rustlang.Rustup Git.Git Microsoft.WindowsTerminal
git clone https://github.com/mergedeyes/Mainframe; cd Mainframe
cargo install --path . --root $HOME\.local
```

That puts `mainframe.exe` into `%USERPROFILE%\.local\bin`. Add that folder to
your PATH (Settings → System → About → Advanced system settings → Environment
Variables), or let Mainframe do it: start it with the full path once, press `i`
in its own folder and install the release build. Installing into a folder
that isn't on your PATH adds it.

Run it in [Windows Terminal](https://aka.ms/terminal); the old console window
works, but draws less well. Commands you write (`:`, actions, runners) run in
PowerShell, builds and the rest without a shell. Mainframe sets up Visual
Studio's C++ environment for its commands by itself, so you don't need a
Developer PowerShell.

**Updating:** pull the new version, then either run the `cargo install` line
again, or start Mainframe in its own folder, press `i` and pick the release
build. See [Installing and deploying](guides/install-and-deploy.md).

## Optional tools

Mainframe works without these. Each one switches on a feature:

| Tool | What it adds |
|---|---|
| an editor | `e` and `Enter` edit files: `$VISUAL` or `$EDITOR`, else nano (on Windows `edit`, which comes with Windows 11, else Notepad). Or set `editor` in the [configuration](reference/config.md). |
| VS Code (`code`) | `o` and `O` open a file or the whole project. |
| [zoxide](https://github.com/ajeetdsouza/zoxide) | `c` accepts keywords like `c watch`, `Z` opens zoxide's picker. |
| `gio` (GLib) or `trash-put` | Linux: `d` moves files to the trash instead of deleting them. On Windows it's always the Recycle Bin. |
| `notify-send` | Linux: a desktop notification when a long task finishes. Windows shows its own. |
| Visual Studio or its Build Tools | Windows: C and C++ with `cl`, CMake's Visual Studio generator, `.sln`/`.vcxproj` with msbuild. |
| [.NET SDK](https://dotnet.microsoft.com/download) | `.sln`, `.csproj` and `.fsproj` projects. |
| PowerShell 7 (`pwsh`) | Windows: used for your commands instead of Windows PowerShell 5.1. |
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

**PowerShell** (in your `$PROFILE`):

```powershell
function mf {
    $f = New-TemporaryFile
    $env:MAINFRAME_CWD_FILE = $f
    mainframe @args
    if ($LASTEXITCODE -eq 0) { Set-Location (Get-Content -Raw -Encoding utf8 $f) }
    Remove-Item $f, Env:MAINFRAME_CWD_FILE
}
```

Then start it with `mf`.

## Configuration

Press `,` inside Mainframe. It creates `~/.config/mainframe/config.toml`
(`%APPDATA%\mainframe\config.toml` on Windows) with
every option explained and opens it in your editor. See
[Configuration](reference/config.md).
