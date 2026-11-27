//! `i`: the install picker and the review of desktop entries it changes.

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Style, Stylize},
    text::{Line, Span},
    widgets::{ListItem, Paragraph},
};

use crate::{
    app::{AfterReview, DesktopReview, InstallPicker},
    browser::human_size,
    config::pretty_path,
    desktop::{self, Change, Status as DesktopStatus},
    install::{self, Installed},
};

use super::{ACCENT, DIM, list, popup, popup_pane, truncate};

/// "pulls, then " in front of what an install does, when `p` started it.
fn pulls(before: &Option<Vec<crate::cmd::Step>>) -> &'static str {
    match before {
        Some(b) if b.iter().any(|s| s.program == "cargo") => "pulls, rebuilds, then ",
        Some(_) => "pulls, then ",
        None => "",
    }
}

pub(super) fn draw_install(f: &mut Frame, picker: &mut InstallPicker, area: Rect) {
    let rect = popup(area, 120, picker.len() as u16 + 14 + picker.behind.is_some() as u16);
    let title = Line::from(format!(" install from {} ", pretty_path(&picker.root)).bold());
    let keys = " Enter install · p pull, then install · d desktop entry · r run · u remove shadowing copy · Esc close ";
    let inner = popup_pane(f, rect, title, keys);
    let [warning, head, list_area, details] = Layout::vertical([
        Constraint::Length(picker.behind.is_some() as u16),
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(8),
    ])
    .areas(inner);
    if let Some(behind) = &picker.behind {
        f.render_widget(
            Line::from(format!(" ⚠ {behind} — Enter installs the old version, p pulls the new commits first"))
                .fg(Color::Red)
                .bold(),
            warning,
        );
    }

    let row = |name: &str, kind: &str, arch: &str, size: &str, age: &str| {
        format!(
            "{:<22} {:<16} {:<20} {:>8}  {:<10}",
            truncate(name, 22),
            truncate(kind, 16),
            truncate(arch, 20),
            size,
            age
        )
    };
    f.render_widget(
        Line::from(format!("  {}  status", row("name", "built as", "arch", "size", "built"))).fg(DIM),
        head,
    );

    let mut items = Vec::new();
    if picker.cargo {
        items.push(ListItem::new(Line::from(vec![
            Span::styled("⟳ cargo install --path .", Style::new().fg(Color::Yellow).bold()),
            Span::styled(
                format!("   build release from source into {}", pretty_path(&picker.install_dir)),
                Style::new().fg(DIM),
            ),
        ])));
    }
    for c in &picker.items {
        let mut spans = vec![
            Span::raw(row(&c.name, &c.kind, &c.arch, &human_size(c.size), &install::age(c.modified))),
            Span::raw("  "),
        ];
        match c.installed {
            Installed::Same => spans.push("✓ installed".green()),
            Installed::Newer => spans.push("↑ newer than installed binary".yellow()),
            Installed::Older => spans.push("↓ older than installed binary".fg(DIM)),
            Installed::No => {}
        }
        if c.shadowed_by.is_some() {
            // The installed copy never runs; launchers and shells pick the other one.
            spans.push("  ⚠ shadowed".fg(Color::Yellow));
        }
        if c.stale {
            spans.push("  ⚠ stale".fg(Color::LightRed));
        }
        items.push(ListItem::new(Line::from(spans)));
    }
    f.render_stateful_widget(list(items), list_area, &mut picker.state);

    let label = |s: &'static str| Span::styled(format!("{s:<10}"), Style::new().fg(DIM));
    let mut lines = match picker.selected() {
        Some(c) => vec![
            Line::from(vec![label("path"), Span::raw(c.rel.clone())]),
            Line::from(vec![
                label("target"),
                Span::raw(c.triple.clone().unwrap_or_else(|| "host".into())),
                Span::styled(format!("  ({})", c.arch), Style::new().fg(DIM)),
            ]),
            Line::from(vec![
                label("installs"),
                Span::raw(pretty_path(&c.dest)),
                Span::styled(
                    match c.installed {
                        Installed::No => "",
                        Installed::Same => "  (identical copy already there)",
                        Installed::Newer => "  (replaces the older installed binary)",
                        Installed::Older => "  (would replace a NEWER installed binary)",
                    },
                    Style::new().fg(if c.installed == Installed::Older { Color::LightRed } else { DIM }),
                ),
            ]),
        ],
        None => vec![
            Line::from(vec![
                label("runs"),
                Span::raw(install::cargo_install(&picker.install_dir).display(&crate::shell::Shell::default())),
                Span::styled("  (release build, may take a while)", Style::new().fg(DIM)),
            ]),
            Line::from(vec![label("installs"), Span::raw(format!("{}/<bin name>", pretty_path(&picker.install_dir)))]),
        ],
    };
    for plan in picker.selected_desktop() {
        let (state, col) = match plan.status {
            DesktopStatus::New => ("not installed yet — will be added", Color::Yellow),
            DesktopStatus::Changed => ("differs — you'll see the diff (d to review now)", Color::Yellow),
            DesktopStatus::UpToDate => ("up to date", Color::Green),
            DesktopStatus::Linked => ("symlinked to the project file, always current", Color::Green),
        };
        let name = plan.src.file_name().unwrap_or_default().to_string_lossy();
        // Status first: long paths would otherwise push it out of the popup.
        let mut spans = vec![label("desktop"), Span::styled(state, Style::new().fg(col))];
        if plan.system && plan.needs_update() {
            spans.push(Span::styled(" · system file, needs sudo", Style::new().fg(DIM)));
        }
        spans.push(Span::styled(format!("  {name} → {}", pretty_path(&plan.target)), Style::new().fg(DIM)));
        lines.push(Line::from(spans));
    }
    if let Some(other) = picker.selected().and_then(|c| c.shadowed_by.as_ref()) {
        lines.push(Line::from(
            format!("⚠ {} comes first on PATH and runs instead — u to remove it", pretty_path(other)).fg(Color::Yellow),
        ));
    }
    if picker.selected().is_some_and(|c| c.stale) {
        lines.push(Line::from("sources changed after this was built — b / B to rebuild first".fg(Color::LightRed)));
    }
    let [_, d] = Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(details);
    f.render_widget(Paragraph::new(lines), d);
}

