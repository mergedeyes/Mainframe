//! A list filtered as you type: the core shared by the `T`, `g o`, `J` and `^P` pickers.

use ratatui::{
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    widgets::ListState,
};

use crate::{
    fuzzy,
    line::{Edit, LineEdit},
};

/// How typed text picks items.
#[derive(Clone, Copy)]
pub enum Match {
    /// Every word appears somewhere, list order kept.
    Words,
    /// Subsequence match, best first; only the best `limit` are listed.
    Fuzzy { limit: usize },
}

pub struct FilterList<T> {
    pub items: Vec<T>,
    /// What the filter searches in, per item, lowercased once up front.
    hay: Vec<String>,
    how: Match,
    pub filter: LineEdit,
    /// Indices into `items` that match, in the order shown.
    pub visible: Vec<usize>,
    pub state: ListState,
    /// Matches before `limit` cut the list.
    pub total: usize,
}

/// What a key did to the list.
#[derive(PartialEq, Eq)]
pub enum Key {
    Handled,
    /// Esc or ^C: the picker should close.
    Close,
}

impl<T> FilterList<T> {
    pub fn new(items: Vec<T>, hay: impl Fn(&T) -> String, how: Match) -> Self {
        let hay = items.iter().map(|t| hay(t).to_lowercase()).collect();
        let mut list = Self {
            items,
            hay,
            how,
            filter: LineEdit::default(),
            visible: Vec::new(),
            state: ListState::default(),
            total: 0,
        };
        list.apply_filter();
        list
    }

    fn apply_filter(&mut self) {
        self.visible = match self.how {
            Match::Words => {
                let words: Vec<String> = self.filter.text.split_whitespace().map(str::to_lowercase).collect();
                (0..self.items.len()).filter(|&i| words.iter().all(|w| self.hay[i].contains(w))).collect()
            }
            Match::Fuzzy { limit } => {
                let q = self.filter.text.replace(' ', "").to_lowercase();
                let mut scored: Vec<(i64, usize)> =
                    self.hay.iter().enumerate().filter_map(|(i, h)| fuzzy::score(&q, h).map(|s| (s, i))).collect();
                if !q.is_empty() {
                    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
                }
                self.total = scored.len();
                scored.into_iter().take(limit).map(|(_, i)| i).collect()
            }
        };
        if matches!(self.how, Match::Words) {
            self.total = self.visible.len();
        }
        self.state.select((!self.visible.is_empty()).then_some(0));
    }

    /// Empties the filter, showing every item again.
    pub fn clear_filter(&mut self) {
        self.filter = LineEdit::default();
        self.apply_filter();
    }

    /// Selects the first shown item `pick` accepts, if any.
    pub fn select_where(&mut self, pick: impl Fn(&T) -> bool) {
        if let Some(row) = self.visible.iter().position(|&i| pick(&self.items[i])) {
            self.state.select(Some(row));
        }
    }

    pub fn selected(&self) -> Option<&T> {
        self.visible.get(self.state.selected()?).map(|&i| &self.items[i])
    }

    /// The keys every picker shares: moving, closing, typing into the filter. A picker
    /// handles its own keys (Enter, its ctrl keys) before handing the rest here.
    pub fn key(&mut self, key: KeyEvent) -> Key {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let len = self.visible.len();
        match key.code {
            KeyCode::Esc => return Key::Close,
            KeyCode::Char('c') if ctrl => return Key::Close,
            KeyCode::Down | KeyCode::Tab => step(&mut self.state, len, 1),
            KeyCode::Char('n') if ctrl => step(&mut self.state, len, 1),
            KeyCode::Up | KeyCode::BackTab => step(&mut self.state, len, -1),
            KeyCode::Char('p') if ctrl => step(&mut self.state, len, -1),
            KeyCode::PageDown => step(&mut self.state, len, 10),
            KeyCode::PageUp => step(&mut self.state, len, -10),
            _ => {
                if self.filter.key(key) == Edit::Changed {
                    self.apply_filter();
                }
            }
        }
        Key::Handled
    }
}

/// Moves the selection of a `len`-row list by `delta`, stopping at either end.
pub fn step(state: &mut ListState, len: usize, delta: isize) {
    let cur = state.selected().unwrap_or(0);
    state.select(Some(cur.saturating_add_signed(delta).min(len.saturating_sub(1))));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(list: &mut FilterList<&'static str>, text: &str) {
        for c in text.chars() {
            list.key(KeyEvent::from(KeyCode::Char(c)));
        }
    }

    #[test]
    fn words_keep_order_and_fuzzy_ranks() {
        let items = vec!["src/app/mod.rs", "README.md", "src/main.rs"];
        let mut words = FilterList::new(items.clone(), |s| s.to_string(), Match::Words);
        typed(&mut words, "src rs");
        assert_eq!(words.visible, vec![0, 2]);
        assert_eq!(words.total, 2);

        let mut fuzzy = FilterList::new(items, |s| s.to_string(), Match::Fuzzy { limit: 1 });
        typed(&mut fuzzy, "main");
        assert_eq!(fuzzy.selected(), Some(&"src/main.rs"));
        assert_eq!(fuzzy.visible.len(), 1);
    }

    #[test]
    fn steps_stop_at_the_ends() {
        let mut state = ListState::default();
        step(&mut state, 3, -1);
        assert_eq!(state.selected(), Some(0));
        step(&mut state, 3, 10);
        assert_eq!(state.selected(), Some(2));
        step(&mut state, 0, 1);
        assert_eq!(state.selected(), Some(0));
    }
}
