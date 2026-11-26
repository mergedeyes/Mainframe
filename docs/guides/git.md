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
| **Sync** | `f` | fetch from all remotes; can also delete merged branches, see below |
| | `p` | pull |
| | `P` | push; the first push of a branch sets its upstream |
| | `B` | push another local branch, without switching to it |
| **Branches** | `b` | list branches |
| | `o` | switch to a branch, numbered in the order to merge them, see below |
| | `n` | new branch from here; the name is tidied up for git and GitHub, see below |
| | `W` | remove another worktree |
| | `z` / `Z` | stash / pop the stash |
| **Releases** | `r` | make a release, see below |
| | `R` | list releases and CI runs |
| **Other** | `i` | `git init` here |
| | `h` | create a GitHub repository for this folder |
| | `g` | open lazygit, gitui or tig, whichever is installed |

Outside the git menu, `s` and `u` stage and unstage right in the file list.

## Switching branches, in merge order

`g o` lists every branch, local and on the remote, with two dates: when
the work on it started (its oldest commit that isn't in `origin/main` yet)
and its last commit. Branches with work still to merge are numbered 1, 2,
3 … by when their work started, so merging or opening pull requests in
that order brings them in the way they were written. A branch stacked on
another comes right after it.

Below the numbered ones come the base (`origin/main`, or `main` in a repo
without a remote) and the branches that have nothing it doesn't, such as
merged ones. The last column says how each stands with its remote: not
pushed, pushed, commits to push or pull, only on the remote, or deleted on
the remote. Type to filter, `Enter` switches. When nothing matches,
`Enter` switches to the name you typed.

## New branches

`g n` asks for a name and makes it one git and GitHub accept, so you can
type it the way you'd say it: `feat: Add dark mode (WIP)` becomes
`feat/add-dark-mode-wip`. Letters are lowercased, spaces and characters
git refuses turn into `-`, and a `:` or `/` separates folders. While you
type, the prompt shows `→` with the name the branch will get.

## Branches deleted on the remote

When a pull request is merged, GitHub usually deletes its branch. `g f`
then drops `origin/<branch>` on the next fetch, but your local branch
stays; `g o` shows it as deleted on the remote. To have `g f` delete those
too, set this in the [configuration](../reference/config.md):

```toml
prune_gone_branches = true
```

It only deletes a local branch when everything on it is in `origin/main`,
so no work is lost. That includes branches GitHub rebased or squashed when
merging: their commits have new ids in `main`, but merging the branch again
wouldn't change a file. One whose work isn't in `main`, for example when
its pull request was closed, stays, and the output says so; so does one
that would conflict with `main`. The branch you have checked out stays too.

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
