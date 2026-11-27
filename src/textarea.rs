//! A multi-line text editor with soft word wrap and undo, for writing blog posts. Only the
//! text and the cursor; drawing it is up to the UI, which tells it how wide and tall it is.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Undo steps kept.
const MAX_UNDO: usize = 500;

#[derive(Debug, Clone, Default)]
pub struct TextArea {
    pub lines: Vec<String>,
    /// Cursor: line index and byte offset in it, always on a char boundary.
    row: usize,
    col: usize,
    /// Column on screen kept while moving up and down through shorter rows.
    want: Option<usize>,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    /// The kind of the last edit, so a run of typing is one undo step.
    last: Step,
    /// Wrap width in characters, set by the UI; 0 means "not drawn yet", no wrapping.
    pub width: usize,
    /// Visible rows, set by the UI, for PgUp / PgDn.
    pub height: usize,
}

#[derive(Debug, Clone)]
struct Snapshot {
    lines: Vec<String>,
    row: usize,
    col: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Step {
    #[default]
    None,
    Typing,
    Deleting,
}

/// What a key did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edit {
    Changed,
    Moved,
    Ignored,
}

impl TextArea {
    pub fn new(text: &str) -> Self {
        Self { lines: text.split('\n').map(String::from).collect(), ..Self::default() }
    }

    pub fn text(&self) -> String {
        self.lines.join("\n")
    }

    /// Cursor as (line, byte offset).
    pub fn cursor(&self) -> (usize, usize) {
        (self.row, self.col)
    }

    pub fn words(&self) -> usize {
        self.lines.iter().map(|l| l.split_whitespace().count()).sum()
    }

    pub fn on_first_row(&self) -> bool {
        self.row == 0 && self.sub_row() == 0
    }

    fn line(&self) -> &str {
        &self.lines[self.row]
    }

    // ── keys ───────────────────────────────────────────────────────────────

    pub fn key(&mut self, key: KeyEvent) -> Edit {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let before = (self.row, self.col);
        let changed = match key.code {
            KeyCode::Char('z') if ctrl => self.undo(),
            KeyCode::Char('y') if ctrl => self.redo(),
            KeyCode::Char('b') if ctrl => self.wrap_word("**", "**"),
            KeyCode::Char('e') if ctrl => self.wrap_word("_", "_"),
            KeyCode::Char('k') if ctrl => self.link(),
            KeyCode::Char('w') if ctrl => self.delete_word_left(),
            KeyCode::Backspace if ctrl || alt => self.delete_word_left(),
            KeyCode::Char(c) if !ctrl && !alt => {
                self.insert_char(c);
                true
            }
            KeyCode::Enter => {
                self.enter();
                true
            }
            KeyCode::Tab => self.indent(true),
            KeyCode::BackTab => self.indent(false),
            KeyCode::Backspace => self.backspace(),
            KeyCode::Delete => self.delete(),
            _ => {
                self.motion(key.code, ctrl);
                false
            }
        };
        if !matches!(key.code, KeyCode::Up | KeyCode::Down | KeyCode::PageUp | KeyCode::PageDown) {
            self.want = None;
        }
        if changed {
            Edit::Changed
        } else if (self.row, self.col) != before {
            self.last = Step::None;
            Edit::Moved
        } else {
            Edit::Ignored
        }
    }

