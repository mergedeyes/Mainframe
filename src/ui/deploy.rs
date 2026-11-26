//! `D`: deploy targets and what a deploy will do.

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Style, Stylize},
    text::{Line, Span},
    widgets::{ListItem, Paragraph},
};

use crate::{app::DeployPicker, config::pretty_path, install};

use super::{ACCENT, DIM, list, popup, popup_pane, truncate};

pub(super) fn draw_deploy(f: &mut Frame, picker: &mut DeployPicker, area: Rect) {
    let label = |s: &'static str| Span::styled(format!("  {s:<10}"), Style::new().fg(DIM));
    let mut details = Vec::new();
    if let Some(d) = picker.selected() {
        for (i, step) in d.summary(&picker.root, picker.keep_build).into_iter().enumerate() {
            details.push(Line::from(vec![label(if i == 0 { "does" } else { "" }), Span::raw(step)]));
        }
        let artifact = d.artifact_path(&picker.root);
        let built = match std::fs::metadata(&artifact).and_then(|m| m.modified()) {
            // The file in the project folder isn't what gets deployed then.
            _ if !d.from.trim().is_empty() => {
                Span::styled(format!("built from {} on each deploy", d.from.trim()), Style::new().fg(Color::Green))
            }
            Ok(t) => Span::styled(format!("built {}", install::age(t)), Style::new().fg(Color::Green)),
            Err(_) => Span::styled("not built yet", Style::new().fg(Color::Yellow)),
        };
        details.push(Line::from(vec![label("artifact"), built]));
        let how = match (d.interactive, d.sudo) {
            (true, _) => "in the terminal — ssh and sudo may ask for passwords",
            (false, true) => "in the output pane — needs an ssh key and sudo without password",
            (false, false) => "in the output pane — needs an ssh key without prompt",
        };
        details.push(Line::from(vec![label("runs"), Span::styled(how, Style::new().fg(DIM))]));
    }
    details.push(Line::from(vec![
        label("settings"),
        Span::styled(pretty_path(&crate::settings::file_for(&picker.root)), Style::new().fg(DIM)),
    ]));
    if picker.broken {
        details.push(Line::from(vec![
            label(""),
            Span::styled(
                "doesn't parse, so no targets show — E opens it, the status line says where",
                Style::new().fg(Color::Red),
            ),
        ]));
    }

    let rect = popup(area, 110, picker.targets.len() as u16 + details.len() as u16 + 4);
    let title = Line::from(format!(" deploy {} ", picker.title).bold());
    let keys = " Enter deploy · u roll back · l backups · a add · e edit · x delete · E file · Esc close ";
    let inner = popup_pane(f, rect, title, keys);
    let [list_area, _, details_area] =
        Layout::vertical([Constraint::Length(picker.targets.len() as u16), Constraint::Length(1), Constraint::Fill(1)])
            .areas(inner);

    let items: Vec<ListItem> = picker
        .targets
        .iter()
        .map(|d| {
            let mut spans = vec![
                Span::styled(format!("{:<16}", truncate(&d.name, 16)), Style::new().fg(Color::Yellow).bold()),
                Span::raw(format!("{:<30} ", truncate(&d.host, 30))),
                Span::styled(d.remote_path.clone(), Style::new().fg(DIM)),
            ];
            if !d.key.is_empty() {
                spans.push(Span::styled(format!("  [{}]", d.key), Style::new().fg(ACCENT)));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();
    f.render_stateful_widget(list(items), list_area, &mut picker.state);
    f.render_widget(Paragraph::new(details), details_area);
}
