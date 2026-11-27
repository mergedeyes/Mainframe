//! A Jekyll blog in the repo `blog_repo` names: its drafts and posts, new posts, and moving
//! them between `_drafts/` and `_posts/`. Only the files; the `L` picker is in `app::blog_picker`.

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Draft,
    Post,
}

#[derive(Debug, Clone)]
pub struct Post {
    pub path: PathBuf,
    pub state: State,
    /// `YYYY-MM-DD` from the file name; posts only.
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
    let blog = fs::canonicalize(config::expand_tilde(setting)).ok()?;
    let cwd = fs::canonicalize(cwd).ok()?;
    if cwd.starts_with(&blog) {
        return Some(blog);
    }
    // Another worktree shares the blog's git dir. Only when the blog folder is the repo's
    // top, so a blog inside, say, a dotfiles repo at $HOME doesn't claim all of $HOME.
    let top = |d: &Path| git_text(d, &["rev-parse", "--show-toplevel"]).and_then(|t| fs::canonicalize(t).ok());
    let common = |d: &Path| {
        git_text(d, &["rev-parse", "--path-format=absolute", "--git-common-dir"]).and_then(|t| fs::canonicalize(t).ok())
    };
    let here = top(&cwd)?;
    (top(&blog)? == blog && common(&cwd)? == common(&blog)?).then_some(here)
}

/// Drafts first, most recently changed first; then posts, newest first.
pub fn list(root: &Path) -> Vec<Post> {
    let mut drafts: Vec<(SystemTime, Post)> = md_files(&root.join(DRAFTS))
        .into_iter()
        .map(|(path, stem)| {
            let changed = fs::metadata(&path).and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
            let title = read_title(&path).unwrap_or_else(|| stem.clone());
            (changed, Post { path, state: State::Draft, date: None, slug: stem, title })
        })
        .collect();
    drafts.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.slug.cmp(&b.1.slug)));

    let mut posts: Vec<Post> = md_files(&root.join(POSTS))
        .into_iter()
        .filter_map(|(path, stem)| {
            let (date, slug) = split_date(&stem)?;
            let title = read_title(&path).unwrap_or_else(|| slug.to_string());
            Some(Post { state: State::Post, date: Some(date.to_string()), slug: slug.to_string(), title, path })
        })
        .collect();
    posts.sort_by(|a, b| b.date.cmp(&a.date).then_with(|| b.slug.cmp(&a.slug)));

    drafts.into_iter().map(|(_, p)| p).chain(posts).collect()
}

/// `(path, stem)` of every `*.md` file directly in `dir`.
fn md_files(dir: &Path) -> Vec<(PathBuf, String)> {
    let Ok(entries) = fs::read_dir(dir) else { return Vec::new() };
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|x| x == "md"))
        .filter_map(|p| Some((p.clone(), p.file_stem()?.to_str()?.to_string())))
        .collect()
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
    let mut lines = text.lines();
    if lines.next()?.trim_end() != "---" {
        return None;
    }
    for line in lines {
        if line.trim_end() == "---" {
            break;
        }
        if let Some(value) = line.strip_prefix("title:") {
            let title = unquote(value.trim());
            return (!title.is_empty()).then_some(title);
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
    // SAFETY: `time` and `localtime_r` only write to the `tm` handed to them.
    unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&now, &mut tm).is_null() {
            return "1970-01-01".into();
        }
        format!("{:04}-{:02}-{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday)
    }
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

fn move_to(root: &Path, from: &Path, to: &Path) -> Result<(), String> {
    if to.exists() {
        return Err(format!("{} already exists", rel(root, to)));
    }
    let res = fs::create_dir_all(to.parent().expect("always in a folder")).and_then(|_| fs::rename(from, to));
    res.map_err(|e| format!("can't move {} to {}: {e}", rel(root, from), rel(root, to)))
}

/// `path` relative to the blog, for messages.
pub fn rel(root: &Path, path: &Path) -> String {
    path.strip_prefix(root).unwrap_or(path).display().to_string()
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

/// Shell steps that follow the GitHub Pages build of the pushed commit, then print and
/// notify "post live: <url>" or what went wrong. None without `gh` or a GitHub origin.
pub fn follow_pages_cmd(root: &Path, post_path: &Path) -> Option<String> {
    which("gh")?;
    let origin = git_text(root, &["remote", "get-url", "origin"])?;
    let repo = crate::release::github_repo(&origin)?;
    Some(follow_pages_script(&repo, &post_url(root, &repo, post_path)?))
}

/// The polling loop behind [`follow_pages_cmd`], in braces so it joins a `&&` chain as one
/// step. About three minutes, every five seconds: the build for a push shows up after a while.
fn follow_pages_script(repo: &str, url: &str) -> String {
    let api = format!("gh api repos/{repo}/pages/builds/latest");
    let live = quote_str(&format!("post live: {url}"));
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

/// The address of a post, by Jekyll's default permalink `/:year/:month/:day/:title/`, on the
/// domain in `CNAME` or else the repo's github.io address.
pub fn post_url(root: &Path, repo: &str, post_path: &Path) -> Option<String> {
    let stem = post_path.file_stem()?.to_str()?;
    let (date, slug) = split_date(stem)?;
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
    Some(format!("{base}/{}/{slug}/", date.replace('-', "/")))
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
        let blog = fs::canonicalize(&blog).unwrap();
        let setting = blog.to_str().unwrap();

        assert_eq!(root(&blog, setting), Some(blog.clone()));
        assert_eq!(root(&blog.join("_posts"), setting), Some(blog.clone()));
        assert_eq!(root(&blog.join("_posts"), &format!("{setting}/")), Some(blog.clone()));
        assert_eq!(root(&wt, setting), Some(fs::canonicalize(&wt).unwrap()));
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
            let script = format!("{before}{}", follow_pages_script("me/blog", "https://me.github.io/blog/x/"));
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
    fn today_is_a_date() {
        let t = today();
        assert!(split_date(&format!("{t}-x")).is_some(), "{t}");
    }
}
