//! The step-by-step forms (new project, deploy target, release, GitHub repo).

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Position, Rect},
    style::{Color, Style, Stylize},
    text::{Line, Span},
    widgets::{Clear, Paragraph, Wrap},
};

use crate::form::{Form, FormKind};

use super::{ACCENT, DIM, footer::draw_completions, pane, popup};

pub(super) fn draw_form(f: &mut Frame, form: &Form, area: Rect) {
    // One line per step; long ones wrap.
    let (steps, verb, heading, leads) = match &form.kind {
        FormKind::GhRepoCreate => (vec![crate::git::gh_repo_create_cmd(form)], "create", "command", ("  $  ", "  && ")),
        FormKind::Release(ctx) => (crate::release::steps(form, ctx), "release", "command", ("  $  ", "  && ")),
        FormKind::Deploy { root, keep_build, .. } => (
            crate::app::deploy_from_form(form).summary(root, *keep_build),
            "save",
            "D then Enter will",
            ("  ·  ", "  ·  "),
        ),
        FormKind::NewProject => (crate::scaffold::steps(form), "create", "will", ("  ·  ", "  ·  ")),
    };
    let w = 92.min(area.width.saturating_sub(2));
    let mut cmd = vec![Line::from(format!("  {heading}").fg(DIM))];
    for (i, step) in steps.iter().enumerate() {
        cmd.extend(hang_wrap(if i == 0 { leads.0 } else { leads.1 }, step, w.saturating_sub(7) as usize));
    }
    let rect = popup(area, w, form.fields.len() as u16 + 6 + cmd.len() as u16);
    f.render_widget(Clear, rect);
    let block = pane(Line::from(form.title.clone().bold()), true);
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let [fields_area, _, note_area, cmd_area, button_area] = Layout::vertical([
        Constraint::Length(form.fields.len() as u16),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(inner);

    const LABEL_W: usize = 14;
    for (i, field) in form.fields.iter().enumerate() {
        let active = i == form.step;
        let row = Rect { y: fields_area.y + i as u16, height: 1, ..fields_area };
        let mut spans = vec![
            Span::styled(if active { "▶ " } else { "  " }, Style::new().fg(ACCENT)),
            Span::styled(
                format!("{:<LABEL_W$}", field.label),
                if active { Style::new().fg(ACCENT).bold() } else { Style::new().fg(DIM) },
            ),
        ];
        if field.is_choice() {
            for (j, opt) in field.options.iter().enumerate() {
                let style = match (j == field.choice, active) {
                    (true, true) => Style::new().bg(ACCENT).fg(Color::Black).bold(),
                    (true, false) => Style::new().bold(),
                    _ => Style::new().fg(DIM),
                };
                spans.push(Span::styled(format!(" {opt} "), style));
                spans.push(Span::raw(" "));
            }
        } else if field.input.is_empty() {
            let shown = if field.default.is_empty() { "—".to_string() } else { field.default.clone() };
            spans.push(Span::styled(shown, if active { Style::new().fg(DIM).italic() } else { Style::new() }));
            if active {
                f.set_cursor_position(Position::new(row.x + 2 + LABEL_W as u16, row.y));
            }
        } else {
            spans.push(Span::styled(field.input.text.clone(), Style::new().fg(Color::Yellow).bold()));
            if active {
                let x = row.x + 2 + LABEL_W as u16 + field.input.col() as u16;
                f.set_cursor_position(Position::new(x.min(row.right().saturating_sub(1)), row.y));
            }
        }
        if active {
            spans.push(Span::styled(format!("   {}", field.hint), Style::new().fg(DIM)));
        }
        f.render_widget(Line::from(spans), row);
    }

    if let Some(note) = &form.note {
        f.render_widget(Line::from(format!("  ⚠ {note}").yellow()), note_area);
    }

    f.render_widget(Paragraph::new(cmd).wrap(Wrap { trim: false }), cmd_area);

    let button = if form.on_confirm_step() {
        Line::from(vec![
            Span::styled(format!("  ▶ Enter: {verb}  "), Style::new().bg(Color::Green).fg(Color::Black).bold()),
            Span::styled("   ↑ back · Esc cancel", Style::new().fg(DIM)),
        ])
    } else {
        Line::from("  Enter accept & next · type to replace · ←/→ edit the default · ↑/↓ field · Esc cancel".fg(DIM))
    };
    f.render_widget(button, button_area);

    let on_dir_field = form.fields.get(form.step).is_some_and(|fl| fl.dir);
    if let Some(c) = form.completer.as_ref().filter(|_| on_dir_field) {
        draw_completions(f, c, fields_area.x + 1 + LABEL_W as u16, fields_area.y + form.step as u16, false);
    }
}

/// Word-wraps `text` to `width` behind `lead`, continuation lines indented under the text.
fn hang_wrap(lead: &'static str, text: &str, width: usize) -> Vec<Line<'static>> {
    let mut rows = vec![String::new()];
    for word in text.split(' ') {
        let row = rows.last_mut().expect("never empty");
        if !row.is_empty() && row.chars().count() + 1 + word.chars().count() > width.max(1) {
            rows.push(word.to_string());
        } else {
            if !row.is_empty() {
                row.push(' ');
            }
            row.push_str(word);
        }
    }
    let indent = " ".repeat(lead.chars().count());
    rows.into_iter()
        .enumerate()
        .map(|(i, r)| {
            let lead = if i == 0 { lead.to_string() } else { indent.clone() };
            Line::from(vec![Span::styled(lead, Style::new().fg(DIM)), Span::raw(r)])
        })
        .collect()
}
