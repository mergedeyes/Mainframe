//! A Jekyll blog in the repo `blog_repo` names: its drafts and posts, new posts, moving them
//! between `_drafts/` and `_posts/`, and the /now page. Only the files; the `L` picker is in
//! `app::blog_picker`.

use std::{
    fs, io,
    path::{Path, PathBuf},
    time::SystemTime,
};

use crate::{
    config,
    git::{git_lines, git_text},
    project::{quote_str, which},
};

pub const DRAFTS: &str = "_drafts";
pub const POSTS: &str = "_posts";
/// The /now page (https://nownownow.com/about), at the top of the blog: one page rewritten in
/// place whenever what you're up to changes, with the day of the last change in `updated:`.
pub const NOW: &str = "now.md";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Draft,
    Post,
    /// The /now page, `now.md`.
    Now,
}

#[derive(Debug, Clone)]
pub struct Post {
    pub path: PathBuf,
    pub state: State,
    /// `YYYY-MM-DD`: a post's from the file name, the /now page's from `updated:`.
    pub date: Option<String>,
    /// The file name without date and `.md`: Jekyll's `:title`.
    pub slug: String,
    /// From the front matter, else the file name.
    pub title: String,
}

impl Post {
    /// `_posts/2026-11-27-hello.md`, for messages.
    pub fn rel(&self) -> String {
        let dir = match self.state {
            State::Draft => DRAFTS,
            State::Post => POSTS,
            State::Now => return NOW.into(),
        };
        format!("{dir}/{}", config::dir_name(&self.path))
    }
}

/// The worktree to work in when `cwd` is inside the blog repo `setting` names (or in a
/// worktree of it); None when it isn't, or when no blog repo is set.
pub fn root(cwd: &Path, setting: &str) -> Option<PathBuf> {
    let setting = setting.trim();
    if setting.is_empty() {
        return None;
    }
    let blog = crate::platform::canonicalize(&config::expand_tilde(setting)).ok()?;
    let cwd = crate::platform::canonicalize(cwd).ok()?;
    if cwd.starts_with(&blog) {
        return Some(blog);
    }
    // Another worktree shares the blog's git dir. Only when the blog folder is the repo's
    // top, so a blog inside, say, a dotfiles repo at $HOME doesn't claim all of $HOME.
    let top = |d: &Path| {
        git_text(d, &["rev-parse", "--show-toplevel"]).and_then(|t| crate::platform::canonicalize(Path::new(&t)).ok())
    };
    let common = |d: &Path| {
        git_text(d, &["rev-parse", "--path-format=absolute", "--git-common-dir"])
            .and_then(|t| crate::platform::canonicalize(Path::new(&t)).ok())
    };
    let here = top(&cwd)?;
    (top(&blog)? == blog && common(&cwd)? == common(&blog)?).then_some(here)
}

/// The /now page when there is one, then drafts, most recently changed first, then posts,
/// newest first.
pub fn list(root: &Path) -> Vec<Post> {
    let now = Some(root.join(NOW)).filter(|p| p.is_file()).and_then(|p| post(&p));
    let mut drafts: Vec<(SystemTime, Post)> = md_files(&root.join(DRAFTS))
        .iter()
        .filter_map(|path| {
            let changed = fs::metadata(path).and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
            Some((changed, post(path)?))
        })
        .collect();
    drafts.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.slug.cmp(&b.1.slug)));
    let mut posts: Vec<Post> = md_files(&root.join(POSTS)).iter().filter_map(|p| post(p)).collect();
    posts.sort_by(|a, b| b.date.cmp(&a.date).then_with(|| b.slug.cmp(&a.slug)));
    now.into_iter().chain(drafts.into_iter().map(|(_, p)| p)).chain(posts).collect()
}

/// The post, draft or /now page at `path`, as `list` would show it.
pub fn post(path: &Path) -> Option<Post> {
    let stem = path.file_stem()?.to_str()?.to_string();
    let dir = path.parent()?.file_name()?;
    let title = |fallback: &str| read_title(path).unwrap_or_else(|| fallback.to_string());
    if dir == DRAFTS {
        Some(Post { path: path.to_path_buf(), state: State::Draft, date: None, title: title(&stem), slug: stem })
    } else if dir == POSTS {
        let (date, slug) = split_date(&stem)?;
        let (date, slug) = (date.to_string(), slug.to_string());
        Some(Post { path: path.to_path_buf(), state: State::Post, date: Some(date), title: title(&slug), slug })
    } else if path.file_name()? == NOW {
        let date = fs::read_to_string(path).ok().and_then(|t| front_value(&t, "updated"));
        Some(Post { path: path.to_path_buf(), state: State::Now, date, title: title("Now"), slug: stem })
    } else {
        None
    }
}

