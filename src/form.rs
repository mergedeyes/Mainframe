//! Step-through forms: every field has a default that Enter accepts and typing replaces.

use crate::{
    complete::{self, Completer},
    line::{Edit, LineEdit},
};

pub struct Field {
    pub label: &'static str,
    pub default: String,
    /// What the user typed; empty means "use the default".
    pub input: LineEdit,
    /// Non-empty for choice fields (cycled with ←/→ or picked by first letter).
    pub options: Vec<String>,
    pub choice: usize,
    pub hint: String,
    /// Directory field: Tab completes paths instead of moving to the next field.
    pub dir: bool,
}

impl Field {
    pub fn text(label: &'static str, default: impl Into<String>, hint: impl Into<String>) -> Self {
        Self {
            label,
            default: default.into(),
            input: LineEdit::default(),
            options: Vec::new(),
            choice: 0,
            hint: hint.into(),
            dir: false,
        }
    }

    pub fn dir(label: &'static str, default: impl Into<String>, hint: impl Into<String>) -> Self {
        Self { dir: true, ..Self::text(label, default, hint) }
    }

    pub fn choice(label: &'static str, options: &[&str], default: usize, hint: impl Into<String>) -> Self {
        Self {
            options: options.iter().map(|o| o.to_string()).collect(),
            choice: default,
            ..Self::text(label, "", hint)
        }
    }

    pub fn is_choice(&self) -> bool {
        !self.options.is_empty()
    }

    pub fn value(&self) -> &str {
        if self.is_choice() {
            &self.options[self.choice]
        } else if self.input.is_empty() {
            &self.default
        } else {
            &self.input.text
        }
    }
}

pub enum FormKind {
    GhRepoCreate,
    Release(Box<crate::release::Ctx>),
}

pub struct Form {
    pub kind: FormKind,
    pub title: String,
    pub fields: Vec<Field>,
    /// Index of the active field; `fields.len()` is the final "confirm" step.
    pub step: usize,
    /// Shown under the fields (e.g. "not a git repo yet, will run git init first").
    pub note: Option<String>,
    /// Suggestions for the active directory field.
    pub completer: Option<Completer>,
}

pub enum FormEvent {
    None,
    Cancel,
    Submit,
}

impl Form {
    pub fn get(&self, label: &str) -> &str {
        self.fields.iter().find(|f| f.label == label).map_or("", Field::value)
    }

    pub fn on_confirm_step(&self) -> bool {
        self.step >= self.fields.len()
    }

    pub fn key(&mut self, key: ratatui::crossterm::event::KeyEvent) -> FormEvent {
        use ratatui::crossterm::event::{KeyCode, KeyModifiers};
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if key.code == KeyCode::Esc || (ctrl && key.code == KeyCode::Char('c')) {
            return FormEvent::Cancel;
        }
        let before = self.step;
        let event = if self.on_confirm_step() {
            match key.code {
                KeyCode::Enter | KeyCode::Char('y') => FormEvent::Submit,
                KeyCode::Up | KeyCode::BackTab | KeyCode::Backspace => {
                    self.step -= 1;
                    FormEvent::None
                }
                _ => FormEvent::None,
            }
        } else if self.fields[self.step].is_choice() {
            self.key_choice(key);
            FormEvent::None
        } else {
            self.key_text(key);
            FormEvent::None
        };
        if self.step != before {
            self.completer = None;
        }
        event
    }

    fn key_choice(&mut self, key: ratatui::crossterm::event::KeyEvent) {
        use ratatui::crossterm::event::KeyCode;
        let field = &mut self.fields[self.step];
        let n = field.options.len();
        match key.code {
            KeyCode::Enter | KeyCode::Down | KeyCode::Tab => self.step += 1,
            KeyCode::Up | KeyCode::BackTab | KeyCode::Backspace => self.step = self.step.saturating_sub(1),
            KeyCode::Right | KeyCode::Char(' ') => field.choice = (field.choice + 1) % n,
            KeyCode::Left => field.choice = (field.choice + n - 1) % n,
            KeyCode::Char(c) => {
                if let Some(i) = field.options.iter().position(|o| o.starts_with(c.to_ascii_lowercase())) {
                    field.choice = i;
                }
            }
            _ => {}
        }
    }

    fn key_text(&mut self, key: ratatui::crossterm::event::KeyEvent) {
        use ratatui::crossterm::event::KeyCode;
        let field = &mut self.fields[self.step];
        match key.code {
            KeyCode::Enter | KeyCode::Down => return self.step += 1,
            KeyCode::Up => return self.step = self.step.saturating_sub(1),
            KeyCode::Tab | KeyCode::BackTab if field.dir => {
                let completer = self.completer.get_or_insert_with(|| Completer::new(complete::Kind::Dir, false));
                if field.input.is_empty() {
                    // Start from the default so Tab refines it instead of starting over.
                    field.input = LineEdit::new(field.default.clone());
                    completer.update(field.input.head(), false);
                }
                let mut head = field.input.head().to_string();
                completer.cycle(&mut head, key.code == KeyCode::Tab);
                return field.input.replace_head(head);
            }
            KeyCode::Tab => return self.step += 1,
            KeyCode::BackTab => return self.step = self.step.saturating_sub(1),
            // Backspace on an empty field steps back, like a wizard.
            KeyCode::Backspace if field.input.is_empty() => return self.step = self.step.saturating_sub(1),
            // Moving the cursor in an untouched field means "edit the default".
            KeyCode::Left | KeyCode::Right | KeyCode::Home | KeyCode::End
                if field.input.is_empty() && !field.default.is_empty() =>
            {
                field.input = LineEdit::new(field.default.clone());
            }
            _ => {}
        }
        let edit = field.input.key(key);
        if field.dir && edit != Edit::Ignored {
            let completer = self.completer.get_or_insert_with(|| Completer::new(complete::Kind::Dir, false));
            completer.update(field.input.head(), false);
        }
    }
}
