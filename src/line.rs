//! A single-line text editor with a cursor, shared by prompts, the filter and forms.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Default, Clone)]
pub struct LineEdit {
    pub text: String,
    /// Byte offset, always on a char boundary.
    cursor: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edit {
    Changed,
    Moved,
    Ignored,
}

impl LineEdit {
    /// Cursor starts at the end.
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        Self { cursor: text.len(), text }
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Text before the cursor (what completion works on).
    pub fn head(&self) -> &str {
        &self.text[..self.cursor]
    }

    /// Cursor position in characters, for placing the terminal cursor.
    pub fn col(&self) -> usize {
        self.head().chars().count()
    }

    /// Replaces the text before the cursor, keeping what follows it.
    pub fn replace_head(&mut self, head: String) {
        let tail = self.text.split_off(self.cursor);
        self.cursor = head.len();
        self.text = head + &tail;
    }

    pub fn key(&mut self, key: KeyEvent) -> Edit {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let before = (self.text.len(), self.cursor);
        match key.code {
            KeyCode::Char('a') if ctrl => self.cursor = 0,
            KeyCode::Char('e') if ctrl => self.cursor = self.text.len(),
            KeyCode::Char('b') if alt => self.cursor = self.word_left(),
            KeyCode::Char('f') if alt => self.cursor = self.word_right(),
            KeyCode::Char('u') if ctrl => {
                self.text.drain(..self.cursor);
                self.cursor = 0;
            }
            KeyCode::Char('k') if ctrl => self.text.truncate(self.cursor),
            KeyCode::Char('w') if ctrl => self.delete_to(self.word_left()),
            KeyCode::Backspace if ctrl || alt => self.delete_to(self.word_left()),
            KeyCode::Char(c) if !ctrl && !alt => {
                self.text.insert(self.cursor, c);
                self.cursor += c.len_utf8();
            }
            KeyCode::Backspace => self.delete_to(prev(&self.text, self.cursor)),
            KeyCode::Delete => {
                let end = next(&self.text, self.cursor);
                self.text.drain(self.cursor..end);
            }
            KeyCode::Left if ctrl => self.cursor = self.word_left(),
            KeyCode::Right if ctrl => self.cursor = self.word_right(),
            KeyCode::Left => self.cursor = prev(&self.text, self.cursor),
            KeyCode::Right => self.cursor = next(&self.text, self.cursor),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.text.len(),
            _ => return Edit::Ignored,
        }
        if self.text.len() != before.0 {
            Edit::Changed
        } else if self.cursor != before.1 {
            Edit::Moved
        } else {
            Edit::Ignored
        }
    }

    /// Deletes from `start` up to the cursor.
    fn delete_to(&mut self, start: usize) {
        self.text.drain(start..self.cursor);
        self.cursor = start;
    }

    fn word_left(&self) -> usize {
        let mut i = self.cursor;
        while i > 0 && is_sep(char_before(&self.text, i)) {
            i = prev(&self.text, i);
        }
        while i > 0 && !is_sep(char_before(&self.text, i)) {
            i = prev(&self.text, i);
        }
        i
    }

    fn word_right(&self) -> usize {
        let mut i = self.cursor;
        let at = |i: usize| self.text[i..].chars().next();
        while at(i).is_some_and(is_sep) {
            i = next(&self.text, i);
        }
        while at(i).is_some_and(|c| !is_sep(c)) {
            i = next(&self.text, i);
        }
        i
    }
}

fn is_sep(c: char) -> bool {
    c.is_whitespace() || c == '/'
}

fn char_before(s: &str, i: usize) -> char {
    s[..i].chars().next_back().unwrap_or(' ')
}

fn prev(s: &str, i: usize) -> usize {
    s[..i].char_indices().next_back().map_or(0, |(j, _)| j)
}

fn next(s: &str, i: usize) -> usize {
    s[i..].chars().next().map_or(i, |c| i + c.len_utf8())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    #[test]
    fn insert_in_the_middle() {
        let mut l = LineEdit::new("fix bug");
        for _ in 0..3 {
            l.key(k(KeyCode::Left));
        }
        l.key(k(KeyCode::Char('a')));
        l.key(k(KeyCode::Char(' ')));
        assert_eq!(l.text, "fix a bug");
        l.key(k(KeyCode::Backspace));
        l.key(k(KeyCode::Delete));
        assert_eq!(l.text, "fix aug");
    }

    #[test]
    fn words_and_kills() {
        let mut l = LineEdit::new("cargo build --release");
        l.key(KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL));
        assert_eq!(l.head(), "cargo build ");
        l.key(ctrl('w'));
        assert_eq!(l.text, "cargo --release");
        l.key(ctrl('k'));
        assert_eq!(l.text, "cargo ");
        l.key(ctrl('a'));
        l.key(ctrl('u'));
        assert_eq!(l.text, "cargo ");
    }

    #[test]
    fn utf8_and_replace_head() {
        let mut l = LineEdit::new("äöü");
        l.key(k(KeyCode::Left));
        l.key(k(KeyCode::Backspace));
        assert_eq!(l.text, "äü");
        assert_eq!(l.col(), 1);
        let mut l = LineEdit::new("cd sr tail");
        for _ in 0..5 {
            l.key(k(KeyCode::Left));
        }
        l.replace_head("cd src/".into());
        assert_eq!(l.text, "cd src/ tail");
        assert_eq!(l.head(), "cd src/");
    }
}
