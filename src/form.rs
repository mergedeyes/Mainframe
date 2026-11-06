//! Step-through forms: every field has a default that Enter accepts and typing replaces.

use crate::complete::{self, Completer};

pub struct Field {
    pub label: &'static str,
    pub default: String,
    /// What the user typed; empty means "use the default".
    pub input: String,
    /// Non-empty for choice fields (cycled with ←/→ or picked by first letter).
    pub options: Vec<&'static str>,
    pub choice: usize,
    pub hint: &'static str,
    /// Directory field: Tab completes paths instead of moving to the next field.
    pub dir: bool,
}

impl Field {
    pub fn text(label: &'static str, default: impl Into<String>, hint: &'static str) -> Self {
        Self { label, default: default.into(), input: String::new(), options: Vec::new(), choice: 0, hint, dir: false }
    }

    pub fn dir(label: &'static str, default: impl Into<String>, hint: &'static str) -> Self {
        Self { dir: true, ..Self::text(label, default, hint) }
    }

    pub fn choice(label: &'static str, options: &[&'static str], default: usize, hint: &'static str) -> Self {
        Self { label, default: String::new(), input: String::new(), options: options.to_vec(), choice: default, hint, dir: false }
    }

    pub fn is_choice(&self) -> bool {
        !self.options.is_empty()
    }

    pub fn value(&self) -> &str {
        if self.is_choice() {
            self.options[self.choice]
        } else if self.input.is_empty() {
            &self.default
        } else {
            &self.input
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FormKind {
    GhRepoCreate,
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
        let last = self.fields.len();

        if key.code == KeyCode::Esc || (ctrl && key.code == KeyCode::Char('c')) {
            return FormEvent::Cancel;
        }

        // Directory fields: Tab completes, and suggestions follow typing.
        if let Some(field) = self.fields.get_mut(self.step).filter(|f| f.dir) {
            let completer = self.completer.get_or_insert_with(|| Completer::new(complete::Kind::Dir, false));
            match key.code {
                KeyCode::Tab | KeyCode::BackTab => {
                    if field.input.is_empty() {
                        // Start from the default so Tab refines it instead of starting over.
                        field.input = field.default.clone();
                        completer.update(&field.input, false);
                    }
                    completer.cycle(&mut field.input, key.code == KeyCode::Tab);
                    return FormEvent::None;
                }
                KeyCode::Char('u') if ctrl => {
                    field.input.clear();
                    completer.update("", false);
                    return FormEvent::None;
                }
                KeyCode::Backspace if !field.input.is_empty() => {
                    field.input.pop();
                    completer.update(&field.input, false);
                    return FormEvent::None;
                }
                KeyCode::Char(c) if !ctrl => {
                    field.input.push(c);
                    completer.update(&field.input, false);
                    return FormEvent::None;
                }
                _ => {}
            }
        }
        let before = self.step;
        let event = self.key_general(key, ctrl, last);
        if self.step != before {
            self.completer = None;
        }
        event
    }

    fn key_general(&mut self, key: ratatui::crossterm::event::KeyEvent, ctrl: bool, last: usize) -> FormEvent {
        use ratatui::crossterm::event::KeyCode;
        match key.code {
            KeyCode::Up | KeyCode::BackTab => self.step = self.step.saturating_sub(1),
            KeyCode::Down | KeyCode::Tab => self.step = (self.step + 1).min(last),
            KeyCode::Enter if self.on_confirm_step() => return FormEvent::Submit,
            KeyCode::Enter => self.step += 1,
            _ if self.on_confirm_step() => match key.code {
                KeyCode::Char('y') => return FormEvent::Submit,
                KeyCode::Backspace => self.step -= 1,
                _ => {}
            },
            _ => {
                let field = &mut self.fields[self.step];
                if field.is_choice() {
                    let n = field.options.len();
                    match key.code {
                        KeyCode::Right | KeyCode::Char(' ') => field.choice = (field.choice + 1) % n,
                        KeyCode::Left => field.choice = (field.choice + n - 1) % n,
                        KeyCode::Backspace => self.step = self.step.saturating_sub(1),
                        KeyCode::Char(c) => {
                            if let Some(i) = field.options.iter().position(|o| o.starts_with(c.to_ascii_lowercase())) {
                                field.choice = i;
                            }
                        }
                        _ => {}
                    }
                } else {
                    match key.code {
                        KeyCode::Char('u') if ctrl => field.input.clear(),
                        // Backspace on an empty field steps back, like a wizard.
                        KeyCode::Backspace if field.input.is_empty() => self.step = self.step.saturating_sub(1),
                        KeyCode::Backspace => {
                            field.input.pop();
                        }
                        KeyCode::Char(c) if !ctrl => field.input.push(c),
                        _ => {}
                    }
                }
            }
        }
        FormEvent::None
    }
}
