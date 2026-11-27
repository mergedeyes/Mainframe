//! The blog post editor: fields on top, the Markdown body, and the preview beside it.

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Position, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Clear, Paragraph},
};

use crate::{
    app::{Ask, Part, PostEditor},
    blog::State,
    line::LineEdit,
    markdown,
};

use super::{ACCENT, DIM, hints, pane, truncate};

/// Below this width the preview doesn't fit next to the text.
const PREVIEW_MIN_WIDTH: u16 = 90;

pub(super) fn draw_post_editor(f: &mut Frame, ed: &mut PostEditor, area: Rect) {
    // Everything but the header and footer rows.
    let rect = Rect { y: area.y + 1, height: area.height.saturating_sub(2), ..area };
    f.render_widget(Clear, rect);
    let (state, color) = match ed.state {
        State::Draft => ("draft", Color::Yellow),
        State::Post => ("post", Color::Green),
        State::Now => ("now", Color::Cyan),
    };
    let title = Line::from(vec![
        Span::styled(format!(" {state} "), Style::new().fg(color).bold()),
        Span::styled(format!("{} ", ed.rel()), Style::new().bold()),
    ]);
    let go = if ed.state == State::Draft { "^G publish" } else { "^G push" };
    let keys = format!(" ^S save · {go} · ^P preview · ^Z ^Y undo · ^B bold · ^E italic · ^K link · Esc close ");
    let block = pane(title, true).title_bottom(hints(&keys));
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    let [title_row, desc_row, rule, body_area, status_row] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    field(f, "title", &ed.title, "required", ed.focus == Part::Title, title_row);
    field(f, "description", &ed.description, "optional, one line", ed.focus == Part::Description, desc_row);
    f.render_widget(Line::from("─".repeat(rule.width as usize)).fg(DIM), rule);

    let show_preview = ed.preview && inner.width >= PREVIEW_MIN_WIDTH;
    let (text_area, preview_area) = if show_preview {
        let [l, sep, r] = Layout::horizontal([Constraint::Percentage(50), Constraint::Length(3), Constraint::Fill(1)])
            .areas(body_area);
        for y in sep.y..sep.bottom() {
            f.render_widget(Line::from(" │ ").fg(DIM), Rect { y, height: 1, ..sep });
        }
        (l, Some(r))
    } else {
        (body_area, None)
    };
    draw_body(f, ed, text_area);
    if let Some(area) = preview_area {
        draw_preview(f, ed, area);
    }
    draw_status(f, ed, status_row);
}

fn field(f: &mut Frame, label: &str, edit: &LineEdit, placeholder: &str, active: bool, area: Rect) {
    const LABEL_W: u16 = 13;
    let label_style = if active { Style::new().fg(ACCENT).bold() } else { Style::new().fg(DIM) };
    let mut spans = vec![Span::styled(format!(" {label:<w$}", w = LABEL_W as usize - 1), label_style)];
    if edit.is_empty() {
        spans.push(Span::styled(placeholder.to_string(), Style::new().fg(DIM).italic()));
    } else {
        let style = if label == "title" { Style::new().bold() } else { Style::new() };
        spans.push(Span::styled(edit.text.clone(), style));
    }
    f.render_widget(Line::from(spans), area);
    if active {
        let x = (area.x + LABEL_W + edit.col() as u16).min(area.right().saturating_sub(1));
        f.set_cursor_position(Position::new(x, area.y));
    }
}

