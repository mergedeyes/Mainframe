//! The file list and the preview beside it.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{ListItem, Paragraph},
};

use crate::{
    app::{App, Focus, Mode},
    browser::{Preview, Sort, human_size},
    install,
};

use super::{DIM, list, pane, truncate};

pub(super) fn draw_browser(f: &mut Frame, app: &mut App, area: Rect) {
    let b = &app.browser;
    let mut title = vec![Span::styled(
        format!(" {} ", b.cwd.file_name().map_or("/".into(), |n| n.to_string_lossy().into_owned())),
        Style::new().bold(),
    )];
    if !b.filter.is_empty() || matches!(app.mode, Mode::Filter(_)) {
        title.push(Span::styled(format!("/{} ", b.filter), Style::new().fg(Color::Yellow)));
    }
    if b.sort != Sort::Name {
        title.push(Span::styled(format!("↓ {} ", b.sort.label()), Style::new().fg(DIM)));
    }
    let count = format!(" {}/{} ", b.state.selected().map_or(0, |i| i + 1), b.visible.len());
    let mut block =
        pane(Line::from(title), app.focus == Focus::Browser).title_bottom(Line::from(count).right_aligned().fg(DIM));
    if !b.marked.is_empty() {
        block = block.title_bottom(
            Line::from(format!(" {} marked · y d s u act on them · Esc clears ", b.marked.len())).fg(Color::Magenta),
        );
    } else if b.hidden_count > 0 {
        block = block.title_bottom(Line::from(format!(" {} hidden · . to show ", b.hidden_count)).fg(DIM));
    }
    let inner_w = area.width.saturating_sub(4) as usize; // borders + highlight symbol

    let items: Vec<ListItem> = b
        .visible
        .iter()
        .map(|&i| {
            let e = &b.entries[i];
            let (mark, mark_col) =
                app.git.as_ref().and_then(|g| g.mark(&e.path, e.is_dir)).unwrap_or((" ", Color::Reset));
            let style = if e.is_dir {
                Style::new().fg(Color::Blue).bold()
            } else if e.is_symlink {
                Style::new().fg(Color::Cyan)
            } else if e.is_exec {
                Style::new().fg(Color::Green)
            } else {
                Style::new()
            };
            let style = if e.name.starts_with('.') { style.add_modifier(Modifier::DIM) } else { style };
            let marked = b.marked.contains(&e.path);
            let style = if marked { style.fg(Color::Magenta).bold() } else { style };
            let name = if e.is_dir { format!("{}/", e.name) } else { e.name.clone() };
            let size = match (b.sort, e.modified) {
                (Sort::Newest, Some(t)) => install::age(t),
                _ if e.is_dir => String::new(),
                _ => human_size(e.size),
            };
            let name_w = inner_w.saturating_sub(size.len() + 4);
            let name = truncate(&name, name_w);
            let pad = inner_w.saturating_sub(3 + name.chars().count() + size.len());
            ListItem::new(Line::from(vec![
                Span::styled(mark, Style::new().fg(mark_col).bold()),
                Span::styled(if marked { "+ " } else { "  " }, Style::new().fg(Color::Magenta).bold()),
                Span::styled(name, style),
                Span::raw(" ".repeat(pad)),
                Span::styled(size, Style::new().fg(DIM)),
            ]))
        })
        .collect();

    app.browser_height = area.height.saturating_sub(2);
    if let Some(err) = &b.error {
        f.render_widget(Paragraph::new(err.clone().red()).block(block), area);
        return;
    }
    if items.is_empty() {
        let msg = if b.filter.is_empty() { "(empty)" } else { "(no matches)" };
        f.render_widget(Paragraph::new(msg.fg(DIM)).block(block), area);
        return;
    }
    f.render_stateful_widget(list(items).block(block), area, &mut app.browser.state);
}

pub(super) fn draw_preview(f: &mut Frame, app: &App, area: Rect) {
    let entry = app.browser.selected();
    let mut title = vec![Span::styled(entry.map_or(String::new(), |e| format!(" {} ", e.name)), Style::new().fg(DIM))];
    if matches!(app.preview, Some(Preview::Diff(_))) {
        title.push(Span::styled("diff · V for contents ", Style::new().fg(Color::Yellow)));
    }
    let block = pane(Line::from(title), false);
    let height = area.height.saturating_sub(2) as usize;

    let lines: Vec<Line> = match &app.preview {
        None => vec![],
        Some(Preview::Text(text)) => {
            let w = text.len().min(height).to_string().len();
            text.iter()
                .take(height)
                .enumerate()
                .map(|(i, l)| {
                    Line::from(vec![
                        Span::styled(format!("{:>w$} │ ", i + 1), Style::new().fg(DIM)),
                        Span::raw(l.clone()),
                    ])
                })
                .collect()
        }
        Some(Preview::Diff(text)) => text
            .iter()
            .take(height)
            .map(|l| {
                let style = match l.as_bytes().first() {
                    Some(b'+') => Style::new().fg(Color::Green),
                    Some(b'-') => Style::new().fg(Color::Red),
                    Some(b'@') => Style::new().fg(Color::Cyan),
                    Some(b' ') => Style::new(),
                    _ => Style::new().fg(DIM),
                };
                Line::from(Span::styled(l.clone(), style))
            })
            .collect(),
        Some(Preview::Dir(items)) if items.is_empty() => vec![Line::from("(empty)".fg(DIM))],
        Some(Preview::Dir(items)) => items
            .iter()
            .take(height)
            .map(|(n, dir)| if *dir { Line::from(format!("{n}/").blue().bold()) } else { Line::from(n.clone()) })
            .collect(),
        Some(Preview::Binary(size)) => {
            let mut v = vec![Line::from(format!("binary file · {}", human_size(*size)).fg(DIM))];
            if entry.is_some_and(|e| e.is_exec) {
                v.push(Line::default());
                v.push(Line::from("executable — r to run, i to install".fg(Color::Green)));
            }
            v
        }
        Some(Preview::Error(e)) => vec![Line::from(e.clone().red())],
    };
    f.render_widget(Paragraph::new(lines).block(block), area);
}
