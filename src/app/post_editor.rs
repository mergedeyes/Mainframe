//! The built-in editor for blog posts: title and description fields, the Markdown body, a
//! live preview. Saves by itself a moment after you stop typing, and publishes or pushes.

use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime},
};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{App, Level, Mode};
use crate::{
    blog::{self, Change, Doc, State},
    line::{self, LineEdit},
    textarea::{self, TextArea},
};

/// How long after the last change autosave waits.
const AUTOSAVE_AFTER: Duration = Duration::from_millis(1500);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    Title,
    Description,
    Body,
}

/// A y/N question shown in the editor's status line.
pub enum Ask {
    /// Publish this draft; the text is the question.
    Publish(String),
    /// Close although the file changed elsewhere, dropping the edits here.
    Discard,
}

pub struct PostEditor {
    pub root: PathBuf,
    pub path: PathBuf,
    pub state: State,
    pub title: LineEdit,
    pub description: LineEdit,
    pub body: TextArea,
    pub focus: Part,
    /// The file as read, for the front matter lines the editor doesn't show.
    doc: Doc,
    pub preview: bool,
    /// Changes not on disk yet, and when the last one was made.
    pub dirty: bool,
    changed: Instant,
    pub saved: Option<Instant>,
    /// The file's modification time when it was last read or written here.
    disk: Option<SystemTime>,
    /// Something else changed the file: autosave holds off until ^S or ^R.
    pub conflict: bool,
    pub ask: Option<Ask>,
    /// The first body row on screen, kept by the UI.
    pub scroll: usize,
}

fn mtime(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).and_then(|m| m.modified()).ok()
}

impl PostEditor {
    fn load(root: PathBuf, path: PathBuf) -> Result<Self, String> {
        let text = fs::read_to_string(&path).map_err(|e| format!("can't read {}: {e}", blog::rel(&root, &path)))?;
        let doc = Doc::parse(&text);
        let state = blog::post(&path).map_or(State::Draft, |p| p.state);
        Ok(Self {
            title: LineEdit::new(doc.title.clone()),
            description: LineEdit::new(doc.description.clone()),
            body: TextArea::new(&doc.body),
            focus: if doc.title.is_empty() { Part::Title } else { Part::Body },
            disk: mtime(&path),
            doc,
            root,
            path,
            state,
            preview: true,
            dirty: false,
            changed: Instant::now(),
            saved: None,
            conflict: false,
            ask: None,
            scroll: 0,
        })
    }

    pub fn rel(&self) -> String {
        blog::rel(&self.root, &self.path)
    }

    fn touch(&mut self) {
        self.dirty = true;
        self.changed = Instant::now();
    }

    /// Writes the post. Without `force` it won't write over changes made elsewhere.
    fn save(&mut self, force: bool) -> Result<(), String> {
        if !force && mtime(&self.path) != self.disk {
            self.conflict = true;
            return Err(format!("{} changed outside Mainframe — ^R reloads it, ^S overwrites it", self.rel()));
        }
        self.doc.title = self.title.text.trim().to_string();
        self.doc.description = self.description.text.trim().to_string();
        self.doc.body = self.body.text();
        fs::write(&self.path, self.doc.render()).map_err(|e| format!("can't save {}: {e}", self.rel()))?;
        self.disk = mtime(&self.path);
        (self.dirty, self.conflict, self.saved) = (false, false, Some(Instant::now()));
        Ok(())
    }

    /// Pasted text: all of it into the body, the first line into a field.
    pub fn paste(&mut self, text: &str) {
        match self.focus {
            Part::Body => self.body.insert_str(text),
            Part::Title | Part::Description => {
                let field = if self.focus == Part::Title { &mut self.title } else { &mut self.description };
                let first = text.lines().find(|l| !l.trim().is_empty()).unwrap_or_default();
                let head = format!("{}{}", field.head(), first.trim());
                field.replace_head(head);
            }
        }
        self.touch();
    }
}