/// Every `*.md` file directly in `dir`.
fn md_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else { return Vec::new() };
    entries.flatten().map(|e| e.path()).filter(|p| p.is_file() && p.extension().is_some_and(|x| x == "md")).collect()
}

/// `2026-11-27-hello` → `("2026-11-27", "hello")`; None without a valid date in front.
fn split_date(stem: &str) -> Option<(&str, &str)> {
    let (date, rest) = (stem.get(..10)?, stem.get(10..)?);
    let slug = rest.strip_prefix('-').filter(|s| !s.is_empty())?;
    let b = date.as_bytes();
    let digits = |r: std::ops::Range<usize>| b[r].iter().all(u8::is_ascii_digit);
    (digits(0..4) && b[4] == b'-' && digits(5..7) && b[7] == b'-' && digits(8..10)).then_some((date, slug))
}

fn read_title(path: &Path) -> Option<String> {
    title_of(&fs::read_to_string(path).ok()?)
}

/// `title` from the YAML front matter at the top of a post.
pub fn title_of(text: &str) -> Option<String> {
    front_value(text, "title")
}

/// A one-line `key:` from the YAML front matter at the top of a file; None when it's
/// missing or empty.
fn front_value(text: &str, key: &str) -> Option<String> {
    let mut lines = text.lines();
    if lines.next()?.trim_end() != "---" {
        return None;
    }
    for line in lines {
        if line.trim_end() == "---" {
            break;
        }
        if let Some(value) = line.strip_prefix(key).and_then(|rest| rest.strip_prefix(':')) {
            let value = unquote(value.trim());
            return (!value.is_empty()).then_some(value);
        }
    }
    None
}

/// A YAML scalar as written on one line: `"…"` with backslash escapes, `'…'` with `''`
/// for a quote, or plain (a ` #` starts a comment).
fn unquote(v: &str) -> String {
    if let Some(inner) = v.strip_prefix('"') {
        let mut out = String::new();
        let mut chars = inner.chars();
        while let Some(c) = chars.next() {
            match c {
                '"' => break,
                '\\' => match chars.next() {
                    Some('n') => out.push('\n'),
                    Some('t') => out.push('\t'),
                    Some(other) => out.push(other),
                    None => break,
                },
                c => out.push(c),
            }
        }
        out
    } else if let Some(inner) = v.strip_prefix('\'') {
        let mut out = String::new();
        let mut chars = inner.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\'' {
                if chars.peek() == Some(&'\'') {
                    chars.next();
                } else {
                    break;
                }
            }
            out.push(c);
        }
        out
    } else {
        v.split(" #").next().unwrap_or(v).trim().to_string()
    }
}

/// A post split for the editor: title, description and body to edit, the other front
/// matter kept as it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Doc {
    front: Vec<Front>,
    pub title: String,
    pub description: String,
    pub body: String,
}

/// A front matter line. Title and description remember how they were written, so they're
/// written back unchanged unless they were edited.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Front {
    Title { raw: String, value: String },
    Description { raw: String, value: String },
    Other(String),
}

impl Doc {
    pub fn parse(text: &str) -> Self {
        let mut doc = Doc { front: Vec::new(), title: String::new(), description: String::new(), body: String::new() };
        let lines: Vec<&str> = text.split('\n').collect();
        let close = (lines.first().map(|l| l.trim_end()) == Some("---"))
            .then(|| lines.iter().skip(1).position(|l| l.trim_end() == "---"))
            .flatten();
        let Some(close) = close.map(|c| c + 1) else {
            doc.front.push(Front::Other("layout: post".into()));
            doc.body = text.to_string();
            return doc;
        };
        for line in &lines[1..close] {
            let raw = line.to_string();
            if let Some(v) = line.strip_prefix("title:") {
                doc.title = unquote(v.trim());
                doc.front.push(Front::Title { raw, value: doc.title.clone() });
            } else if let Some(v) = line.strip_prefix("description:") {
                doc.description = unquote(v.trim());
                doc.front.push(Front::Description { raw, value: doc.description.clone() });
            } else {
                doc.front.push(Front::Other(raw));
            }
        }
        let body = lines[close + 1..].join("\n");
        // The empty line after the front matter is part of the layout, not of the text.
        doc.body = body.strip_prefix('\n').map(str::to_string).unwrap_or(body);
        doc
    }

    pub fn render(&self) -> String {
        let edited = |raw: &str, was: &str, key: &str, now: &str| {
            if was == now { raw.to_string() } else { format!("{key}: {}", yaml_str(now)) }
        };
        let mut lines = Vec::new();
        let (mut title_at, mut had_desc) = (None, false);
        for f in &self.front {
            match f {
                Front::Title { raw, value } => {
                    title_at = Some(lines.len());
                    lines.push(edited(raw, value, "title", &self.title));
                }
                Front::Description { raw, value } => {
                    had_desc = true;
                    if !self.description.is_empty() {
                        lines.push(edited(raw, value, "description", &self.description));
                    }
                }
                Front::Other(l) => lines.push(l.clone()),
            }
        }
        // A missing title goes below the layout, a new description below the title.
        let title_at = title_at.unwrap_or_else(|| {
            let at = lines.iter().position(|l| l.starts_with("layout:")).map_or(0, |i| i + 1);
            lines.insert(at, format!("title: {}", yaml_str(&self.title)));
            at
        });
        if !had_desc && !self.description.is_empty() {
            lines.insert(title_at + 1, format!("description: {}", yaml_str(&self.description)));
        }
        format!("---\n{}\n---\n\n{}", lines.join("\n"), self.body)
    }
}

