//! Key overviews: `?` and the `g` menu.

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Style, Stylize},
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::{config::pretty_path, git};

use super::{ACCENT, DIM, popup, popup_pane};

type HelpSection = (&'static str, &'static [(&'static str, &'static str)]);

/// Three columns, sized to fit a 30-row terminal.
const HELP: [&[HelpSection]; 3] = [
    &[(
        "navigate",
        &[
            ("j k ↑ ↓", "move"),
            ("h ← ⌫", "parent dir"),
            ("l →", "enter dir"),
            ("Enter", "enter dir / edit file"),
            ("Home End", "first / last"),
            ("PgUp PgDn", "page (also ^u ^d)"),
            ("/", "filter"),
            ("^P", "find file in project"),
            ("c", "go to path / zoxide"),
            ("Z", "zoxide picker"),
            ("J", "jump to project · ^G new"),
            ("Tab ⇧Tab", "complete in prompts"),
            ("~", "home"),
            (".", "toggle hidden"),
            ("^r", "reload"),
        ],
    )],
    &[
        (
            "do",
            &[
                ("e", "edit in $editor"),
                ("o / O", "VS Code file / project"),
                ("r / R", "run here / in terminal"),
                ("b / B", "build / release build"),
                ("t / T", "test / all tasks"),
                ("i", "install picker"),
                ("D", "deploy · edit targets"),
                ("C", "clean builds / caches"),
                ("g", "git menu"),
                ("M", "project notes / todos"),
                ("^L", "CLAUDE.md preset"),
                (":", "shell command"),
                ("!", "open shell here"),
                (",", "edit config"),
            ],
        ),
        (
            "files",
            &[
                ("a / A", "new file / dir"),
                ("m F2", "rename / move"),
                ("d Del", "trash / delete"),
                ("Space", "mark (for y d s u)"),
                ("y", "yank"),
                ("p / P", "paste copy / move"),
                ("s / u", "git stage / unstage"),
            ],
        ),
    ],
    &[
        (
            "output",
            &[
                ("Tab", "focus output pane"),
                ("w / z", "show / zoom output"),
                ("+ / -", "resize output"),
                ("[ / ]", "prev / next tab"),
                ("x ^c", "kill · x close/clear tab"),
                ("@ / W", "rerun / watch"),
                ("n / N", "next / prev file:line"),
                ("/  ^S", "search / save it"),
            ],
        ),
        ("view", &[("S", "sort name/newest/size"), ("v / V", "preview / git diff"), ("q", "quit")]),
    ],
];

fn help_column(sections: &[HelpSection]) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for (title, keys) in sections {
        if !lines.is_empty() {
            lines.push(Line::default());
        }
        lines.push(Line::from(title.to_uppercase().fg(Color::Yellow).bold()));
        for (k, d) in keys.iter() {
            lines.push(Line::from(vec![
                Span::styled(format!("  {k:<11}"), Style::new().fg(ACCENT).bold()),
                Span::raw(*d),
            ]));
        }
    }
    lines
}

pub(super) fn draw_help(f: &mut Frame, area: Rect) {
    let columns = HELP.map(help_column);
    let h = columns.iter().map(Vec::len).max().unwrap_or(0) as u16 + 2;
    let rect = popup(area, 110, h);
    let inner = popup_pane(f, rect, Line::from(" keys ".bold()), " custom keys: , to edit config · any key to close ");
    let areas = Layout::horizontal([Constraint::Ratio(1, 3); 3]).split(inner);
    for (lines, area) in columns.into_iter().zip(areas.iter()) {
        f.render_widget(Paragraph::new(lines), *area);
    }
}

fn git_column(groups: &'static [(&'static str, &'static [git::MenuItem])]) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for (title, items) in groups {
        lines.push(Line::from(title.to_uppercase().fg(Color::Yellow).bold()));
        for it in items.iter() {
            lines.push(Line::from(vec![
                Span::styled(format!("  {}  ", it.key), Style::new().fg(ACCENT).bold()),
                Span::raw(it.label),
            ]));
        }
        lines.push(Line::default());
    }
    lines
}

pub(super) fn draw_git_menu(f: &mut Frame, info: Option<&git::GitInfo>, area: Rect) {
    let left = git_column(&git::MENU[..2]);
    let right = git_column(&git::MENU[2..]);

    let status = match info {
        Some(g) => {
            let mut s = vec![Span::styled(format!(" {}", g.branch), Style::new().fg(Color::Magenta).bold())];
            if !g.upstream {
                s.push(" (no upstream)".fg(DIM));
            }
            if g.ahead > 0 {
                s.push(format!("  ↑{} to push", g.ahead).green());
            }
            if g.behind > 0 {
                s.push(format!("  ↓{} to pull", g.behind).red());
            }
            s.push(if g.dirty() { "  ● uncommitted changes".yellow() } else { "  clean".green() });
            s.push(format!("   {}", pretty_path(&g.root)).fg(DIM));
            Line::from(s)
        }
        None => Line::from(" not a git repository — i to init".fg(Color::Red)),
    };

    let h = left.len().max(right.len()) as u16 + 4;
    let rect = popup(area, 78, h);
    let inner = popup_pane(f, rect, Line::from(" git ".bold()), " Esc close ");
    let [top, _, body] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Fill(1)]).areas(inner);
    f.render_widget(status, top);
    let [l, r] = Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)]).areas(body);
    f.render_widget(Paragraph::new(left), l);
    f.render_widget(Paragraph::new(right), r);
}