pub(super) fn draw_desktop_review(f: &mut Frame, review: &DesktopReview, area: Rect) {
    let plan = &review.plans[review.idx];
    let diff = match &plan.old {
        Some(old) => desktop::diff(old, &plan.new),
        None => plan.new.lines().map(|l| (Change::Added, l.to_string())).collect(),
    };

    let label = |s: &'static str| Span::styled(format!("{s:<10}"), Style::new().fg(DIM));
    let how = match (&plan.old, plan.system, plan.symlink) {
        (_, true, _) => "system file — written with sudo (you'll be asked for your password)",
        (None, _, _) => "new file",
        (Some(_), _, true) => "replaces the installed symlink with a file",
        (Some(_), _, false) => "replaces the installed copy",
    };
    let mut head = vec![
        Line::from(vec![label("from"), Span::raw(pretty_path(&plan.src))]),
        Line::from(vec![
            label("to"),
            Span::raw(pretty_path(&plan.target)),
            Span::styled(format!("  ({how})"), Style::new().fg(DIM)),
        ]),
    ];
    match &review.then {
        AfterReview::InstallBinary(c, _, before) => head.push(Line::from(vec![
            label("then"),
            Span::raw(format!("{}installs {} → {}", pulls(before), c.name, pretty_path(&c.dest))),
        ])),
        AfterReview::CargoInstall(_, before) => {
            head.push(Line::from(vec![label("then"), Span::raw(format!("{}runs cargo install", pulls(before)))]))
        }
        AfterReview::Nothing => {}
    }
    for note in &plan.notes {
        head.push(Line::from(vec![label("rewrote"), Span::styled(note.clone(), Style::new().fg(ACCENT))]));
    }

    let body: Vec<Line> = diff
        .iter()
        .map(|(change, text)| match change {
            Change::Same => Line::from(format!("  {text}").fg(DIM)),
            Change::Removed => Line::from(format!("- {text}").fg(Color::Red)),
            Change::Added => Line::from(format!("+ {text}").fg(Color::Green)),
        })
        .collect();

    let h = (head.len() + body.len()) as u16 + 5;
    let rect = popup(area, 110, h);
    let title = format!(" update desktop entry {}/{} ", review.idx + 1, review.plans.len());
    let inner =
        popup_pane(f, rect, Line::from(title.bold()), " y/Enter update · n skip entry · Esc cancel all · j/k scroll ");
    let [head_area, _, body_area] =
        Layout::vertical([Constraint::Length(head.len() as u16), Constraint::Length(1), Constraint::Fill(1)])
            .areas(inner);
    f.render_widget(Paragraph::new(head), head_area);
    f.render_widget(Paragraph::new(body).scroll((review.scroll, 0)), body_area);
}
