//! `C`: build output, dependencies and caches to clear.

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Style, Stylize},
    text::{Line, Span},
    widgets::{ListItem, Paragraph},
};

use crate::app::CleanupPicker;

use super::{ACCENT, DIM, list, popup, popup_pane, spin_frame, truncate};

pub(super) fn draw_cleanup(f: &mut Frame, picker: &mut CleanupPicker, area: Rect) {
    let title = Line::from(format!(" clean up {} ", picker.title).bold());
    if picker.measuring() {
        let rect = popup(area, 60, 3);
        let spin = spin_frame(picker.started);
        let inner = popup_pane(f, rect, title, " Esc close ");
        f.render_widget(
            Line::from(vec![Span::styled(format!(" {spin} "), Style::new().fg(ACCENT)), "measuring…".fg(DIM)]),
            inner,
        );
        return;
    }

    let label = |s: &'static str| Span::styled(format!("  {s:<6}"), Style::new().fg(DIM));
    let mut details = Vec::new();
    if let Some(item) = picker.selected() {
        details.push(Line::from(vec![label("what"), Span::raw(item.note)]));
        if let Some(u) = item.size.filter(|u| u.disk.is_some()) {
            details.push(Line::from(vec![
                label("size"),
                Span::raw(u.long()),
                Span::styled("  btrfs compression, estimated from samples", Style::new().fg(DIM)),
            ]));
        }
        details.push(Line::from(vec![label("runs"), Span::styled(truncate(&runs(item), 90), Style::new().fg(DIM))]));
    }
    if !picker.marked.is_empty() {
        let chosen = picker.chosen();
        details.push(Line::from(vec![
            label("marked"),
            Span::styled(
                format!("{} · {}", chosen.len(), crate::cleanup::total(&chosen).long()),
                Style::new().fg(Color::Yellow),
            ),
        ]));
    }

    let items = picker.items();
    let rows = items.len() as u16;
    let rect = popup(area, 100, rows + details.len() as u16 + 4);
    let inner = popup_pane(f, rect, title, " Space mark · a mark all like this · Enter clean · Esc close ");
    let [list_area, _, details_area] =
        Layout::vertical([Constraint::Length(rows), Constraint::Length(1), Constraint::Fill(1)]).areas(inner);

    let label_w = items.iter().map(|i| i.title().chars().count()).max().unwrap_or(0).min(36);
    let rows: Vec<ListItem> = items
        .iter()
        .enumerate()
        .map(|(n, i)| {
            let mark = if picker.marked.contains(&n) { "● " } else { "  " };
            let size = i.size.map_or_else(|| "–".to_string(), |u| u.short());
            ListItem::new(Line::from(vec![
                Span::styled(mark, Style::new().fg(Color::Yellow)),
                Span::styled(
                    format!("{:<label_w$}", truncate(&i.title(), label_w)),
                    Style::new().fg(Color::Yellow).bold(),
                ),
                Span::raw(format!("{size:>9}  ")),
                Span::styled(i.location(), Style::new().fg(DIM)),
            ]))
        })
        .collect();
    f.render_stateful_widget(list(rows), list_area, &mut picker.state);
    f.render_widget(Paragraph::new(details), details_area);
}

/// What clearing `item` does, as one line.
fn runs(item: &crate::cleanup::Item) -> String {
    let shell = crate::shell::Shell::default();
    item.jobs().iter().map(|j| j.display(&shell)).collect::<Vec<_>>().join(" && ")
}
