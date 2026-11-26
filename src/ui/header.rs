//! The top line: where you are, the project, git state, todos and what's running.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
};

use crate::{app::App, config::pretty_path};

use super::{ACCENT, DIM, spinner, truncate, truncate_left};

pub(super) fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    // Right side first: what's running, so the path can be truncated to fit.
    let tab = app.tab();
    let mut right = if let Some(t) = &tab.task {
        let el = t.started.elapsed();
        Line::from(Span::styled(
            format!(" {} {} {:.1}s ", spinner(t), truncate(&t.label, 40), el.as_secs_f32()),
            Style::new().fg(ACCENT),
        ))
    } else if let Some(l) = &tab.last_run {
        let (sym, col) = if l.ok { ("✔", Color::Green) } else { ("✘", Color::Red) };
        Line::from(Span::styled(format!(" {sym} {} ", truncate(&l.label, 40)), Style::new().fg(col)))
    } else {
        Line::default()
    };
    let others = app.tabs.iter().enumerate().filter(|&(i, t)| i != app.tab && t.running()).count();
    if others > 0 {
        right.push_span(Span::styled(format!("+{others} running "), Style::new().fg(DIM)));
    }
    let elsewhere = app.running_elsewhere();
    if elsewhere > 0 {
        right.push_span(Span::styled(format!("▶ {elsewhere} in other projects (J) "), Style::new().fg(DIM)));
    }

    let mut badges = Vec::new();
    if let Some(p) = &app.project {
        badges.push(Span::raw("  "));
        badges.push(Span::styled(
            format!(" {} · {} ", p.kind.label(), p.name),
            Style::new().fg(Color::Black).bg(Color::Yellow),
        ));
    }
    if let Some(g) = &app.git {
        badges.push(Span::raw("  "));
        badges.push(Span::styled(format!(" {}", g.branch), Style::new().fg(Color::Magenta).bold()));
        if g.ahead > 0 {
            badges.push(Span::styled(format!(" ↑{}", g.ahead), Style::new().fg(Color::Green)));
        }
        if g.behind > 0 {
            badges.push(Span::styled(format!(" ↓{}", g.behind), Style::new().fg(Color::Red)));
        }
        if g.dirty() {
            badges.push(Span::styled(" ●", Style::new().fg(Color::Yellow)));
        }
    }
    if app.open_todos > 0 {
        badges.push(Span::raw("  "));
        badges.push(Span::styled(format!("☐ {}", app.open_todos), Style::new().fg(Color::Yellow)));
    }

    let logo = Span::styled(" ◆ MAINFRAME ", Style::new().bg(ACCENT).fg(Color::Black).bold());
    let fixed = logo.width() + 1 + badges.iter().map(Span::width).sum::<usize>() + right.width();
    let path = truncate_left(&pretty_path(&app.browser.cwd), (area.width as usize).saturating_sub(fixed));

    let mut spans = vec![logo, Span::raw(" "), Span::styled(path, Style::new().bold())];
    spans.extend(badges);
    f.render_widget(Line::from(spans), area);
    f.render_widget(right.right_aligned(), area);
}