impl App {
    /// The editor on `path` in the blog at `root`.
    pub(super) fn open_post_editor(&mut self, root: PathBuf, path: &Path, focus: Option<Part>) {
        match PostEditor::load(root, path.to_path_buf()) {
            Ok(mut ed) => {
                if let Some(f) = focus {
                    ed.focus = f;
                }
                self.mode = Mode::Post(Box::new(ed));
            }
            Err(e) => self.set_status(e, Level::Err),
        }
    }

    /// Autosave, from `tick`.
    pub(super) fn poll_post_editor(&mut self) {
        let Mode::Post(ed) = &mut self.mode else { return };
        if ed.dirty
            && !ed.conflict
            && ed.changed.elapsed() >= AUTOSAVE_AFTER
            && let Err(e) = ed.save(false)
        {
            self.set_status(e, Level::Err);
        }
    }

    pub(super) fn key_post_editor(&mut self, key: KeyEvent) {
        let Mode::Post(ed) = &mut self.mode else { return };
        if let Some(ask) = ed.ask.take() {
            if matches!(key.code, KeyCode::Char('y') | KeyCode::Char('Y')) {
                match ask {
                    Ask::Publish(_) => self.publish_from_editor(),
                    Ask::Discard => self.leave_post_editor(),
                }
            }
            return;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => return self.close_post_editor(),
            KeyCode::Char('q') if ctrl => return self.close_post_editor(),
            KeyCode::Char('s') if ctrl => {
                let res = ed.save(true).map(|()| format!("saved {}", ed.rel()));
                return match res {
                    Ok(msg) => self.set_status(msg, Level::Ok),
                    Err(e) => self.set_status(e, Level::Err),
                };
            }
            KeyCode::Char('r') if ctrl => return self.reload_post(),
            KeyCode::Char('p') if ctrl => {
                ed.preview = !ed.preview;
                return;
            }
            KeyCode::Char('g') if ctrl => return self.go_live(),
            _ => {}
        }
        match ed.focus {
            Part::Title | Part::Description => {
                let down = matches!(key.code, KeyCode::Enter | KeyCode::Tab | KeyCode::Down);
                let up = matches!(key.code, KeyCode::BackTab | KeyCode::Up);
                ed.focus = match (ed.focus, down, up) {
                    (Part::Title, true, _) => Part::Description,
                    (Part::Description, true, _) => Part::Body,
                    (Part::Description, _, true) => Part::Title,
                    (f, _, _) => {
                        let field = if f == Part::Title { &mut ed.title } else { &mut ed.description };
                        if field.key(key) == line::Edit::Changed {
                            ed.touch();
                        }
                        f
                    }
                };
            }
            Part::Body => {
                if key.code == KeyCode::Up && key.modifiers.is_empty() && ed.body.on_first_row() {
                    ed.focus = Part::Description;
                } else if ed.body.key(key) == textarea::Edit::Changed {
                    ed.touch();
                }
            }
        }
    }

    /// Esc: saves what's left and goes back to the picker. When the file changed elsewhere,
    /// asks before dropping the edits made here.
    fn close_post_editor(&mut self) {
        let Mode::Post(ed) = &mut self.mode else { return };
        if ed.dirty {
            if ed.conflict {
                ed.ask = Some(Ask::Discard);
                return;
            }
            if let Err(e) = ed.save(false) {
                self.set_status(e, Level::Err);
                return;
            }
        }
        self.leave_post_editor();
    }

    fn leave_post_editor(&mut self) {
        let Mode::Post(ed) = std::mem::replace(&mut self.mode, Mode::Normal) else { return };
        self.open_blog_picker(Some(&ed.path));
    }

    /// ^R: the file as it is on disk, dropping unsaved edits.
    fn reload_post(&mut self) {
        let Mode::Post(ed) = &self.mode else { return };
        if ed.dirty && !ed.conflict {
            return self.set_status("unsaved edits — ^S saves them first, or wait for autosave", Level::Info);
        }
        let (root, path, focus, rel) = (ed.root.clone(), ed.path.clone(), ed.focus, ed.rel());
        self.open_post_editor(root, &path, Some(focus));
        self.set_status(format!("reloaded {rel}"), Level::Info);
    }

