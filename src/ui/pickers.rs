//! The pickers that filter as you type: `^P` files, `J` projects, `T` tasks, `g o` branches,
//! `L` blog posts.

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Style, Stylize},
    text::{Line, Span},
    widgets::ListItem,
};

use crate::{
    app::{BlogPicker, BranchPicker, FileFinder, ProjectPicker, TaskPicker},
    blog::State,
    config::pretty_path,
    git, install,
};

use super::{ACCENT, DIM, filter_line, list, popup, popup_pane, truncate};

pub(super) fn draw_file_finder(f: &mut Frame, finder: &mut FileFinder, area: Rect) {
    let rect = popup(area, 100, area.height.saturating_sub(4));
    let title = Line::from(format!(" files in {} ", pretty_path(&finder.root)).bold());
    let inner = popup_pane(f, rect, title, " type to filter · Enter show in list · ^E edit · Esc close ");
    let [input, list_area] = Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(inner);
    filter_line(f, &finder.list.filter, Some(finder.list.total), input);

    let items: Vec<ListItem> = finder
        .list
        .visible
        .iter()
        .map(|&i| {
            let path = finder.list.items[i].as_str();
            let (dir, name) = path.rsplit_once('/').map_or(("", path), |(d, n)| (d, n));
            let mut spans = Vec::new();
            if !dir.is_empty() {
                spans.push(Span::styled(format!("{dir}/"), Style::new().fg(DIM)));
            }
            spans.push(Span::raw(name.to_string()));
            ListItem::new(Line::from(spans))
        })
        .collect();
    f.render_stateful_widget(list(items), list_area, &mut finder.list.state);
}

pub(super) fn draw_project_picker(f: &mut Frame, picker: &mut ProjectPicker, area: Rect) {
    let rect = popup(area, 90, picker.list.items.len() as u16 + 4);
    let title = Line::from(format!(" projects in {} ", pretty_path(&picker.root)).bold());
    let keys = " type to filter · Enter go · ^T notes · ^O VS Code · ^G new · ^L CLAUDE.md · Esc close ";
    let inner = popup_pane(f, rect, title, keys);
    let [input, list_area] = Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(inner);
    filter_line(f, &picker.list.filter, Some(picker.list.visible.len()), input);

    // name · kind · todos · age, the name taking what's left.
    let w = list_area.width.saturating_sub(2) as usize;
    let name_w = w.saturating_sub(2 + 7 + 5 + 6 + 9);
    let items: Vec<ListItem> = picker
        .list
        .visible
        .iter()
        .map(|&i| {
            let e = &picker.list.items[i];
            let here = e.path == picker.current;
            let (group, name) = e.name.rsplit_once('/').map_or(("", e.name.as_str()), |(g, n)| (g, n));
            let shown = truncate(&e.name, name_w);
            let pad = name_w.saturating_sub(shown.chars().count());
            let mut spans = vec![Span::styled(if here { "● " } else { "  " }, Style::new().fg(ACCENT))];
            if shown == e.name && !group.is_empty() {
                spans.push(Span::styled(format!("{group}/"), Style::new().fg(DIM)));
                spans.push(Span::styled(name.to_string(), Style::new().bold()));
            } else {
                spans.push(Span::styled(shown, Style::new().bold()));
            }
            let todos = match picker.todos[i] {
                0 => String::new(),
                n => format!("☐ {n}"),
            };
            let running = match picker.running[i] {
                0 => String::new(),
                n => format!("▶ {n}"),
            };
            spans.extend([
                Span::raw(" ".repeat(pad)),
                Span::styled(format!("{running:>5}"), Style::new().fg(Color::Green)),
                Span::styled(format!(" {:<6}", e.kind), Style::new().fg(Color::Yellow)),
                Span::styled(format!("{todos:>5} "), Style::new().fg(Color::Yellow)),
                Span::styled(format!("{:>9}", e.active.map(install::age).unwrap_or_default()), Style::new().fg(DIM)),
            ]);
            ListItem::new(Line::from(spans))
        })
        .collect();
    // One column for the marker: the name gets every column it can.
    f.render_stateful_widget(list(items).highlight_symbol("▌"), list_area, &mut picker.list.state);
}

pub(super) fn draw_task_picker(f: &mut Frame, picker: &mut TaskPicker, area: Rect) {
    let rect = popup(area, 100, picker.list.items.len() as u16 + 4);
    let title = Line::from(format!(" tasks · {} ", picker.title).bold());
    let inner = popup_pane(f, rect, title, " type to filter · Enter run · ^R in terminal · Esc close ");
    let [input, list_area] = Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(inner);
    filter_line(f, &picker.list.filter, None, input);

    let name_w = picker.list.items.iter().map(|t| t.name.chars().count()).max().unwrap_or(0).min(28);
    let items: Vec<ListItem> = picker
        .list
        .visible
        .iter()
        .map(|&i| {
            let t = &picker.list.items[i];
            ListItem::new(Line::from(vec![
                Span::styled(format!("{:<8}", t.group), Style::new().fg(Color::Yellow)),
                Span::styled(format!("{:<name_w$}  ", truncate(&t.name, name_w)), Style::new().bold()),
                Span::styled(t.cmd.clone(), Style::new().fg(DIM)),
            ]))
        })
        .collect();
    f.render_stateful_widget(list(items), list_area, &mut picker.list.state);
}

