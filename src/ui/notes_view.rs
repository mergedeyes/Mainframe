//! `M`: a project's notes and todos.

use ratatui::{
    Frame,
    layout::{Position, Rect},
    style::{Color, Style, Stylize},
    text::{Line, Span},
    widgets::{Clear, ListItem, Paragraph},
};

use crate::{app::NotesView, config::pretty_path, notes::Item};

use super::{ACCENT, DIM, SELECTED, hints, list, pane, popup};

pub(super) fn draw_notes(f: &mut Frame, view: &mut NotesView, area: Rect) {
    let rect = popup(area, 100, (view.notes.items.len() as u16 + 4).max(10));
    f.render_widget(Clear, rect);
    let (open, done) = (view.notes.open(), view.notes.done());
    let mut title = vec![Span::styled(format!(" notes · {} ", view.title), Style::new().bold())];
    if open + done > 0 {
        title.push(Span::styled(format!("{open} open · {done} done "), Style::new().fg(DIM)));
    }
    let keys = if view.editing.is_some() {
        " Enter save · Esc cancel "
    } else {
        " a todo  n note  ⏎ edit  ␣ check  t ⇄note  d del  u undo  J K move  C clear checked  E editor "
    };
    let close = if view.editing.is_some() { "" } else { " Esc close " };
    let block =
        pane(Line::from(title), true).title(Line::from(close).right_aligned().fg(DIM)).title_bottom(hints(keys));
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    if view.notes.items.is_empty() {
        f.render_widget(
            Paragraph::new(vec![
                Line::default(),
                Line::from("  nothing here yet".fg(DIM)),
                Line::default(),
                Line::from(vec![
                    "  a".fg(ACCENT).bold(),
                    " add a todo   ".fg(DIM),
                    "n".fg(ACCENT).bold(),
                    " add a note   ".fg(DIM),
                    "E".fg(ACCENT).bold(),
                    " write in the editor".fg(DIM),
                ]),
                Line::default(),
                Line::from(format!("  saved in {}", pretty_path(&view.notes.path)).fg(DIM)),
            ]),
            inner,
        );
        return;
    }

    let editing = view.editing.as_ref().map(|e| e.index);
    let items: Vec<ListItem> = view
        .notes
        .items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let (lead, lead_style) = match item {
                Item::Todo { done: false, indent, .. } => (format!("{indent}☐ "), Style::new().fg(Color::Yellow)),
                Item::Todo { done: true, indent, .. } => (format!("{indent}☑ "), Style::new().fg(Color::Green)),
                Item::Note(_) => ("  ".into(), Style::new()),
            };
            let text = match &view.editing {
                Some(e) if editing == Some(i) => Span::raw(e.input.text.clone()),
                _ => match item {
                    Item::Todo { done: true, text, .. } => {
                        Span::styled(text.clone(), Style::new().fg(DIM).crossed_out())
                    }
                    Item::Todo { text, .. } => Span::raw(text.clone()),
                    Item::Note(text) if text.starts_with('#') => {
                        Span::styled(text.clone(), Style::new().fg(ACCENT).bold())
                    }
                    Item::Note(text) => Span::styled(text.clone(), Style::new().fg(Color::Gray)),
                },
            };
            ListItem::new(Line::from(vec![Span::styled(lead, lead_style), text]))
        })
        .collect();
    // Not bold: crossed-out done todos stay readable when selected.
    let list = list(items).highlight_style(Style::new().bg(SELECTED));
    f.render_stateful_widget(list, inner, &mut view.state);

    // The cursor sits in the line being edited.
    if let (Some(e), Some(sel)) = (&view.editing, view.state.selected()) {
        let row = sel.saturating_sub(view.state.offset()) as u16;
        let lead = match &view.notes.items[e.index] {
            Item::Todo { indent, .. } => indent.chars().count() + 2,
            Item::Note(_) => 2,
        };
        let x = inner.x + 2 + lead as u16 + e.input.col() as u16;
        f.set_cursor_position(Position::new(x.min(inner.right().saturating_sub(1)), inner.y + row));
    }
}