    fn motion(&mut self, code: KeyCode, ctrl: bool) {
        match code {
            KeyCode::Left if ctrl => self.word_left(),
            KeyCode::Right if ctrl => self.word_right(),
            KeyCode::Left => {
                if self.col > 0 {
                    self.col = prev(self.line(), self.col);
                } else if self.row > 0 {
                    self.row -= 1;
                    self.col = self.line().len();
                }
            }
            KeyCode::Right => {
                if self.col < self.line().len() {
                    self.col = next(self.line(), self.col);
                } else if self.row + 1 < self.lines.len() {
                    self.row += 1;
                    self.col = 0;
                }
            }
            KeyCode::Up => self.vertical(-1),
            KeyCode::Down => self.vertical(1),
            KeyCode::PageUp => self.vertical(-(self.height.max(2) as isize - 1)),
            KeyCode::PageDown => self.vertical(self.height.max(2) as isize - 1),
            KeyCode::Home if ctrl => (self.row, self.col) = (0, 0),
            KeyCode::End if ctrl => {
                self.row = self.lines.len() - 1;
                self.col = self.line().len();
            }
            KeyCode::Home => self.col = 0,
            KeyCode::End => self.col = self.line().len(),
            _ => {}
        }
    }

    // ── wrapping and screen rows ───────────────────────────────────────────

    /// Where each screen row of line `i` starts.
    pub fn rows_of(&self, i: usize) -> Vec<usize> {
        wrap(&self.lines[i], self.width)
    }

    /// The screen row the cursor is on within its line.
    fn sub_row(&self) -> usize {
        row_of(&self.rows_of(self.row), self.col)
    }

    /// The cursor on screen: (row counted from the top of the text, column).
    pub fn screen_cursor(&self) -> (usize, usize) {
        let above: usize = (0..self.row).map(|i| self.rows_of(i).len()).sum();
        let starts = self.rows_of(self.row);
        let r = row_of(&starts, self.col);
        (above + r, self.lines[self.row][starts[r]..self.col].chars().count())
    }

    /// Moves `n` screen rows up (negative) or down, keeping the column.
    fn vertical(&mut self, n: isize) {
        let starts = self.rows_of(self.row);
        let r = row_of(&starts, self.col);
        let x = *self.want.get_or_insert_with(|| self.lines[self.row][starts[r]..self.col].chars().count());
        let (mut line, mut sub) = (self.row, r);
        for _ in 0..n.unsigned_abs() {
            if n < 0 {
                if sub > 0 {
                    sub -= 1;
                } else if line > 0 {
                    line -= 1;
                    sub = self.rows_of(line).len() - 1;
                } else {
                    // Up on the first row goes to the start, like most editors.
                    self.col = 0;
                    return;
                }
            } else if sub + 1 < self.rows_of(line).len() {
                sub += 1;
            } else if line + 1 < self.lines.len() {
                line += 1;
                sub = 0;
            } else {
                self.col = self.line().len();
                return;
            }
        }
        let starts = self.rows_of(line);
        let text = &self.lines[line];
        let end = starts.get(sub + 1).copied();
        let row_text = &text[starts[sub]..end.unwrap_or(text.len())];
        // A wrapped row's last character belongs to the next row once the cursor is past it.
        let max = row_text.chars().count() - usize::from(end.is_some() && !row_text.is_empty());
        let offset: usize = row_text.chars().take(x.min(max)).map(char::len_utf8).sum();
        self.row = line;
        self.col = starts[sub] + offset;
    }

    // ── editing ────────────────────────────────────────────────────────────

    /// Saves the state for undo, unless this edit continues a run of the same kind.
    fn checkpoint(&mut self, step: Step) {
        if step == Step::None || step != self.last {
            self.undo.push(Snapshot { lines: self.lines.clone(), row: self.row, col: self.col });
            if self.undo.len() > MAX_UNDO {
                self.undo.remove(0);
            }
        }
        self.redo.clear();
        self.last = step;
    }

    fn undo(&mut self) -> bool {
        let Some(s) = self.undo.pop() else { return false };
        self.redo.push(Snapshot { lines: std::mem::replace(&mut self.lines, s.lines), row: self.row, col: self.col });
        (self.row, self.col) = (s.row, s.col);
        self.last = Step::None;
        true
    }

    fn redo(&mut self) -> bool {
        let Some(s) = self.redo.pop() else { return false };
        self.undo.push(Snapshot { lines: std::mem::replace(&mut self.lines, s.lines), row: self.row, col: self.col });
        (self.row, self.col) = (s.row, s.col);
        self.last = Step::None;
        true
    }