pub(super) fn draw_branch_picker(f: &mut Frame, picker: &mut BranchPicker, area: Rect) {
    let rect = popup(area, 130, picker.list.items.len() as u16 + 5);
    let title = match &picker.base {
        Some(base) => format!(" switch branch · numbered in merge order, against {base} "),
        None => " switch branch ".to_string(),
    };
    let inner = popup_pane(f, rect, Line::from(title.bold()), " type to filter · Enter switch · Esc close ");
    let [input, head, list_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Fill(1)]).areas(inner);
    filter_line(f, &picker.list.filter, None, input);

    let name_w = picker.list.items.iter().map(|b| b.name.chars().count()).max().unwrap_or(0).clamp(6, 36);
    let in_base = format!("in {}", picker.base.as_deref().unwrap_or("base"));
    let commits_w = in_base.chars().count().max(7);
    // The highlight symbol takes two columns in front of every row.
    f.render_widget(
        Line::from(format!(
            "    #    {:<name_w$}  {:<16}  {:<16}  {:<commits_w$}  remote",
            "branch", "work started", "last commit", "commits"
        ))
        .fg(DIM),
        head,
    );

    let items: Vec<ListItem> = picker
        .list
        .visible
        .iter()
        .map(|&i| {
            let b = &picker.list.items[i];
            let number = b.number.map_or_else(|| "   ".to_string(), |n| format!("{n:>3}"));
            let mark = if b.current { "● " } else { "  " };
            let name_style = if b.current { Style::new().fg(ACCENT).bold() } else { Style::new().bold() };
            let started = b.started.as_ref().map_or("—", |s| s.1.as_str());
            let (commits, commits_color) = if b.base {
                ("base".to_string(), ACCENT)
            } else if b.ahead > 0 {
                (format!("+{}", b.ahead), Color::Reset)
            } else {
                (in_base.clone(), DIM)
            };
            let (remote, remote_color) = branch_remote(b);
            ListItem::new(Line::from(vec![
                Span::styled(format!("{number}  "), Style::new().fg(Color::Yellow).bold()),
                Span::styled(mark, Style::new().fg(ACCENT)),
                Span::styled(format!("{:<name_w$}  ", truncate(&b.name, name_w)), name_style),
                Span::styled(
                    format!("{started:<16}  "),
                    Style::new().fg(if b.started.is_some() { Color::Reset } else { DIM }),
                ),
                Span::raw(format!("{:<16}  ", b.last.1)),
                Span::styled(format!("{commits:<commits_w$}  "), Style::new().fg(commits_color)),
                Span::styled(remote, Style::new().fg(remote_color)),
            ]))
        })
        .collect();
    f.render_stateful_widget(list(items), list_area, &mut picker.list.state);
}

/// How a branch stands with its remote, for the last column of the `g o` picker.
fn branch_remote(b: &git::Branch) -> (String, Color) {
    if b.remote_only {
        return ("only on the remote".into(), DIM);
    }
    match b.upstream {
        git::Upstream::None => ("not pushed".into(), DIM),
        git::Upstream::Gone => ("deleted on the remote".into(), Color::Yellow),
        git::Upstream::Track { ahead: 0, behind: 0 } => ("pushed".into(), Color::Green),
        git::Upstream::Track { ahead, behind } => {
            let parts: Vec<String> = [(ahead, "to push"), (behind, "to pull")]
                .into_iter()
                .filter(|(n, _)| *n > 0)
                .map(|(n, what)| format!("{n} {what}"))
                .collect();
            (parts.join(", "), Color::Yellow)
        }
    }
}

pub(super) fn draw_blog_picker(f: &mut Frame, picker: &mut BlogPicker, area: Rect) {
    let rect = popup(area, 100, picker.list.items.len().max(3) as u16 + 4);
    let title = Line::from(format!(" blog · {} ", pretty_path(&picker.root)).bold());
    let keys = if picker.filtering {
        " type to filter · Enter keep · Esc clear "
    } else {
        " ⏎ write · E own editor · a new · p publish · P push · u unpublish · d trash · / filter · Esc close "
    };
    let inner = popup_pane(f, rect, title, keys);
    let [input, list_area] = Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(inner);
    if picker.filtering {
        filter_line(f, &picker.list.filter, Some(picker.list.total), input);
    } else if picker.list.filter.is_empty() {
        let (drafts, posts) = picker.list.items.iter().fold((0, 0), |(d, p), post| match post.state {
            State::Draft => (d + 1, p),
            State::Post => (d, p + 1),
        });
        f.render_widget(Line::from(format!(" {drafts} drafts · {posts} posts · / filters")).fg(DIM), input);
    } else {
        f.render_widget(
            Line::from(vec![
                Span::styled(" › ", Style::new().fg(DIM)),
                Span::raw(picker.list.filter.text.clone()),
                Span::styled("   Esc shows all", Style::new().fg(DIM)),
            ]),
            input,
        );
    }

    if picker.list.items.is_empty() {
        f.render_widget(
            Line::from(vec![
                Span::styled("  no drafts or posts yet · ", Style::new().fg(DIM)),
                Span::styled("a", Style::new().fg(ACCENT).bold()),
                Span::styled(" writes one", Style::new().fg(DIM)),
            ]),
            list_area,
        );
        return;
    }
    let items: Vec<ListItem> = picker
        .list
        .visible
        .iter()
        .map(|&i| {
            let p = &picker.list.items[i];
            let (state, color) = match p.state {
                State::Draft => ("draft", Color::Yellow),
                State::Post => ("post", Color::Green),
            };
            ListItem::new(Line::from(vec![
                Span::styled(format!("{state:<6}"), Style::new().fg(color)),
                Span::styled(format!("{:<12}", p.date.as_deref().unwrap_or("")), Style::new().fg(DIM)),
                Span::styled(p.title.clone(), Style::new().bold()),
            ]))
        })
        .collect();
    f.render_stateful_widget(list(items), list_area, &mut picker.list.state);
}
