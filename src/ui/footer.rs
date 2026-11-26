//! The bottom line: prompts, confirmations, status messages, or the most used keys.

use ratatui::{
    Frame,
    layout::{Position, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, BorderType, Clear, Paragraph},
};

use crate::{
    app::{App, Level, Mode},
    complete::Completer,
    line::LineEdit,
};

use super::{ACCENT, DIM, SELECTED, truncate};

pub(super) fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    match &app.mode {
        Mode::Prompt { kind, input, completer, .. } => {
            let label = format!(" {}: ", kind.label());
            let label_w = label.chars().count() as u16;
            input_line(f, label, input, "", area);
            if let Some(c) = completer {
                draw_completions(f, c, area.x + label_w.saturating_sub(1), area.y, true);
            }
            return;
        }
        Mode::Confirm { msg, .. } => {
            f.render_widget(
                Line::from(format!(" {msg} ")).style(Style::new().bg(Color::Yellow).fg(Color::Black).bold()),
                area,
            );
            return;
        }
        Mode::Search(edit) => return input_line(f, " search output: ".into(), edit, "   Enter keep · Esc clear", area),
        Mode::Filter(edit) => return input_line(f, " filter: ".into(), edit, "   Enter keep · Esc clear", area),
        _ => {}
    }

    if let Some((msg, level, _)) = &app.status {
        let col = match level {
            Level::Info => ACCENT,
            Level::Ok => Color::Green,
            Level::Err => Color::Red,
        };
        f.render_widget(Line::from(format!(" {msg}").fg(col)), area);
        return;
    }

    let key = |k: &'static str| Span::styled(k, Style::new().fg(ACCENT).bold());
    let txt = |t: &'static str| Span::styled(t, Style::new().fg(DIM));
    let mut spans = vec![Span::raw(" ")];
    if let Some(hint) = &app.run_hint {
        spans.extend([
            key("r "),
            Span::styled(format!("{} ", truncate(hint, 40)), Style::new().fg(Color::Green)),
            txt(" "),
        ]);
    }
    spans.extend([
        key("e"),
        txt(" edit  "),
        key("b"),
        txt(" build  "),
        key("i"),
        txt(" install  "),
        key("o"),
        txt(" code  "),
        key("g"),
        txt(" git  "),
        key(":"),
        txt(" cmd  "),
        key("?"),
        txt(" help"),
    ]);
    f.render_widget(Line::from(spans), area);
}

/// `label` and the text being typed, the cursor in it, a dim `tail` after it.
pub(super) fn input_line(f: &mut Frame, label: String, edit: &LineEdit, tail: &'static str, area: Rect) {
    let x = area.x + label.chars().count() as u16 + edit.col() as u16;
    f.render_widget(
        Line::from(vec![
            Span::styled(label, Style::new().fg(ACCENT).bold()),
            Span::raw(edit.text.clone()),
            Span::styled(tail, Style::new().fg(DIM)),
        ]),
        area,
    );
    f.set_cursor_position(Position::new(x.min(area.right().saturating_sub(1)), area.y));
}

/// Suggestion list anchored to an input line: above it (footer prompt) or below it (form field).
pub(super) fn draw_completions(f: &mut Frame, c: &Completer, x: u16, anchor_y: u16, above: bool) {
    const ROWS: usize = 10;
    let n = c.items.len();
    if n == 0 {
        return;
    }
    let screen = f.area();
    let shown = n.min(ROWS);
    // Scroll the window so the selected item stays visible.
    let first = c.selected.map_or(0, |s| (s + 1).saturating_sub(ROWS)).min(n - shown);
    let items = &c.items[first..first + shown];

    let widest = items.iter().map(|i| i.display.chars().count()).max().unwrap_or(0);
    let x = x.min(screen.right().saturating_sub(24));
    let w = (widest as u16 + 4).clamp(24, screen.right().saturating_sub(x).max(24));
    let h = shown as u16 + 2;
    let y = if above { anchor_y.saturating_sub(h) } else { (anchor_y + 1).min(screen.bottom().saturating_sub(h)) };
    let rect = Rect::new(x, y, w, h).intersection(screen);

    let lines: Vec<Line> = items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let mut style = if item.is_dir { Style::new().fg(Color::Blue).bold() } else { Style::new() };
            if c.selected == Some(first + i) {
                style = style.bg(SELECTED).add_modifier(Modifier::REVERSED);
            }
            Line::from(Span::styled(format!(" {} ", truncate(&item.display, w as usize - 4)), style))
        })
        .collect();

    let count = match c.selected {
        Some(s) => format!(" {}/{n} ", s + 1),
        None if n > shown => format!(" {n} matches · Tab "),
        None => " Tab ".into(),
    };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(DIM))
        .title_bottom(Line::from(count).right_aligned());
    f.render_widget(Clear, rect);
    f.render_widget(Paragraph::new(lines).block(block), rect);
}