    fn insert_char(&mut self, c: char) {
        // A new word is a new undo step, so undo doesn't take back a whole paragraph.
        let word_start = c.is_whitespace() && !self.line()[..self.col].ends_with(char::is_whitespace);
        self.checkpoint(if word_start { Step::None } else { Step::Typing });
        self.last = Step::Typing;
        let col = self.col;
        self.lines[self.row].insert(col, c);
        self.col += c.len_utf8();
    }

    /// Inserts pasted text as one undo step, without list continuation.
    pub fn insert_str(&mut self, s: &str) {
        let s = s.replace("\r\n", "\n").replace('\r', "\n");
        if s.is_empty() {
            return;
        }
        self.checkpoint(Step::None);
        let tail = self.lines[self.row].split_off(self.col);
        let mut parts = s.split('\n');
        self.lines[self.row].push_str(parts.next().unwrap_or_default());
        for part in parts {
            self.row += 1;
            self.lines.insert(self.row, part.to_string());
        }
        self.col = self.lines[self.row].len();
        self.lines[self.row].push_str(&tail);
        self.want = None;
    }

    /// A new line that continues the list or quote the cursor is in; on an empty item it
    /// ends the list instead.
    fn enter(&mut self) {
        self.checkpoint(Step::None);
        let marker = list_marker(self.line());
        if let Some(m) = &marker
            && self.line().trim_end() == m.text.trim_end()
            && self.col == self.line().len()
        {
            self.lines[self.row].clear();
            self.col = 0;
            return;
        }
        let tail = self.lines[self.row].split_off(self.col);
        let prefix = match marker {
            Some(m) if self.col >= m.text.len() => m.next,
            _ => String::new(),
        };
        self.row += 1;
        self.col = prefix.len();
        self.lines.insert(self.row, prefix + &tail);
    }

    fn backspace(&mut self) -> bool {
        if self.col == 0 && self.row == 0 {
            return false;
        }
        self.checkpoint(Step::Deleting);
        if self.col > 0 {
            let start = prev(self.line(), self.col);
            self.lines[self.row].drain(start..self.col);
            self.col = start;
        } else {
            let line = self.lines.remove(self.row);
            self.row -= 1;
            self.col = self.line().len();
            self.lines[self.row].push_str(&line);
        }
        true
    }

    fn delete(&mut self) -> bool {
        if self.col == self.line().len() && self.row + 1 == self.lines.len() {
            return false;
        }
        self.checkpoint(Step::Deleting);
        if self.col < self.line().len() {
            let end = next(self.line(), self.col);
            let col = self.col;
            self.lines[self.row].drain(col..end);
        } else {
            let line = self.lines.remove(self.row + 1);
            self.lines[self.row].push_str(&line);
        }
        true
    }

    fn delete_word_left(&mut self) -> bool {
        if self.col == 0 {
            return self.backspace();
        }
        self.checkpoint(Step::None);
        let start = word_left_in(self.line(), self.col);
        self.lines[self.row].drain(start..self.col);
        self.col = start;
        true
    }

    /// Tab indents a list item by two spaces (or puts four spaces in elsewhere); Shift-Tab
    /// takes the indent away again.
    fn indent(&mut self, more: bool) -> bool {
        let in_list = list_marker(self.line()).is_some();
        let lead = self.line().len() - self.line().trim_start_matches(' ').len();
        if more {
            self.checkpoint(Step::None);
            let (at, pad) = if in_list { (0, "  ") } else { (self.col, "    ") };
            self.lines[self.row].insert_str(at, pad);
            self.col += pad.len();
        } else {
            let n = lead.min(if in_list { 2 } else { 4 });
            if n == 0 {
                return false;
            }
            self.checkpoint(Step::None);
            self.lines[self.row].drain(..n);
            self.col = self.col.saturating_sub(n);
        }
        true
    }