/// The body, wrapped and highlighted, scrolled so the cursor stays in view.
fn draw_body(f: &mut Frame, ed: &mut PostEditor, area: Rect) {
    let (w, h) = (area.width.saturating_sub(2) as usize, area.height as usize);
    if w == 0 || h == 0 {
        return;
    }
    // One column on the left to breathe, one on the right for the cursor at a row's end.
    ed.body.width = w;
    ed.body.height = h;
    let (cy, cx) = ed.body.screen_cursor();
    if cy < ed.scroll {
        ed.scroll = cy;
    } else if cy >= ed.scroll + h {
        ed.scroll = cy + 1 - h;
    }

    let mut rows: Vec<Line> = Vec::with_capacity(h);
    let mut fence = false;
    let mut row = 0;
    for (i, text) in ed.body.lines.iter().enumerate() {
        // Highlighting needs every line (code fences), the rows only the visible ones.
        let segs = markdown::highlight(text, &mut fence);
        let starts = ed.body.rows_of(i);
        if row + starts.len() <= ed.scroll {
            row += starts.len();
            continue;
        }
        for (n, &from) in starts.iter().enumerate() {
            if row >= ed.scroll && rows.len() < h {
                let to = starts.get(n + 1).copied().unwrap_or(text.len());
                let mut spans = vec![Span::raw(" ")];
                spans.extend(markdown::spans_in(text, &segs, from..to, Style::new()));
                rows.push(Line::from(spans));
            }
            row += 1;
        }
        if rows.len() >= h {
            break;
        }
    }
    if ed.body.lines.iter().all(String::is_empty) && ed.focus != Part::Body {
        rows = vec![Line::from(" write here — Markdown works".fg(DIM).italic())];
    }
    f.render_widget(Paragraph::new(rows), area);
    if ed.focus == Part::Body && ed.ask.is_none() {
        let x = area.x + 1 + cx as u16;
        let y = area.y + (cy - ed.scroll) as u16;
        f.set_cursor_position(Position::new(x.min(area.right().saturating_sub(1)), y));
    }
}

/// The rendered post, scrolled so the part being written is in view.
fn draw_preview(f: &mut Frame, ed: &PostEditor, area: Rect) {
    let w = area.width.saturating_sub(1) as usize;
    let mut lines = Vec::new();
    let title = ed.title.text.trim();
    if !title.is_empty() {
        let style = Style::new().fg(ACCENT).add_modifier(Modifier::BOLD);
        lines.push(Line::from(Span::styled(truncate(title, w), style)));
    }
    let desc = ed.description.text.trim();
    if !desc.is_empty() {
        lines.push(Line::from(Span::styled(truncate(desc, w), Style::new().fg(DIM).italic())));
    }
    if !lines.is_empty() {
        lines.push(Line::default());
    }
    let head = lines.len();
    let (body, anchors) = markdown::preview(&ed.body.text(), w);
    lines.extend(body);

    // The block the cursor is in starts a third of the way down, once the text is that long.
    let (cursor_line, _) = ed.body.cursor();
    let at = anchors.iter().rev().find(|(src, _)| *src <= cursor_line).map_or(0, |(_, row)| head + row);
    let h = area.height as usize;
    let max = lines.len().saturating_sub(h);
    let scroll = if ed.focus == Part::Body { at.saturating_sub(h / 3).min(max) } else { 0 };
    f.render_widget(Paragraph::new(lines).scroll((scroll as u16, 0)), area);
}

fn draw_status(f: &mut Frame, ed: &PostEditor, area: Rect) {
    if let Some(ask) = &ed.ask {
        let msg = match ask {
            Ask::Publish(q) => q.clone(),
            Ask::Discard => "Changed outside Mainframe: close and drop your edits here? [y/N]".into(),
        };
        f.render_widget(
            Line::from(format!(" {msg} ")).style(Style::new().bg(Color::Yellow).fg(Color::Black).bold()),
            area,
        );
        return;
    }
    let words = ed.body.words();
    let mut spans =
        vec![Span::styled(format!(" {words} word{} ", if words == 1 { "" } else { "s" }), Style::new().fg(DIM))];
    if ed.conflict {
        spans
            .push(Span::styled("· ⚠ changed outside Mainframe: ^R reload · ^S overwrite", Style::new().fg(Color::Red)));
    } else if ed.dirty {
        spans.push(Span::styled("· ● unsaved", Style::new().fg(Color::Yellow)));
    } else if ed.saved.is_some() {
        spans.push(Span::styled("· saved", Style::new().fg(Color::Green)));
    }
    if ed.preview && area.width < PREVIEW_MIN_WIDTH {
        spans.push(Span::styled(" · widen the window for the preview", Style::new().fg(DIM)));
    }
    f.render_widget(Line::from(spans), area);
}