    /// ^G: publish a draft (after asking), or commit and push a post's changes.
    fn go_live(&mut self) {
        let Mode::Post(ed) = &mut self.mode else { return };
        if ed.conflict {
            return self.set_status("the file changed outside Mainframe — ^R or ^S first", Level::Err);
        }
        if ed.title.text.trim().is_empty() {
            ed.focus = Part::Title;
            return self.set_status("a post needs a title", Level::Err);
        }
        if let Err(e) = ed.save(false) {
            return self.set_status(e, Level::Err);
        }
        let Some(post) = blog::post(&ed.path) else { return };
        match post.state {
            State::Draft => {
                let target = blog::publish_target(&ed.root, &post, &blog::today());
                if target.exists() {
                    let msg = format!("{} already exists", blog::rel(&ed.root, &target));
                    return self.set_status(msg, Level::Err);
                }
                ed.ask = Some(Ask::Publish(format!(
                    "Publish as {}, commit \"post: {}\" and push? [y/N]",
                    blog::rel(&ed.root, &target),
                    post.title
                )));
            }
            State::Post => {
                let msg = match blog::change(&ed.root, &post.path) {
                    Change::None => {
                        let msg = format!("no changes to {}", ed.rel());
                        return self.set_status(msg, Level::Info);
                    }
                    Change::New => format!("post: {}", post.title),
                    Change::Edited => format!("post: update {}", post.title),
                };
                let (root, path) = (ed.root.clone(), ed.path.clone());
                self.mode = Mode::Normal;
                self.commit_post(root, &path, msg);
            }
        }
    }

    fn publish_from_editor(&mut self) {
        let Mode::Post(ed) = std::mem::replace(&mut self.mode, Mode::Normal) else { return };
        match blog::post(&ed.path) {
            Some(post) => self.publish_post(ed.root.clone(), &post),
            None => self.mode = Mode::Post(ed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::temp_dir;

    #[test]
    fn saves_edits_but_not_over_changes_made_elsewhere() {
        let root = temp_dir("post-editor");
        let path = blog::create(&root, "Titel", "", true, "2026-11-27").unwrap();
        let mut ed = PostEditor::load(root.clone(), path.clone()).unwrap();
        assert_eq!(ed.focus, Part::Body);
        ed.body.insert_str("Hallo **Welt**\n");
        ed.description = LineEdit::new("kurz");
        ed.save(false).unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "---\nlayout: post\ntitle: \"Titel\"\ndescription: \"kurz\"\n---\n\nHallo **Welt**\n"
        );

        // Another program writes the file; a later mtime even within the same second.
        fs::write(&path, "theirs").unwrap();
        let later = SystemTime::now() + Duration::from_secs(5);
        fs::File::options().write(true).open(&path).unwrap().set_modified(later).unwrap();
        ed.body.insert_str("mehr");
        assert!(ed.save(false).is_err());
        assert!(ed.conflict);
        assert_eq!(fs::read_to_string(&path).unwrap(), "theirs");
        ed.save(true).unwrap();
        assert!(!ed.conflict);
        assert!(fs::read_to_string(&path).unwrap().ends_with("Hallo **Welt**\nmehr"));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn paste_puts_one_line_into_fields() {
        let root = temp_dir("post-editor-paste");
        let path = blog::create(&root, "T", "", true, "2026-11-27").unwrap();
        let mut ed = PostEditor::load(root.clone(), path).unwrap();
        ed.focus = Part::Description;
        ed.paste("\n  erste Zeile \nzweite");
        assert_eq!(ed.description.text, "erste Zeile");
        assert!(ed.dirty);
        fs::remove_dir_all(&root).unwrap();
    }
}