    /// The word at the cursor as a byte range, if the cursor is in or right after one.
    fn word_at(&self) -> Option<(usize, usize)> {
        let line = self.line();
        let is_word = |c: char| c.is_alphanumeric() || c == '-' || c == '\'';
        let start = line[..self.col].char_indices().rev().take_while(|&(_, c)| is_word(c)).last().map(|(i, _)| i);
        let end = self.col + line[self.col..].chars().take_while(|&c| is_word(c)).map(char::len_utf8).sum::<usize>();
        let start = start.unwrap_or(self.col);
        (start < end).then_some((start, end))
    }

    /// Puts `open` and `close` around the word at the cursor, or an empty pair to type in.
    fn wrap_word(&mut self, open: &str, close: &str) -> bool {
        self.checkpoint(Step::None);
        let (start, end) = self.word_at().unwrap_or((self.col, self.col));
        let empty = start == end;
        let line = &mut self.lines[self.row];
        line.insert_str(end, close);
        line.insert_str(start, open);
        self.col = if empty { start + open.len() } else { end + open.len() + close.len() };
        true
    }

    /// `[word]()` with the cursor in the parentheses, or `[]()` with it in the brackets.
    fn link(&mut self) -> bool {
        let had_word = self.word_at().is_some();
        self.wrap_word("[", "]()");
        if had_word {
            self.col -= 1;
        }
        true
    }

    fn word_left(&mut self) {
        if self.col == 0 {
            if self.row > 0 {
                self.row -= 1;
                self.col = self.line().len();
            }
            return;
        }
        self.col = word_left_in(self.line(), self.col);
    }

    fn word_right(&mut self) {
        let line = self.line();
        if self.col == line.len() {
            if self.row + 1 < self.lines.len() {
                self.row += 1;
                self.col = 0;
            }
            return;
        }
        let rest = &line[self.col..];
        let spaces: usize = rest.chars().take_while(|c| c.is_whitespace()).map(char::len_utf8).sum();
        let word: usize = rest[spaces..].chars().take_while(|c| !c.is_whitespace()).map(char::len_utf8).sum();
        self.col += spaces + word;
    }
}

/// The marker of a list item or quote at the start of `line`, and what the next line starts with.
struct Marker {
    text: String,
    next: String,
}

fn list_marker(line: &str) -> Option<Marker> {
    let indent = &line[..line.len() - line.trim_start_matches(' ').len()];
    let rest = &line[indent.len()..];
    let (mark, next_mark) = if rest.starts_with("> ") {
        ("> ".to_string(), "> ".to_string())
    } else if let Some(c) = rest.chars().next().filter(|c| matches!(c, '-' | '*' | '+'))
        && rest[1..].starts_with(' ')
    {
        (format!("{c} "), format!("{c} "))
    } else {
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        let after = &rest[digits.len()..];
        let dot = after.chars().next().filter(|c| matches!(c, '.' | ')'))?;
        if digits.is_empty() || digits.len() > 9 || !after[1..].starts_with(' ') {
            return None;
        }
        let n: u64 = digits.parse().ok()?;
        (format!("{digits}{dot} "), format!("{}{dot} ", n + 1))
    };
    // A task item continues as an open task.
    let after = &rest[mark.len()..];
    let task = ["[ ] ", "[x] ", "[X] "].into_iter().find(|t| after.starts_with(t));
    let (mark, next_mark) = match task {
        Some(t) if !mark.starts_with('>') => (format!("{mark}{t}"), format!("{next_mark}[ ] ")),
        _ => (mark, next_mark),
    };
    Some(Marker { text: format!("{indent}{mark}"), next: format!("{indent}{next_mark}") })
}

