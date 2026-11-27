# Blog

Mainframe can look after one [Jekyll](https://jekyllrb.com) blog: write
posts in its own Markdown editor, publish them and push them without leaving
the window. It only does
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

`L` lists the blog's [/now page](#the-now-page) on top when it has one,
then the drafts in `_drafts/`, the ones you changed last on top, then the
posts in `_posts/`, newest first. Each row shows whether it's the /now page,
a draft or a post, the post's date (for the /now page, the day it was last
updated), and the title from its front matter (the file name when there is
none).

Letters are commands here, so `/` starts the filter: type, then `Enter`
keeps it, `Esc` clears it.

`Enter` or `e` opens the selected post in the post editor (below). `E`
opens it in your own editor (`editor` in the config) instead; when you
close that, the picker comes back with the same file selected.

## Writing a post

`a` asks for:

- **title**: required; it also makes the file name, lowercase with `-`
  between words, umlauts spelled out (`Größe` → `groesse`)
- **description**: optional, one line
- **save as**: `draft` writes `_drafts/<name>.md`, `post now` writes
  `_posts/<today>-<name>.md`

Mainframe never replaces a file that's already there. The new file starts
with its front matter and opens in the post editor, ready for the text:

```yaml
---
layout: post
title: "Größe zählt"
description: "Warum Maße wichtig sind"
---
```

## The post editor

```
╭ draft _drafts/groesse-zaehlt.md ──────────────────────────────────────────╮
│ title       Größe zählt                                                    │
│ description Warum Maße wichtig sind                                        │
│────────────────────────────────────────────────────────────────────────────│
│ # Einleitung                          │ Größe zählt                        │
│                                       │ Warum Maße wichtig sind            │
│ Heute geht es um **Maße**.            │                                    │
│                                       │ Einleitung                         │
│ - eins                                │                                    │
│ - zwei                                │ Heute geht es um Maße.             │
│                                       │                                    │
│                                       │ • eins                             │
│                                       │ • zwei                             │
│ 7 words · saved                                                            │
╰ ^S save · ^G publish · ^P preview · ^Z ^Y undo · ^B bold · … · Esc close ──╯
```

Title and description are fields at the top; Mainframe writes them into the
front matter and leaves every other front matter line (`tags:`, `image:` …)
as it is. Below them is the Markdown, highlighted as you type, with long
lines wrapped. On the right, in windows at least 90 columns wide, a preview
shows the post roughly as it will read, and follows the part you're
writing. `Ctrl-P` turns it off.

Some help with Markdown:

- `Enter` in a list continues it (`- `, `1. ` counting up, `- [ ] `), and on
  an empty item ends it; quotes (`> `) continue the same way
- `Tab` and Shift-Tab indent and outdent list items
- `Ctrl-B` makes the word at the cursor `**bold**`, `Ctrl-E` `_italic_`,
  `Ctrl-K` a link with the cursor where the address goes
- pasting puts the text in as it is, without continuing lists

There's nothing to save by hand: a moment after you stop typing, and when
you close the editor with `Esc`, the post is written. `Ctrl-S` saves right
away. If another program changes the file while it's open, Mainframe stops
saving and says so: `Ctrl-R` loads the file as it is now, `Ctrl-S` keeps
your version.

`Ctrl-Z` and `Ctrl-Y` undo and redo, a word at a time. All keys are in the
[key reference](../reference/keys.md#post-editor).

`Ctrl-G` takes the post live from the editor: on a draft it publishes it
like `p` (asking first), on a post it commits and pushes your changes like
`P`.

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

## The /now page

A [/now page](https://nownownow.com/about) says what you're up to at the
moment. It isn't a post: it's one page, `now.md` at the top of the blog,
that you rewrite whenever that changes, with the day of the last change in
its front matter as `updated:`.

`n` opens it in the post editor. When the blog has none yet, `n` writes one
first:

```yaml
---
layout: now
title: "Now"
description: "What I'm focused on at the moment."
permalink: /now/
updated: 2026-11-27
---
```

The layout is `now` when the blog has `_layouts/now.html`, and `page`
otherwise, which Jekyll themes have. Show `page.updated` in your layout to
tell readers how current the page is.

`P` in the picker, or `Ctrl-G` in the editor, pushes it: when `now.md` has
changes, Mainframe sets `updated:` to today, commits only `now.md` with
`now: update` (`now: add the /now page` the first time), and pushes. `p` and
`u` don't apply to it, since it has no draft; `d` trashes it like a post.

## Following the GitHub Pages build

With the [GitHub CLI](https://cli.github.com) (`gh`) installed and `origin`
on GitHub, `p` and `P` wait for the Pages build of the commit they pushed,
checking every five seconds for up to three minutes. Then they print and
send a desktop notification: `post live:` with the post's address, or the
build error. For the /now page, that's `now page live:`.

The address follows Jekyll's default permalink `/:year/:month/:day/:title/`
(for the /now page, its `permalink:`, else `/now/`),
on the domain in the `CNAME` file, or else `<owner>.github.io/<repo>`.
Pages only builds the branch it's set up for, so pushing another branch
ends with "no finished Pages build".