/// A double-quoted YAML string.
pub fn yaml_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// The front matter of a new post.
pub fn front_matter(title: &str, description: &str) -> String {
    let mut s = format!("---\nlayout: post\ntitle: {}\n", yaml_str(title));
    if !description.is_empty() {
        s.push_str(&format!("description: {}\n", yaml_str(description)));
    }
    s.push_str("---\n\n");
    s
}

/// The file name part for a title: lowercase ASCII words joined by `-`, umlauts spelled
/// out (`ä` → `ae`) and accents dropped (`é` → `e`), like `git::branch_name` tidies.
pub fn slug(title: &str) -> String {
    let mut out = String::new();
    for c in title.to_lowercase().chars() {
        let spelled = match c {
            'ä' | 'æ' => "ae",
            'ö' | 'œ' => "oe",
            'ü' => "ue",
            'ß' => "ss",
            'à' | 'á' | 'â' | 'ã' | 'å' => "a",
            'ç' => "c",
            'è' | 'é' | 'ê' | 'ë' => "e",
            'ì' | 'í' | 'î' | 'ï' => "i",
            'ñ' => "n",
            'ò' | 'ó' | 'ô' | 'õ' | 'ø' => "o",
            'ù' | 'ú' | 'û' => "u",
            'ý' | 'ÿ' => "y",
            c if c.is_ascii_alphanumeric() => {
                out.push(c);
                continue;
            }
            _ => "-",
        };
        // Runs of separators become one.
        if !(spelled == "-" && (out.is_empty() || out.ends_with('-'))) {
            out.push_str(spelled);
        }
    }
    out.trim_end_matches('-').to_string()
}

/// Today in local time, as `YYYY-MM-DD`.
pub fn today() -> String {
    crate::platform::today()
}

/// Where a new post with this title goes: `_drafts/<slug>.md` or `_posts/<today>-<slug>.md`.
pub fn new_path(root: &Path, title: &str, draft: bool, today: &str) -> Result<PathBuf, String> {
    let slug = slug(title);
    if slug.is_empty() {
        return Err(if title.trim().is_empty() {
            "a post needs a title".into()
        } else {
            "the title has no letters or digits for a file name".into()
        });
    }
    Ok(if draft {
        root.join(DRAFTS).join(format!("{slug}.md"))
    } else {
        root.join(POSTS).join(format!("{today}-{slug}.md"))
    })
}

/// Writes a new post with its front matter; never over an existing file.
pub fn create(root: &Path, title: &str, description: &str, draft: bool, today: &str) -> Result<PathBuf, String> {
    let title = title.trim();
    let path = new_path(root, title, draft, today)?;
    let text = front_matter(title, description.trim());
    let write = || -> io::Result<()> {
        fs::create_dir_all(path.parent().expect("always in a folder"))?;
        io::Write::write_all(&mut fs::OpenOptions::new().write(true).create_new(true).open(&path)?, text.as_bytes())
    };
    match write() {
        Ok(()) => Ok(path),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Err(format!("{} already exists", rel(root, &path))),
        Err(e) => Err(format!("can't write {}: {e}", rel(root, &path))),
    }
}

/// Where publishing a draft today puts it.
pub fn publish_target(root: &Path, draft: &Post, today: &str) -> PathBuf {
    root.join(POSTS).join(format!("{today}-{}.md", draft.slug))
}

/// Where unpublishing a post puts it.
pub fn unpublish_target(root: &Path, post: &Post) -> PathBuf {
    root.join(DRAFTS).join(format!("{}.md", post.slug))
}

/// `_drafts/<slug>.md` → `_posts/<today>-<slug>.md`.
pub fn publish(root: &Path, draft: &Post, today: &str) -> Result<PathBuf, String> {
    if draft.state != State::Draft {
        return Err(format!("{} is already published", draft.rel()));
    }
    let to = publish_target(root, draft, today);
    move_to(root, &draft.path, &to)?;
    Ok(to)
}

/// `_posts/<date>-<slug>.md` → `_drafts/<slug>.md`.
pub fn unpublish(root: &Path, post: &Post) -> Result<PathBuf, String> {
    if post.state != State::Post {
        return Err(format!("{} is a draft already", post.rel()));
    }
    let to = unpublish_target(root, post);
    move_to(root, &post.path, &to)?;
    Ok(to)
}

