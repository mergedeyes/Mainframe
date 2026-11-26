//! The output pane: one tab per task, search matches and `file:line` jumps.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style, Stylize},
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::app::{App, Focus};

use super::{ACCENT, DIM, pane, spinner, truncate};

pub(super) fn draw_output(f: &mut Frame, app: &mut App, area: Rect) {
    let height = area.height.saturating_sub(2) as usize;
    let focused = app.focus == Focus::Output;

    // One tab shows just its state; several turn the title into a tab bar.
    let mut title = vec![Span::styled(" output ", Style::new().bold())];
    let tab = app.tab();
    if app.tabs.len() > 1 {
        title.clear();
        for (i, t) in app.tabs.iter().enumerate() {
            let state = match (&t.task, &t.last_run) {
                (Some(task), _) => Span::styled(spinner(task), Style::new().fg(ACCENT)),
                (None, Some(l)) if l.ok => Span::styled("✔", Style::new().fg(Color::Green)),
                (None, Some(_)) => Span::styled("✘", Style::new().fg(Color::Red)),
                (None, None) => Span::raw("·"),
            };
            let style = if i == app.tab { Style::new().bold().reversed() } else { Style::new().fg(DIM) };
            title.extend([
                Span::raw(" "),
                state,
                Span::styled(format!(" {} {} ", i + 1, truncate(&t.label, 20)), style),
            ]);
            if t.watch.is_some() {
                title.push(Span::styled("⟳", Style::new().fg(Color::Magenta)));
            }
        }
        title.push(Span::raw(" "));
    }
    if let Some(t) = &tab.task {
        title.push(Span::styled(
            format!("{} running {:.1}s ", spinner(t), t.started.elapsed().as_secs_f32()),
            Style::new().fg(ACCENT),
        ));
    } else if let Some(l) = &tab.last_run {
        let col = if l.ok { Color::Green } else { Color::Red };
        title.push(Span::styled(format!("{} {} ", if l.ok { "✔" } else { "✘" }, l.detail), Style::new().fg(col)));
    }

    if tab.watch.is_some() {
        title.push(Span::styled("⟳ watching ", Style::new().fg(Color::Magenta)));
    }
    let out = &mut app.tabs[app.tab].output;
    if !out.search.is_empty() {
        let at = out.match_cursor.map_or(0, |c| c + 1);
        title
            .push(Span::styled(format!("/{} {at}/{} ", out.search, out.matches.len()), Style::new().fg(Color::Yellow)));
    } else if !out.locs.is_empty() {
        title.push(Span::styled(format!("{} file:line · n N jump ", out.locs.len()), Style::new().fg(Color::Yellow)));
    }
    out.height = height;
    // The pane may have shrunk (resize, zoom off) since the last scroll.
    out.scroll = out.scroll.min(out.max_scroll());

    let mut block = pane(Line::from(title), focused);
    if out.scroll > 0 {
        block = block.title_bottom(
            Line::from(format!(" ↑ {} lines up · End to follow ", out.scroll)).right_aligned().fg(Color::Yellow),
        );
    } else if focused {
        let hint = if out.selected_loc().is_some() {
            " Enter edit · o VS Code · n N next/prev · Tab back "
        } else {
            " ↑↓ scroll · / search · ^S save · [ ] tabs · z zoom · Tab back "
        };
        block = block.title_bottom(Line::from(hint).right_aligned().fg(DIM));
    }

    let end = out.lines.len().saturating_sub(out.scroll);
    let start = end.saturating_sub(height);
    let picked = out.selected_loc().map(|(i, _)| *i);
    let current = out.current_match();
    let lines: Vec<Line> = (start..end)
        .map(|i| {
            let bg = if current == Some(i) {
                Some(Color::Rgb(110, 85, 0))
            } else if picked == Some(i) {
                Some(Color::DarkGray)
            } else if out.matches.binary_search(&i).is_ok() {
                Some(Color::Rgb(50, 42, 10))
            } else {
                None
            };
            match bg {
                Some(bg) => out.lines[i].clone().patch_style(Style::new().bg(bg)),
                None => out.lines[i].clone(),
            }
        })
        .collect();
    f.render_widget(Paragraph::new(lines).block(block), area);
}
