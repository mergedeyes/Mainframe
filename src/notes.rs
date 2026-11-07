//! Notes and todos per project, as Markdown in `$XDG_DATA_HOME/mainframe/notes/`.
//! Kept outside the project so they never show up in `git status`. `- [ ]` / `- [x]`
//! lines are todos, everything else is a note line; the file stays editable by hand.

use std::{
    env, fs, io,
    path::{Path, PathBuf},
};

use crate::config::home;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    /// `indent` keeps nested todos nested when the file is written back.
    Todo { done: bool, text: String, indent: String },
    Note(String),
}

impl Item {
    pub fn todo(text: impl Into<String>) -> Self {
        Item::Todo { done: false, text: text.into(), indent: String::new() }
    }

    pub fn text(&self) -> &str {
        match self {
            Item::Todo { text, .. } | Item::Note(text) => text,
        }
    }

    pub fn set_text(&mut self, new: String) {
        match self {
            Item::Todo { text, .. } | Item::Note(text) => *text = new,
        }
    }

    pub fn is_open(&self) -> bool {
        matches!(self, Item::Todo { done: false, .. })
    }

    fn parse(line: &str) -> Self {
        let trimmed = line.trim_start();
        let indent = line[..line.len() - trimmed.len()].to_string();
        for bullet in ["- ", "* "] {
            let Some(rest) = trimmed.strip_prefix(bullet) else { continue };
            let done = match rest.get(..3) {
                Some("[ ]") => false,
                Some("[x]" | "[X]") => true,
                _ => continue,
            };
            let text = rest[3..].strip_prefix(' ').unwrap_or(&rest[3..]).to_string();
            return Item::Todo { done, text, indent };
        }
        Item::Note(line.to_string())
    }

    fn render(&self) -> String {
        match self {
            Item::Todo { done, text, indent } => format!("{indent}- [{}] {text}", if *done { 'x' } else { ' ' }),
            Item::Note(text) => text.clone(),
        }
    }
}

pub struct Notes {
    pub path: PathBuf,
    pub items: Vec<Item>,
}

impl Notes {
    /// The notes of the project at `root`; empty when there are none yet.
    pub fn load(root: &Path) -> Self {
        let path = file_for(root);
        let items = fs::read_to_string(&path).map(|t| parse(&t)).unwrap_or_default();
        Self { path, items }
    }

    pub fn open(&self) -> usize {
        self.items.iter().filter(|i| i.is_open()).count()
    }

    pub fn done(&self) -> usize {
        self.items.iter().filter(|i| matches!(i, Item::Todo { done: true, .. })).count()
    }

    /// Writes the file, or removes it once nothing is left, so emptied notes leave no trace.
    pub fn save(&self) -> io::Result<()> {
        if self.items.iter().all(|i| i.text().trim().is_empty()) {
            return match fs::remove_file(&self.path) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            };
        }
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)?;
        }
        fs::write(&self.path, render(&self.items))
    }
}

/// Open todos of the project at `root`, for the header badge and the project picker.
pub fn open_count(root: &Path) -> usize {
    fs::read_to_string(file_for(root)).map_or(0, |t| parse(&t).iter().filter(|i| i.is_open()).count())
}

pub fn dir() -> PathBuf {
    env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".local/share"))
        .join("mainframe")
        .join("notes")
}

/// `~/Projekte/Games/bash_emu` → `Projekte%2FGames%2Fbash_emu.md`: readable, and no two
/// projects share a file.
pub fn file_for(root: &Path) -> PathBuf {
    let home = home();
    let rel = root.strip_prefix(&home).map_or_else(|_| root.to_string_lossy(), |r| r.to_string_lossy());
    let rel = rel.trim_matches('/');
    let name = if rel.is_empty() { "~".to_string() } else { rel.replace('%', "%25").replace('/', "%2F") };
    dir().join(format!("{name}.md"))
}

fn parse(text: &str) -> Vec<Item> {
    text.lines().map(Item::parse).collect()
}

fn render(items: &[Item]) -> String {
    items.iter().map(Item::render).collect::<Vec<_>>().join("\n") + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_markdown() {
        let text = "# Ideas\n\n- [ ] ship it\n  - [x] nested done\n* [X] star bullet\n- plain bullet\n- [?] not a todo\n";
        let items = parse(text);
        assert_eq!(items[2], Item::todo("ship it"));
        assert_eq!(items[3], Item::Todo { done: true, text: "nested done".into(), indent: "  ".into() });
        assert!(matches!(&items[4], Item::Todo { done: true, text, .. } if text == "star bullet"));
        assert_eq!(items[5], Item::Note("- plain bullet".into()));
        assert_eq!(items[6], Item::Note("- [?] not a todo".into()));
        // `* [X]` is written back the canonical way, everything else unchanged.
        assert_eq!(render(&items), text.replace("* [X]", "- [x]"));
    }

    #[test]
    fn file_names_stay_unique() {
        let h = home();
        assert_ne!(file_for(&h.join("a%2Fb")), file_for(&h.join("a/b")));
        assert!(file_for(&h.join("Projekte/Mainframe")).ends_with("Projekte%2FMainframe.md"));
    }
}