/// Writes `now.md` from a starting template when the blog has none; never over an existing
/// file. Its layout is the blog's own `now` layout, else `page`, which Jekyll themes have.
pub fn create_now(root: &Path, today: &str) -> Result<PathBuf, String> {
    let path = root.join(NOW);
    let layout = if root.join("_layouts/now.html").is_file() { "now" } else { "page" };
    let text = format!(
        "---\nlayout: {layout}\ntitle: \"Now\"\ndescription: \"What I'm focused on at the moment.\"\n\
         permalink: /now/\nupdated: {today}\n---\n\n- \n"
    );
    let write =
        || io::Write::write_all(&mut fs::OpenOptions::new().write(true).create_new(true).open(&path)?, text.as_bytes());
    match write() {
        Ok(()) => Ok(path),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Err(format!("{NOW} already exists")),
        Err(e) => Err(format!("can't write {NOW}: {e}")),
    }
}

/// Sets `updated:` in the front matter of the file at `path` to `today`, or adds it at the
/// end of the front matter. The /now page shows it as the day it last changed.
pub fn stamp_updated(path: &Path, today: &str) -> Result<(), String> {
    let name = config::dir_name(path);
    let text = fs::read_to_string(path).map_err(|e| format!("can't read {name}: {e}"))?;
    let stamped = with_updated(&text, today).ok_or_else(|| format!("{name} has no front matter"))?;
    if stamped != text {
        fs::write(path, stamped).map_err(|e| format!("can't write {name}: {e}"))?;
    }
    Ok(())
}

fn with_updated(text: &str, today: &str) -> Option<String> {
    let mut lines: Vec<String> = text.split('\n').map(str::to_string).collect();
    if lines.first()?.trim_end() != "---" {
        return None;
    }
    let close = lines.iter().skip(1).position(|l| l.trim_end() == "---")? + 1;
    let line = format!("updated: {today}");
    match lines[1..close].iter().position(|l| l.starts_with("updated:")) {
        Some(i) => lines[i + 1] = line,
        None => lines.insert(close, line),
    }
    Some(lines.join("\n"))
}

fn move_to(root: &Path, from: &Path, to: &Path) -> Result<(), String> {
    if to.exists() {
        return Err(format!("{} already exists", rel(root, to)));
    }
    let res = fs::create_dir_all(to.parent().expect("always in a folder")).and_then(|_| fs::rename(from, to));
    res.map_err(|e| format!("can't move {} to {}: {e}", rel(root, from), rel(root, to)))
}

