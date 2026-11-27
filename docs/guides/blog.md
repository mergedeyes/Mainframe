# Blog

Mainframe can look after one [Jekyll](https://jekyllrb.com) blog: write
posts, publish them and push them without leaving the window. It only does
this in the blog's own repo, so nothing changes anywhere else.

## Setting it up

Press `,` and set `blog_repo` to the blog's folder:

```toml
blog_repo = "~/Projekte/blog"
```

From then on, in that folder, anywhere below it, or in a worktree of the
same repo, `L` opens the blog picker and the footer shows `L blog`. In every
other folder `L` does nothing.

## The picker

`L` lists the drafts in `_drafts/` first, the ones you changed last on top,
then the posts in `_posts/`, newest first. Each row shows whether it's a
draft or a post, the post's date, and the title from its front matter (the
file name when there is none).

Letters are commands here, so `/` starts the filter: type, then `Enter`
keeps it, `Esc` clears it.

`Enter` or `e` opens the selected file in your editor. When you close the
editor, the picker comes back with the same file selected.

## Writing a post

`a` asks for:

- **title**: required; it also makes the file name, lowercase with `-`
  between words, umlauts spelled out (`Größe` → `groesse`)
- **description**: optional, one line
- **save as**: `draft` writes `_drafts/<name>.md`, `post now` writes
  `_posts/<today>-<name>.md`

Mainframe never replaces a file that's already there. The new file starts
with its front matter and opens in your editor:

```yaml
---
layout: post
title: "Größe zählt"
description: "Warum Maße wichtig sind"
---
```

## Publishing

`p` on a draft moves it to `_posts/` with today's date in front, commits
`_drafts` and `_posts` with `post: <title>`, and pushes. It asks first and
shows the new file name and the commit message. The commands run in the
output pane, like everything else.

`P` on a post commits and pushes your changes to it, with
`post: update <title>`. It only does that when the file has changes.

`u` moves a post back to `_drafts/`, without the date, and doesn't commit:
`p` publishes it again later with that day's date.

`d` moves a draft or a post to the trash, after asking.

## Following the GitHub Pages build

With the [GitHub CLI](https://cli.github.com) (`gh`) installed and `origin`
on GitHub, `p` and `P` wait for the Pages build of the commit they pushed,
checking every five seconds for up to three minutes. Then they print and
send a desktop notification: `post live:` with the post's address, or the
build error.

The address follows Jekyll's default permalink `/:year/:month/:day/:title/`,
on the domain in the `CNAME` file, or else `<owner>.github.io/<repo>`.
Pages only builds the branch it's set up for, so pushing another branch
ends with "no finished Pages build".