/// Where each screen row of `line` starts when it's wrapped at `width` characters: after
/// the last space that fits, or in the middle of a word longer than a row.
pub fn wrap(line: &str, width: usize) -> Vec<usize> {
    let mut starts = vec![0];
    if width == 0 {
        return starts;
    }
    let mut start = 0;
    loop {
        let rest = &line[start..];
        let Some((cut, c)) = rest.char_indices().nth(width) else { break };
        // Break after the last space within the row (or right after it), so the space stays
        // at the row's end.
        let at = match rest[..cut + c.len_utf8()].rfind(' ') {
            Some(sp) if sp > 0 => sp + 1,
            _ => cut,
        };
        start += at;
        starts.push(start);
    }
    starts
}

/// Which of the rows starting at `starts` holds byte `col`.
pub fn row_of(starts: &[usize], col: usize) -> usize {
    starts.iter().rposition(|&s| s <= col).unwrap_or(0)
}

fn word_left_in(line: &str, col: usize) -> usize {
    let head = &line[..col];
    let trimmed = head.trim_end();
    trimmed.char_indices().rev().find(|(_, c)| c.is_whitespace()).map_or(0, |(i, c)| i + c.len_utf8())
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

    fn typed(t: &mut TextArea, s: &str) {
        for c in s.chars() {
            t.key(if c == '\n' { k(KeyCode::Enter) } else { k(KeyCode::Char(c)) });
        }
    }

    #[test]
    fn wraps_after_spaces_and_inside_long_words() {
        assert_eq!(wrap("hello world foo", 11), vec![0, 12]);
        assert_eq!(wrap("hello world foo", 8), vec![0, 6, 12]);
        assert_eq!(wrap("abcdefghij", 4), vec![0, 4, 8]);
        assert_eq!(wrap("short", 10), vec![0]);
        assert_eq!(wrap("exactly10!", 10), vec![0]);
        assert_eq!(wrap("äöü äöü", 4), vec![0, "äöü ".len()]);
        assert_eq!(wrap("anything", 0), vec![0]);
    }

    #[test]
    fn types_joins_and_splits_lines() {
        let mut t = TextArea::new("");
        typed(&mut t, "Hallo\nWelt");
        assert_eq!(t.text(), "Hallo\nWelt");
        t.key(k(KeyCode::Home));
        t.key(k(KeyCode::Backspace));
        assert_eq!(t.text(), "HalloWelt");
        assert_eq!(t.cursor(), (0, 5));
        t.key(k(KeyCode::End));
        t.key(k(KeyCode::Delete));
        assert_eq!(t.text(), "HalloWelt");
        t.key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::CONTROL));
        assert_eq!(t.text(), "");
        assert_eq!(t.words(), 0);
    }

    #[test]
    fn undo_goes_word_by_word_and_redo_comes_back() {
        let mut t = TextArea::new("");
        typed(&mut t, "eins zwei drei");
        t.key(ctrl('z'));
        assert_eq!(t.text(), "eins zwei");
        t.key(ctrl('z'));
        assert_eq!(t.text(), "eins");
        t.key(ctrl('y'));
        assert_eq!(t.text(), "eins zwei");
        // Something new drops the redo.
        typed(&mut t, "!");
        assert_eq!(t.key(ctrl('y')), Edit::Ignored);
        while t.key(ctrl('z')) == Edit::Changed {}
        assert_eq!(t.text(), "");
    }

    #[test]
    fn enter_continues_lists_and_ends_them_on_an_empty_item() {
        let mut t = TextArea::new("");
        typed(&mut t, "- eins\nzwei\n\nweiter");
        assert_eq!(t.text(), "- eins\n- zwei\nweiter");

        let mut t = TextArea::new("");
        typed(&mut t, "9. neun\nzehn\n\n  > zitat\nmehr");
        assert_eq!(t.text(), "9. neun\n10. zehn\n  > zitat\n  > mehr");

        let mut t = TextArea::new("  * [x] done\n");
        t.key(k(KeyCode::Up));
        t.key(k(KeyCode::End));
        typed(&mut t, "\nnext");
        assert_eq!(t.lines[1], "  * [ ] next");

        // In the middle of the marker, Enter is just a line break.
        let mut t = TextArea::new("- item");
        t.key(k(KeyCode::Home));
        typed(&mut t, "\n");
        assert_eq!(t.text(), "\n- item");
    }

    #[test]
    fn wraps_words_in_markers() {
        let mut t = TextArea::new("ein wort hier");
        for _ in 0..6 {
            t.key(k(KeyCode::Right));
        }
        t.key(ctrl('b'));
        assert_eq!(t.text(), "ein **wort** hier");
        assert_eq!(t.cursor(), (0, 12));
        t.key(k(KeyCode::End));
        t.key(k(KeyCode::Char(' ')));
        t.key(ctrl('e'));
        typed(&mut t, "neu");
        assert_eq!(t.text(), "ein **wort** hier _neu_");
        t.key(k(KeyCode::Home));
        t.key(ctrl('k'));
        typed(&mut t, "https://x.org");
        assert_eq!(t.text(), "[ein](https://x.org) **wort** hier _neu_");
    }

    #[test]
    fn moves_by_screen_rows() {
        let mut t = TextArea::new("aaaa bbbb cccc\nxy");
        t.width = 5;
        assert_eq!(t.rows_of(0), vec![0, 5, 10]);
        t.key(KeyEvent::new(KeyCode::Home, KeyModifiers::CONTROL));
        t.key(k(KeyCode::Right));
        t.key(k(KeyCode::Right));
        t.key(k(KeyCode::Down));
        assert_eq!(t.cursor(), (0, 7));
        assert_eq!(t.screen_cursor(), (1, 2));
        t.key(k(KeyCode::Down));
        t.key(k(KeyCode::Down));
        // The column is kept, clamped to the short line.
        assert_eq!(t.cursor(), (1, 2));
        assert_eq!(t.screen_cursor(), (3, 2));
        t.key(k(KeyCode::Up));
        assert_eq!(t.cursor(), (0, 12));
        t.key(k(KeyCode::Down));
        t.key(k(KeyCode::Down));
        assert_eq!(t.cursor(), (1, 2));
        t.key(k(KeyCode::Up));
        t.key(k(KeyCode::Up));
        t.key(k(KeyCode::Up));
        assert!(t.on_first_row());
        t.key(k(KeyCode::Up));
        assert_eq!(t.cursor(), (0, 0));
    }

    #[test]
    fn pastes_as_one_step() {
        let mut t = TextArea::new("vorher nachher");
        for _ in 0..7 {
            t.key(k(KeyCode::Right));
        }
        t.insert_str("- a\r\n- b\n");
        assert_eq!(t.text(), "vorher - a\n- b\nnachher");
        assert_eq!(t.cursor(), (2, 0));
        t.key(ctrl('z'));
        assert_eq!(t.text(), "vorher nachher");
    }

    #[test]
    fn tab_indents_list_items() {
        let mut t = TextArea::new("- item");
        t.key(k(KeyCode::Tab));
        assert_eq!(t.text(), "  - item");
        t.key(k(KeyCode::BackTab));
        t.key(k(KeyCode::BackTab));
        assert_eq!(t.text(), "- item");
        let mut t = TextArea::new("code");
        t.key(k(KeyCode::Home));
        t.key(k(KeyCode::Tab));
        assert_eq!(t.text(), "    code");
    }

    #[test]
    fn word_jumps_cross_lines() {
        let mut t = TextArea::new("one two\nthree");
        t.key(KeyEvent::new(KeyCode::Home, KeyModifiers::CONTROL));
        let right = KeyEvent::new(KeyCode::Right, KeyModifiers::CONTROL);
        t.key(right);
        assert_eq!(t.cursor(), (0, 3));
        t.key(right);
        t.key(right);
        assert_eq!(t.cursor(), (1, 0));
        let left = KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL);
        t.key(left);
        t.key(left);
        assert_eq!(t.cursor(), (0, 4));
    }
}