/// `path` relative to the blog, for messages.
/// `path` inside the blog repo, with `/` like git and Jekyll write it, also on Windows.
pub fn rel(root: &Path, path: &Path) -> String {
    match path.strip_prefix(root) {
        Ok(rel) => rel.iter().map(|c| c.to_string_lossy()).collect::<Vec<_>>().join("/"),
        Err(_) => path.display().to_string(),
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Change {
    None,
    /// Not committed yet.
    New,
    Edited,
}

/// What git sees of `path`.
pub fn change(root: &Path, path: &Path) -> Change {
    let p = path.to_string_lossy();
    match git_lines(root, &["status", "--porcelain", "--", &p]).first() {
        None => Change::None,
        Some(l) if l.starts_with("??") || l.starts_with('A') => Change::New,
        Some(_) => Change::Edited,
    }
}

/// Commits `_drafts` and `_posts` as they are with `message` and pushes.
pub fn commit_cmd(root: &Path, message: &str) -> String {
    let dirs: Vec<&str> = [DRAFTS, POSTS].into_iter().filter(|d| root.join(d).is_dir()).collect();
    format!("git add -A {} && git commit -m {} && git push", dirs.join(" "), quote_str(message))
}

/// Commits only the /now page with `message` and pushes.
pub fn commit_now_cmd(message: &str) -> String {
    format!("git add -A -- {NOW} && git commit -m {} && git push", quote_str(message))
}

/// Shell steps that follow the GitHub Pages build of the pushed commit, then print and
/// notify "post live: <url>" (or "now page live: …") or what went wrong. None without `gh`
/// or a GitHub origin.
pub fn follow_pages_cmd(root: &Path, post_path: &Path) -> Option<String> {
    which("gh")?;
    let origin = git_text(root, &["remote", "get-url", "origin"])?;
    let repo = crate::release::github_repo(&origin)?;
    let what = if post(post_path)?.state == State::Now { "now page" } else { "post" };
    Some(follow_pages_script(&repo, &format!("{what} live: {}", post_url(root, &repo, post_path)?)))
}

/// The polling loop behind [`follow_pages_cmd`], in braces so it joins a `&&` chain as one
/// step. About three minutes, every five seconds: the build for a push shows up after a while.
/// `live` is the message once it's built.
fn follow_pages_script(repo: &str, live: &str) -> String {
    let api = format!("gh api repos/{repo}/pages/builds/latest");
    let live = quote_str(live);
    format!(
        "{{ echo 'waiting for the GitHub Pages build …'; head=$(git rev-parse HEAD); s=; \
         for i in $(seq 36); do sleep 5; r=$({api} -q '.commit + \" \" + .status' 2>/dev/null); \
         case \"$r\" in \"$head built\") s=built; break;; \"$head errored\") s=errored; break;; esac; done; \
         case $s in built) msg={live};; \
         errored) msg=\"Pages build failed: $({api} -q .error.message 2>/dev/null)\";; \
         *) msg='no finished Pages build for this commit after 3 minutes (does Pages build this branch?)';; esac; \
         echo \"$msg\"; command -v notify-send >/dev/null && notify-send -a Mainframe \"$msg\"; [ \"$s\" = built ]; }}"
    )
}

/// The address of a post, by Jekyll's default permalink `/:year/:month/:day/:title/`, or of
/// the /now page, by its `permalink:` (else `/now/`), on the domain in `CNAME` or else the
/// repo's github.io address.
pub fn post_url(root: &Path, repo: &str, post_path: &Path) -> Option<String> {
    let path = if post(post_path)?.state == State::Now {
        let permalink = fs::read_to_string(post_path).ok().and_then(|t| front_value(&t, "permalink"));
        let permalink = permalink.unwrap_or_else(|| "/now/".into());
        format!("/{}", permalink.trim_start_matches('/'))
    } else {
        let (date, slug) = split_date(post_path.file_stem()?.to_str()?)?;
        format!("/{}/{slug}/", date.replace('-', "/"))
    };
    let base = match fs::read_to_string(root.join("CNAME")).ok().map(|c| c.trim().to_string()).filter(|c| !c.is_empty())
    {
        Some(domain) => format!("https://{}", domain.trim_end_matches('/')),
        None => {
            let (owner, name) = repo.split_once('/')?;
            if name.eq_ignore_ascii_case(&format!("{owner}.github.io")) {
                format!("https://{}", name.to_lowercase())
            } else {
                format!("https://{}.github.io/{name}", owner.to_lowercase())
            }
        }
    };
    Some(format!("{base}{path}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{git as test_git, temp_dir};

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    #[test]
    fn slugs() {
        assert_eq!(slug("Hello, World!"), "hello-world");
        assert_eq!(slug("Über Größe & Maße"), "ueber-groesse-masse");
        assert_eq!(slug("ÄÖÜ äöü ß"), "aeoeue-aeoeue-ss");
        assert_eq!(slug("  Café crème -- Rust 2024  "), "cafe-creme-rust-2024");
        assert_eq!(slug("what's new? (v1.2)"), "what-s-new-v1-2");
        assert_eq!(slug("日本語"), "");
        assert_eq!(slug("--"), "");
    }

    #[test]
    fn front_matter_escapes_and_parses_back() {
        let fm = front_matter(r#"Say "hi" \o/"#, "");
        assert_eq!(fm, "---\nlayout: post\ntitle: \"Say \\\"hi\\\" \\\\o/\"\n---\n\n");
        assert_eq!(title_of(&fm).as_deref(), Some(r#"Say "hi" \o/"#));
        let with_desc = front_matter("T", "a \"short\" one");
        assert!(with_desc.contains("description: \"a \\\"short\\\" one\"\n"));

        assert_eq!(title_of("---\ntitle: 'It''s here'\n---\n").as_deref(), Some("It's here"));
        assert_eq!(title_of("---\nlayout: post\ntitle: Plain words # note\n---\n").as_deref(), Some("Plain words"));
        assert_eq!(title_of("---\nlayout: post\n---\ntitle: body text\n"), None);
        assert_eq!(title_of("no front matter\ntitle: x\n"), None);
        assert_eq!(title_of("---\ntitle: \"\"\n---\n"), None);
    }

    #[test]
    fn lists_drafts_then_posts_newest_first() {
        let root = temp_dir("blog-list");
        write(&root.join("_drafts/older.md"), "---\ntitle: \"Older draft\"\n---\n");
        write(&root.join("_drafts/newer.md"), "no front matter");
        let old = SystemTime::now() - std::time::Duration::from_secs(3600);
        fs::File::options().write(true).open(root.join("_drafts/older.md")).unwrap().set_modified(old).unwrap();
        write(&root.join("_posts/2025-01-02-second.md"), "---\ntitle: Second\n---\n");
        write(&root.join("_posts/2026-03-01-third.md"), "---\ntitle: Third\n---\n");
        write(&root.join("_posts/2024-12-31-first.md"), "");
        write(&root.join("_posts/notes.md"), "not a post: no date");
        write(&root.join("_posts/2026-03-01-x.txt"), "not markdown");

        let posts = list(&root);
        let shown: Vec<(State, Option<&str>, &str)> =
            posts.iter().map(|p| (p.state, p.date.as_deref(), p.title.as_str())).collect();
        assert_eq!(
            shown,
            vec![
                (State::Draft, None, "newer"),
                (State::Draft, None, "Older draft"),
                (State::Post, Some("2026-03-01"), "Third"),
                (State::Post, Some("2025-01-02"), "Second"),
                (State::Post, Some("2024-12-31"), "first"),
            ]
        );
        assert_eq!(posts[2].slug, "third");
        assert_eq!(posts[2].rel(), "_posts/2026-03-01-third.md");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn creates_without_overwriting() {
        let root = temp_dir("blog-create");
        let draft = create(&root, " Hallo Welt ", "", true, "2026-11-27").unwrap();
        assert_eq!(draft, root.join("_drafts/hallo-welt.md"));
        assert_eq!(fs::read_to_string(&draft).unwrap(), "---\nlayout: post\ntitle: \"Hallo Welt\"\n---\n\n");

        fs::write(&draft, "my words").unwrap();
        assert_eq!(
            create(&root, "Hallo, Welt!", "", true, "2026-11-27").unwrap_err(),
            "_drafts/hallo-welt.md already exists"
        );
        assert_eq!(fs::read_to_string(&draft).unwrap(), "my words");

        let post = create(&root, "Hallo Welt", "kurz", false, "2026-11-27").unwrap();
        assert_eq!(post, root.join("_posts/2026-11-27-hallo-welt.md"));
        assert!(fs::read_to_string(&post).unwrap().contains("description: \"kurz\"\n"));

        assert!(create(&root, "  ", "", true, "2026-11-27").is_err());
        assert!(create(&root, "!!!", "", true, "2026-11-27").is_err());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn publish_and_unpublish_move_the_file() {
        let root = temp_dir("blog-move");
        create(&root, "Moving On", "", true, "2026-11-27").unwrap();
        let draft = list(&root).remove(0);

        let posted = publish(&root, &draft, "2026-11-28").unwrap();
        assert_eq!(posted, root.join("_posts/2026-11-28-moving-on.md"));
        assert!(!draft.path.exists());
        let post = list(&root).remove(0);
        assert_eq!((post.state, post.title.as_str()), (State::Post, "Moving On"));
        assert!(publish(&root, &post, "2026-11-28").is_err());

        let back = unpublish(&root, &post).unwrap();
        assert_eq!(back, root.join("_drafts/moving-on.md"));
        assert!(!posted.exists());

        // Neither direction replaces a file that's already there.
        let draft = list(&root).remove(0);
        write(&root.join("_posts/2026-11-28-moving-on.md"), "other");
        assert_eq!(publish(&root, &draft, "2026-11-28").unwrap_err(), "_posts/2026-11-28-moving-on.md already exists");
        assert!(draft.path.exists());
        let post = list(&root).into_iter().find(|p| p.state == State::Post).unwrap();
        assert!(unpublish(&root, &post).unwrap_err().ends_with("already exists"));
        assert_eq!(fs::read_to_string(&draft.path).unwrap(), "---\nlayout: post\ntitle: \"Moving On\"\n---\n\n");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn active_only_in_the_blog_repo_and_its_worktrees() {
        let base = temp_dir("blog-root");
        let (blog, other, wt) = (base.join("blog"), base.join("other"), base.join("blog-wt"));
        fs::create_dir_all(blog.join("_posts")).unwrap();
        fs::create_dir_all(&other).unwrap();
        test_git(&blog, &["init", "-q", "-b", "main"]);
        test_git(&blog, &["commit", "-q", "--allow-empty", "-m", "one"]);
        test_git(&blog, &["worktree", "add", "-q", "-b", "draft", wt.to_str().unwrap()]);
        test_git(&other, &["init", "-q"]);
        let blog = crate::platform::canonicalize(&blog).unwrap();
        let setting = blog.to_str().unwrap();

        assert_eq!(root(&blog, setting), Some(blog.clone()));
        assert_eq!(root(&blog.join("_posts"), setting), Some(blog.clone()));
        assert_eq!(root(&blog.join("_posts"), &format!("{setting}/")), Some(blog.clone()));
        assert_eq!(root(&wt, setting), Some(crate::platform::canonicalize(&wt).unwrap()));
        assert_eq!(root(&other, setting), None);
        assert_eq!(root(&base, setting), None);
        assert_eq!(root(&blog, ""), None);
        assert_eq!(root(&blog, &base.join("missing").to_string_lossy()), None);
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn post_urls() {
        let root = temp_dir("blog-url");
        let post = root.join("_posts/2026-11-27-hallo-welt.md");
        assert_eq!(
            post_url(&root, "Jan/blog", &post).as_deref(),
            Some("https://jan.github.io/blog/2026/11/27/hallo-welt/")
        );
        assert_eq!(
            post_url(&root, "jan/jan.github.io", &post).as_deref(),
            Some("https://jan.github.io/2026/11/27/hallo-welt/")
        );
        fs::write(root.join("CNAME"), "blog.example.org\n").unwrap();
        assert_eq!(
            post_url(&root, "jan/blog", &post).as_deref(),
            Some("https://blog.example.org/2026/11/27/hallo-welt/")
        );
        assert_eq!(post_url(&root, "jan/blog", &root.join("_drafts/x.md")), None);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn commits_only_the_folders_that_exist() {
        let root = temp_dir("blog-commit");
        fs::create_dir_all(root.join("_posts")).unwrap();
        assert_eq!(
            commit_cmd(&root, "post: update It's"),
            "git add -A _posts && git commit -m 'post: update It'\\''s' && git push"
        );
        fs::create_dir_all(root.join("_drafts")).unwrap();
        assert!(commit_cmd(&root, "x").starts_with("git add -A _drafts _posts && "));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn sees_new_and_edited_posts() {
        let root = temp_dir("blog-change");
        test_git(&root, &["init", "-q"]);
        let post = create(&root, "One", "", false, "2026-11-27").unwrap();
        assert_eq!(change(&root, &post), Change::New);
        test_git(&root, &["add", "-A"]);
        test_git(&root, &["commit", "-q", "-m", "one"]);
        assert_eq!(change(&root, &post), Change::None);
        fs::write(&post, "changed").unwrap();
        assert_eq!(change(&root, &post), Change::Edited);
        fs::remove_dir_all(&root).unwrap();
    }

    /// The loop with stand-ins for `gh` and `sleep`: it waits for the build of HEAD, not an
    /// older one, and says how it ended; in a `&&` chain it only runs after the push worked.
    // Runs a sh script; Windows gets these once the script is Rust.
    #[cfg(unix)]
    #[test]
    fn follows_the_pages_build() {
        let dir = temp_dir("blog-pages");
        test_git(&dir, &["init", "-q"]);
        test_git(&dir, &["commit", "-q", "--allow-empty", "-m", "one"]);
        let head = test_git(&dir, &["rev-parse", "HEAD"]).trim().to_string();
        let bin = dir.join("bin");
        crate::testutil::stub(&bin, "sleep", "#!/bin/sh\n");
        crate::testutil::stub(&bin, "notify-send", "#!/bin/sh\n");
        let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
        let run = |gh: &str, before: &str| {
            crate::testutil::stub(&bin, "gh", gh);
            let script =
                format!("{before}{}", follow_pages_script("me/blog", "post live: https://me.github.io/blog/x/"));
            let out = std::process::Command::new("sh")
                .args(["-c", &script])
                .current_dir(&dir)
                .env("PATH", &path)
                .output()
                .unwrap();
            let text = String::from_utf8_lossy(&out.stdout).into_owned();
            (out.status.success(), text.lines().last().unwrap_or_default().to_string())
        };
        // An older build first, then this commit's.
        let gh = format!(
            "#!/bin/sh\nn=$(cat n 2>/dev/null || echo 0); echo $((n+1)) > n\n\
             [ $n -lt 2 ] && echo 'old built' || echo '{head} built'\n"
        );
        assert_eq!(run(&gh, ""), (true, "post live: https://me.github.io/blog/x/".into()));
        assert_eq!(fs::read_to_string(dir.join("n")).unwrap().trim(), "3");

        let errored =
            format!("#!/bin/sh\ncase \"$*\" in *error*) echo 'bad config';; *) echo '{head} errored';; esac\n");
        assert_eq!(run(&errored, ""), (false, "Pages build failed: bad config".into()));
        let never = "#!/bin/sh\necho 'old building'\n";
        assert!(run(never, "").1.starts_with("no finished Pages build"));
        assert_eq!(run(never, "false && "), (false, String::new()));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn docs_keep_what_was_not_edited() {
        let text = "---\nlayout: post\ntitle: Plain Title\ntags: [a, b]\n---\n\nHallo\n\nWelt\n";
        let mut doc = Doc::parse(text);
        assert_eq!((doc.title.as_str(), doc.description.as_str()), ("Plain Title", ""));
        assert_eq!(doc.body, "Hallo\n\nWelt\n");
        assert_eq!(doc.render(), text);

        doc.description = "Neu \"hier\"".into();
        doc.body.push_str("mehr\n");
        assert_eq!(
            doc.render(),
            "---\nlayout: post\ntitle: Plain Title\ndescription: \"Neu \\\"hier\\\"\"\ntags: [a, b]\n---\n\nHallo\n\nWelt\nmehr\n"
        );
        doc.title = "Other".into();
        doc.description.clear();
        let again = Doc::parse(&doc.render());
        assert_eq!(again.render(), "---\nlayout: post\ntitle: \"Other\"\ntags: [a, b]\n---\n\nHallo\n\nWelt\nmehr\n");

        // What `create` writes reads back with an empty body.
        let fresh = Doc::parse(&front_matter("T", "D"));
        assert_eq!((fresh.title.as_str(), fresh.description.as_str(), fresh.body.as_str()), ("T", "D", ""));
        assert_eq!(fresh.render(), front_matter("T", "D"));

        // No front matter: the text is the body, and saving adds front matter.
        let mut bare = Doc::parse("just text");
        bare.title = "Titel".into();
        assert_eq!(bare.render(), "---\nlayout: post\ntitle: \"Titel\"\n---\n\njust text");
    }

    #[test]
    fn the_now_page_comes_first_with_its_updated_date() {
        let root = temp_dir("blog-now");
        write(&root.join("_posts/2026-03-01-third.md"), "---\ntitle: Third\n---\n");
        write(&root.join("_drafts/now.md"), "a draft that happens to be called now");
        assert!(list(&root).iter().all(|p| p.state != State::Now));

        write(&root.join("now.md"), "---\nlayout: now\ntitle: \"Jetzt\"\nupdated: 2026-11-20\n---\n\n- x\n");
        let posts = list(&root);
        let shown: Vec<(State, Option<&str>, &str)> =
            posts.iter().map(|p| (p.state, p.date.as_deref(), p.title.as_str())).collect();
        assert_eq!(
            shown,
            vec![
                (State::Now, Some("2026-11-20"), "Jetzt"),
                (State::Draft, None, "now"),
                (State::Post, Some("2026-03-01"), "Third"),
            ]
        );
        assert_eq!(posts[0].rel(), "now.md");
        assert!(publish(&root, &posts[0], "2026-11-27").is_err());
        assert!(unpublish(&root, &posts[0]).is_err());

        // Without front matter: titled "Now", no date.
        write(&root.join("now.md"), "just text");
        let now = post(&root.join("now.md")).unwrap();
        assert_eq!((now.state, now.date, now.title.as_str()), (State::Now, None, "Now"));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn creates_the_now_page_once() {
        let root = temp_dir("blog-now-create");
        let path = create_now(&root, "2026-11-27").unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("---\nlayout: page\ntitle: \"Now\"\n"), "{text}");
        assert!(text.contains("\npermalink: /now/\nupdated: 2026-11-27\n---\n"));
        fs::write(&path, "mine").unwrap();
        assert_eq!(create_now(&root, "2026-11-28").unwrap_err(), "now.md already exists");
        assert_eq!(fs::read_to_string(&path).unwrap(), "mine");

        // The blog's own layout when it has one.
        fs::remove_file(&path).unwrap();
        write(&root.join("_layouts/now.html"), "");
        assert!(fs::read_to_string(create_now(&root, "2026-11-27").unwrap()).unwrap().contains("layout: now\n"));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn stamps_the_updated_date() {
        assert_eq!(
            with_updated("---\ntitle: Now\nupdated: 2026-01-01\ntags: [x]\n---\n\nupdated: body\n", "2026-11-27")
                .unwrap(),
            "---\ntitle: Now\nupdated: 2026-11-27\ntags: [x]\n---\n\nupdated: body\n"
        );
        assert_eq!(
            with_updated("---\ntitle: Now\n---\n\ntext", "2026-11-27").unwrap(),
            "---\ntitle: Now\nupdated: 2026-11-27\n---\n\ntext"
        );
        assert_eq!(with_updated("no front matter", "2026-11-27"), None);

        let root = temp_dir("blog-now-stamp");
        let path = root.join("now.md");
        write(&path, "---\nupdated: 2026-01-01\n---\n");
        stamp_updated(&path, "2026-11-27").unwrap();
        assert_eq!(post(&path).unwrap().date.as_deref(), Some("2026-11-27"));
        write(&path, "nothing");
        assert_eq!(stamp_updated(&path, "2026-11-27").unwrap_err(), "now.md has no front matter");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn now_page_url_and_commit() {
        let root = temp_dir("blog-now-url");
        let now = root.join("now.md");
        write(&now, "---\ntitle: Now\n---\n");
        assert_eq!(post_url(&root, "jan/blog", &now).as_deref(), Some("https://jan.github.io/blog/now/"));
        write(&now, "---\npermalink: \"jetzt/\"\n---\n");
        fs::write(root.join("CNAME"), "blog.example.org").unwrap();
        assert_eq!(post_url(&root, "jan/blog", &now).as_deref(), Some("https://blog.example.org/jetzt/"));
        assert_eq!(commit_now_cmd("now: update"), "git add -A -- now.md && git commit -m 'now: update' && git push");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn today_is_a_date() {
        let t = today();
        assert!(split_date(&format!("{t}-x")).is_some(), "{t}");
    }
}
