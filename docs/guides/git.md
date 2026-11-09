# Git

The header always shows the branch, commits to push (↑) and to pull (↓),
and a dot (●) when there are uncommitted changes. The file list marks each
changed file. `g` opens the git menu; the commands run in the output pane.

## The git menu

Press `g`, then one key:

| | Key | What it does |
|---|---|---|
| **Look** | `s` | `git status` |
| | `d` / `D` | diff of unstaged / staged changes |
| | `l` | log of all branches as a graph |
| | `L` | history of the selected file |
| | `w` | blame of the selected file |
| **Stage and commit** | `a` / `A` | stage the selected (or marked) files / everything |
| | `u` | unstage the selected files |
| | `c` | commit what is staged; asks for the message |
| | `C` | stage everything and commit |
| | `e` | amend the last commit in your editor |
| | `x` | throw away the changes to the selected files, after asking |
| **Sync** | `f` | fetch from all remotes |
| | `p` | pull |
| | `P` | push; the first push of a branch sets its upstream |
| | `B` | push another local branch, without switching to it |
| **Branches** | `b` | list branches |
| | `o` | switch to a branch (Tab completes) |
| | `n` | new branch from here |
| | `W` | remove another worktree |
| | `z` / `Z` | stash / pop the stash |
| **Releases** | `r` | make a release, see below |
| | `R` | list releases and CI runs |
| **Other** | `i` | `git init` here |
| | `h` | create a GitHub repository for this folder |
| | `g` | open lazygit, gitui or tig, whichever is installed |

Outside the git menu, `s` and `u` stage and unstage right in the file list.

## Pushing another branch

`g B` pushes a branch you don't have checked out: pick it from the list
(most recently changed first), and Mainframe pushes it to the remote it
tracks, or to `origin` with the upstream set. Your current branch and your
uncommitted changes stay as they are.

## Worktrees

A worktree is a second checkout of the same repository in another folder,
for example to work on two branches at once. Inside a worktree's folder,
everything in Mainframe works on that checkout and its branch. The
[cleanup](cleanup.md) also finds the build output in your other worktrees.

`g W` removes one. Pick it by its folder name; Mainframe asks first and
says which branch it has checked out. The branch itself stays. When the
worktree has uncommitted changes, the question says how many would be lost.
If you're inside the worktree you remove, Mainframe takes you to the main
checkout first.

You can also delete a worktree's folder by hand. Git then still thinks the
branch is checked out there; `g W` on it, or `git worktree prune`, cleans
that up.

## A GitHub repository for a folder

`g h` creates a repository on GitHub with the [GitHub CLI](https://cli.github.com)
and links the folder to it. A form asks for the owner (you or an
organization), the name, a description, the visibility, the remote name and
whether to push. Each field has a default; `Enter` accepts it.

## Releases

`g r` makes a release in one go, for projects with a version in
`Cargo.toml`, `package.json` or `pyproject.toml`. A form asks for:

1. **version:** the next version; it suggests one.
2. **bump:** how the version change gets into `main`:
   - *merge PR*: a pull request that is merged right away
   - *open PR*: a pull request you merge yourself, for example when `main`
     requires reviews or CI runs on pull requests; the next `g r` then
     continues with the tag
   - *commit*: a commit straight on `main`
   - *already done*: the version is already bumped
3. **checks:** commands that must pass before tagging, like
   `cargo build && cargo test && cargo clippy --all-targets`. Empty skips
   them.
4. **remote:** where the commit and the tag go.
5. **then:** when a workflow runs on tags, Mainframe follows it until it's
   done (*watch CI*). Without one, it publishes the release with
   `gh release create --generate-notes`.

The version is bumped in the manifest and its lock file, the tag is
`vX.Y.Z`. `g R` lists the releases and the latest CI runs.
