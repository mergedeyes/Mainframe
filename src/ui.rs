use ratatui::{
    Frame,
    layout::{Constraint, Layout, Position, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, BorderType, Clear, List, ListItem, Paragraph, Wrap},
};

use crate::{
    app::{AfterReview, App, DesktopReview, FileFinder, Focus, InstallPicker, Level, Mode, NotesView, ProjectPicker, TaskPicker},
    complete::Completer,
    form::{Form, FormKind},
    browser::{Preview, Sort, human_size},
    config::pretty_path,
    desktop::{self, Change, Status as DesktopStatus},
    git,
    install::{self, Installed},
    notes::Item,
    task::Task,
};

const ACCENT: Color = Color::Cyan;
const DIM: Color = Color::DarkGray;
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub fn draw(f: &mut Frame, app: &mut App) {
    let [header, main, footer] =
        Layout::vertical([Constraint::Length(1), Constraint::Fill(1), Constraint::Length(1)]).areas(f.area());

    draw_header(f, app, header);

    let (top, bottom) = match (app.show_output, app.zoom_output) {
        (false, _) => (Some(main), None),
        (true, true) => (None, Some(main)),
        (true, false) => {
            let [t, b] = Layout::vertical([
                Constraint::Percentage(100 - app.output_pct),
                Constraint::Percentage(app.output_pct),
            ])
            .areas(main);
            (Some(t), Some(b))
        }
    };
    if let Some(area) = top {
        if app.show_preview && area.width >= 70 {
            let [l, r] = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(area);
            draw_browser(f, app, l);
            draw_preview(f, app, r);
        } else {
            draw_browser(f, app, area);
        }
    }
    if let Some(area) = bottom {
        draw_output(f, app, area);
    }

    draw_footer(f, app, footer);

    match &mut app.mode {
        Mode::Help => draw_help(f, f.area()),
        Mode::GitMenu => draw_git_menu(f, app.git.as_ref(), f.area()),
        Mode::Install(picker) => draw_install(f, picker, f.area()),
        Mode::Tasks(picker) => draw_task_picker(f, picker, f.area()),
        Mode::Files(finder) => draw_file_finder(f, finder, f.area()),
        Mode::Projects(picker) => draw_project_picker(f, picker, f.area()),
        Mode::Notes(view) => draw_notes(f, view, f.area()),
        Mode::Form(form) => draw_form(f, form, f.area()),
        Mode::DesktopReview(review) => draw_desktop_review(f, review, f.area()),
        _ => {}
    }
}

fn pane(title: Line<'static>, focused: bool) -> Block<'static> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(if focused { ACCENT } else { DIM }))
        .title(title)
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
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

    let mut badges = Vec::new();
    if let Some(p) = &app.project {
        badges.push(Span::raw("  "));
        badges.push(Span::styled(format!(" {} · {} ", p.kind.label(), p.name), Style::new().fg(Color::Black).bg(Color::Yellow)));
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

fn draw_browser(f: &mut Frame, app: &mut App, area: Rect) {
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
    let mut block = pane(Line::from(title), app.focus == Focus::Browser)
        .title_bottom(Line::from(count).right_aligned().fg(DIM));
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
            let (mark, mark_col) = app
                .git
                .as_ref()
                .and_then(|g| g.mark(&e.path, e.is_dir))
                .unwrap_or((" ", Color::Reset));
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
    let list = List::new(items)
        .block(block)
        .highlight_symbol("▌ ")
        .highlight_style(Style::new().bg(Color::Rgb(40, 44, 52)).add_modifier(Modifier::BOLD));
    f.render_stateful_widget(list, area, &mut app.browser.state);
}

fn draw_preview(f: &mut Frame, app: &App, area: Rect) {
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
            .map(|(n, dir)| {
                if *dir {
                    Line::from(format!("{n}/").blue().bold())
                } else {
                    Line::from(n.clone())
                }
            })
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

fn draw_output(f: &mut Frame, app: &mut App, area: Rect) {
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
            title.extend([Span::raw(" "), state, Span::styled(format!(" {} {} ", i + 1, truncate(&t.label, 20)), style)]);
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
        title.push(Span::styled(format!("/{} {at}/{} ", out.search, out.matches.len()), Style::new().fg(Color::Yellow)));
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

fn spinner(task: &Task) -> &'static str {
    SPINNER[(task.started.elapsed().as_millis() / 80) as usize % SPINNER.len()]
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    match &app.mode {
        Mode::Prompt { kind, input, completer, .. } => {
            let label = format!(" {}: ", kind.label());
            let label_w = label.chars().count() as u16;
            let x = area.x + label_w + input.col() as u16;
            f.render_widget(
                Line::from(vec![Span::styled(label, Style::new().fg(ACCENT).bold()), Span::raw(input.text.clone())]),
                area,
            );
            f.set_cursor_position(Position::new(x.min(area.right().saturating_sub(1)), area.y));
            if let Some(c) = completer {
                draw_completions(f, c, area.x + label_w.saturating_sub(1), area.y, true);
            }
            return;
        }
        Mode::Confirm { msg, .. } => {
            f.render_widget(Line::from(format!(" {msg} ")).style(Style::new().bg(Color::Yellow).fg(Color::Black).bold()), area);
            return;
        }
        Mode::Search(edit) => {
            f.render_widget(
                Line::from(vec![
                    Span::styled(" search output: ", Style::new().fg(ACCENT).bold()),
                    Span::raw(edit.text.clone()),
                    Span::styled("   Enter keep · Esc clear", Style::new().fg(DIM)),
                ]),
                area,
            );
            let x = area.x + 16 + edit.col() as u16;
            f.set_cursor_position(Position::new(x.min(area.right().saturating_sub(1)), area.y));
            return;
        }
        Mode::Filter(edit) => {
            f.render_widget(
                Line::from(vec![
                    Span::styled(" filter: ", Style::new().fg(ACCENT).bold()),
                    Span::raw(edit.text.clone()),
                    Span::styled("   Enter keep · Esc clear", Style::new().fg(DIM)),
                ]),
                area,
            );
            let x = area.x + 9 + edit.col() as u16;
            f.set_cursor_position(Position::new(x.min(area.right().saturating_sub(1)), area.y));
            return;
        }
        _ => {}
    }

    if let Some((msg, level, _)) = &app.status {
        let col = match level {
            Level::Info => ACCENT,
            Level::Ok => Color::Green,
            Level::Err => Color::Red,
        };
        f.render_widget(Line::from(format!(" {msg}").fg(col)), area);
        return;
    }

    let key = |k: &'static str| Span::styled(k, Style::new().fg(ACCENT).bold());
    let txt = |t: &'static str| Span::styled(t, Style::new().fg(DIM));
    let mut spans = vec![Span::raw(" ")];
    if let Some(hint) = &app.run_hint {
        spans.extend([key("r "), Span::styled(format!("{} ", truncate(hint, 40)), Style::new().fg(Color::Green)), txt(" ")]);
    }
    spans.extend([
        key("e"), txt(" edit  "),
        key("b"), txt(" build  "),
        key("i"), txt(" install  "),
        key("o"), txt(" code  "),
        key("g"), txt(" git  "),
        key(":"), txt(" cmd  "),
        key("?"), txt(" help"),
    ]);
    f.render_widget(Line::from(spans), area);
}

/// Suggestion list anchored to an input line: above it (footer prompt) or below it (form field).
fn draw_completions(f: &mut Frame, c: &Completer, x: u16, anchor_y: u16, above: bool) {
    const ROWS: usize = 10;
    let n = c.items.len();
    if n == 0 {
        return;
    }
    let screen = f.area();
    let shown = n.min(ROWS);
    // Scroll the window so the selected item stays visible.
    let first = c.selected.map_or(0, |s| (s + 1).saturating_sub(ROWS)).min(n - shown);
    let items = &c.items[first..first + shown];

    let widest = items.iter().map(|i| i.display.chars().count()).max().unwrap_or(0);
    let x = x.min(screen.right().saturating_sub(24));
    let w = (widest as u16 + 4).clamp(24, screen.right().saturating_sub(x).max(24));
    let h = shown as u16 + 2;
    let y = if above { anchor_y.saturating_sub(h) } else { (anchor_y + 1).min(screen.bottom().saturating_sub(h)) };
    let rect = Rect::new(x, y, w, h).intersection(screen);

    let lines: Vec<Line> = items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let mut style = if item.is_dir { Style::new().fg(Color::Blue).bold() } else { Style::new() };
            if c.selected == Some(first + i) {
                style = style.bg(Color::Rgb(40, 44, 52)).add_modifier(Modifier::REVERSED);
            }
            Line::from(Span::styled(format!(" {} ", truncate(&item.display, w as usize - 4)), style))
        })
        .collect();

    let count = match c.selected {
        Some(s) => format!(" {}/{n} ", s + 1),
        None if n > shown => format!(" {n} matches · Tab "),
        None => " Tab ".into(),
    };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(DIM))
        .title_bottom(Line::from(count).right_aligned());
    f.render_widget(Clear, rect);
    f.render_widget(Paragraph::new(lines).block(block), rect);
}

fn popup(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width.saturating_sub(2));
    let h = h.min(area.height.saturating_sub(2));
    Rect::new(area.x + (area.width - w) / 2, area.y + (area.height - h) / 2, w, h)
}

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
            ("J", "jump to project"),
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
                ("g", "git menu"),
                ("M", "project notes / todos"),
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
                ("x ^c", "kill · x closes tab"),
                ("@ / W", "rerun / watch"),
                ("n / N", "next / prev file:line"),
                ("/  ^S", "search / save it"),
            ],
        ),
        (
            "view",
            &[
                ("S", "sort name/newest/size"),
                ("v / V", "preview / git diff"),
                ("q", "quit"),
            ],
        ),
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

fn draw_help(f: &mut Frame, area: Rect) {
    let columns = HELP.map(help_column);
    let h = columns.iter().map(Vec::len).max().unwrap_or(0) as u16 + 2;
    let rect = popup(area, 110, h);
    f.render_widget(Clear, rect);
    let block = pane(Line::from(" keys ".bold()), true)
        .title_bottom(Line::from(" custom keys: , to edit config · any key to close ").right_aligned().fg(DIM));
    let inner = block.inner(rect);
    f.render_widget(block, rect);
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

fn draw_git_menu(f: &mut Frame, info: Option<&git::GitInfo>, area: Rect) {
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
    f.render_widget(Clear, rect);
    let block = pane(Line::from(" git ".bold()), true).title_bottom(Line::from(" Esc close ").right_aligned().fg(DIM));
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let [top, _, body] = Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Fill(1)]).areas(inner);
    f.render_widget(status, top);
    let [l, r] = Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)]).areas(body);
    f.render_widget(Paragraph::new(left), l);
    f.render_widget(Paragraph::new(right), r);
}

fn draw_file_finder(f: &mut Frame, finder: &mut FileFinder, area: Rect) {
    let rect = popup(area, 100, area.height.saturating_sub(4));
    f.render_widget(Clear, rect);
    let block = pane(Line::from(format!(" files in {} ", pretty_path(&finder.root)).bold()), true)
        .title_bottom(Line::from(" type to filter · Enter show in list · ^E edit · Esc close ").right_aligned().fg(DIM));
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let [input, list_area] = Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(inner);

    let count = format!("{} ", finder.total);
    f.render_widget(
        Line::from(vec![Span::styled(" › ", Style::new().fg(ACCENT).bold()), Span::raw(finder.filter.text.clone())]),
        input,
    );
    f.render_widget(Line::from(count).right_aligned().fg(DIM), input);
    f.set_cursor_position(Position::new((input.x + 3 + finder.filter.col() as u16).min(input.right().saturating_sub(1)), input.y));

    let items: Vec<ListItem> = finder
        .visible
        .iter()
        .map(|&i| {
            let path = finder.file(i);
            let (dir, name) = path.rsplit_once('/').map_or(("", path), |(d, n)| (d, n));
            let mut spans = Vec::new();
            if !dir.is_empty() {
                spans.push(Span::styled(format!("{dir}/"), Style::new().fg(DIM)));
            }
            spans.push(Span::raw(name.to_string()));
            ListItem::new(Line::from(spans))
        })
        .collect();
    let list = List::new(items)
        .highlight_symbol("▌ ")
        .highlight_style(Style::new().bg(Color::Rgb(40, 44, 52)).add_modifier(Modifier::BOLD));
    f.render_stateful_widget(list, list_area, &mut finder.state);
}

fn draw_project_picker(f: &mut Frame, picker: &mut ProjectPicker, area: Rect) {
    let rect = popup(area, 90, picker.items.len() as u16 + 4);
    f.render_widget(Clear, rect);
    let block = pane(Line::from(format!(" projects in {} ", pretty_path(&picker.root)).bold()), true).title_bottom(
        Line::from(" type to filter · Enter go · ^T notes · ^O VS Code · Esc close ").right_aligned().fg(DIM),
    );
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let [input, list_area] = Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(inner);

    f.render_widget(
        Line::from(vec![Span::styled(" › ", Style::new().fg(ACCENT).bold()), Span::raw(picker.filter.text.clone())]),
        input,
    );
    f.render_widget(Line::from(format!("{} ", picker.visible.len())).right_aligned().fg(DIM), input);
    f.set_cursor_position(Position::new((input.x + 3 + picker.filter.col() as u16).min(input.right().saturating_sub(1)), input.y));

    // name · kind · todos · age, the name taking what's left.
    let w = list_area.width.saturating_sub(2) as usize;
    let name_w = w.saturating_sub(2 + 7 + 6 + 9);
    let items: Vec<ListItem> = picker
        .visible
        .iter()
        .map(|&i| {
            let e = &picker.items[i];
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
            spans.extend([
                Span::raw(" ".repeat(pad)),
                Span::styled(format!(" {:<6}", e.kind), Style::new().fg(Color::Yellow)),
                Span::styled(format!("{todos:>5} "), Style::new().fg(Color::Yellow)),
                Span::styled(format!("{:>9}", e.active.map(install::age).unwrap_or_default()), Style::new().fg(DIM)),
            ]);
            ListItem::new(Line::from(spans))
        })
        .collect();
    let list = List::new(items)
        .highlight_symbol("▌")
        .highlight_style(Style::new().bg(Color::Rgb(40, 44, 52)).add_modifier(Modifier::BOLD));
    f.render_stateful_widget(list, list_area, &mut picker.state);
}

fn draw_notes(f: &mut Frame, view: &mut NotesView, area: Rect) {
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
    let block = pane(Line::from(title), true)
        .title(Line::from(close).right_aligned().fg(DIM))
        .title_bottom(Line::from(keys).right_aligned().fg(DIM));
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    if view.notes.items.is_empty() {
        f.render_widget(
            Paragraph::new(vec![
                Line::default(),
                Line::from("  nothing here yet".fg(DIM)),
                Line::default(),
                Line::from(vec!["  a".fg(ACCENT).bold(), " add a todo   ".fg(DIM), "n".fg(ACCENT).bold(), " add a note   ".fg(DIM), "E".fg(ACCENT).bold(), " write in the editor".fg(DIM)]),
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
                    Item::Todo { done: true, text, .. } => Span::styled(text.clone(), Style::new().fg(DIM).crossed_out()),
                    Item::Todo { text, .. } => Span::raw(text.clone()),
                    Item::Note(text) if text.starts_with('#') => Span::styled(text.clone(), Style::new().fg(ACCENT).bold()),
                    Item::Note(text) => Span::styled(text.clone(), Style::new().fg(Color::Gray)),
                },
            };
            ListItem::new(Line::from(vec![Span::styled(lead, lead_style), text]))
        })
        .collect();
    let list = List::new(items)
        .highlight_symbol("▌ ")
        .highlight_style(Style::new().bg(Color::Rgb(40, 44, 52)));
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

fn draw_task_picker(f: &mut Frame, picker: &mut TaskPicker, area: Rect) {
    let rect = popup(area, 100, picker.items.len() as u16 + 4);
    f.render_widget(Clear, rect);
    let block = pane(Line::from(format!(" tasks · {} ", picker.title).bold()), true)
        .title_bottom(Line::from(" type to filter · Enter run · ^R in terminal · Esc close ").right_aligned().fg(DIM));
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let [input, list_area] = Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(inner);

    f.render_widget(
        Line::from(vec![Span::styled(" › ", Style::new().fg(ACCENT).bold()), Span::raw(picker.filter.text.clone())]),
        input,
    );
    f.set_cursor_position(Position::new((input.x + 3 + picker.filter.col() as u16).min(input.right().saturating_sub(1)), input.y));

    let name_w = picker.items.iter().map(|t| t.name.chars().count()).max().unwrap_or(0).min(28);
    let items: Vec<ListItem> = picker
        .visible
        .iter()
        .map(|&i| {
            let t = &picker.items[i];
            ListItem::new(Line::from(vec![
                Span::styled(format!("{:<8}", t.group), Style::new().fg(Color::Yellow)),
                Span::styled(format!("{:<name_w$}  ", truncate(&t.name, name_w)), Style::new().bold()),
                Span::styled(t.cmd.clone(), Style::new().fg(DIM)),
            ]))
        })
        .collect();
    let list = List::new(items)
        .highlight_symbol("▌ ")
        .highlight_style(Style::new().bg(Color::Rgb(40, 44, 52)).add_modifier(Modifier::BOLD));
    f.render_stateful_widget(list, list_area, &mut picker.state);
}

fn draw_install(f: &mut Frame, picker: &mut InstallPicker, area: Rect) {
    let rect = popup(area, 120, picker.len() as u16 + 14);
    f.render_widget(Clear, rect);
    let block = pane(Line::from(format!(" install from {} ", pretty_path(&picker.root)).bold()), true)
        .title_bottom(Line::from(" Enter install · d desktop entry · r run · u remove shadowing copy · Esc close ").right_aligned().fg(DIM));
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let [head, list_area, details] =
        Layout::vertical([Constraint::Length(1), Constraint::Fill(1), Constraint::Length(8)]).areas(inner);

    let row = |name: &str, kind: &str, arch: &str, size: &str, age: &str| {
        format!("{:<22} {:<16} {:<20} {:>8}  {:<10}", truncate(name, 22), truncate(kind, 16), truncate(arch, 20), size, age)
    };
    f.render_widget(
        Line::from(format!("  {}  status", row("name", "built as", "arch", "size", "built"))).fg(DIM),
        head,
    );

    let mut items = Vec::new();
    if picker.cargo {
        items.push(ListItem::new(Line::from(vec![
            Span::styled("⟳ cargo install --path .", Style::new().fg(Color::Yellow).bold()),
            Span::styled(format!("   build release from source into {}", pretty_path(&picker.install_dir)), Style::new().fg(DIM)),
        ])));
    }
    for c in &picker.items {
        let mut spans = vec![Span::raw(row(&c.name, &c.kind, &c.arch, &human_size(c.size), &install::age(c.modified))), Span::raw("  ")];
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
    let list = List::new(items)
        .highlight_symbol("▌ ")
        .highlight_style(Style::new().bg(Color::Rgb(40, 44, 52)).add_modifier(Modifier::BOLD));
    f.render_stateful_widget(list, list_area, &mut picker.state);

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
                Span::raw(install::cargo_install_cmd(&picker.install_dir)),
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

fn draw_desktop_review(f: &mut Frame, review: &DesktopReview, area: Rect) {
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
        Line::from(vec![label("to"), Span::raw(pretty_path(&plan.target)), Span::styled(format!("  ({how})"), Style::new().fg(DIM))]),
    ];
    match &review.then {
        AfterReview::InstallBinary(c, _) => head.push(Line::from(vec![
            label("then"),
            Span::raw(format!("installs {} → {}", c.name, pretty_path(&c.dest))),
        ])),
        AfterReview::CargoInstall(_) => head.push(Line::from(vec![label("then"), Span::raw("runs cargo install")])),
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
    f.render_widget(Clear, rect);
    let title = format!(" update desktop entry {}/{} ", review.idx + 1, review.plans.len());
    let block = pane(Line::from(title.bold()), true).title_bottom(
        Line::from(" y/Enter update · n skip entry · Esc cancel all · j/k scroll ").right_aligned().fg(DIM),
    );
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let [head_area, _, body_area] =
        Layout::vertical([Constraint::Length(head.len() as u16), Constraint::Length(1), Constraint::Fill(1)]).areas(inner);
    f.render_widget(Paragraph::new(head), head_area);
    f.render_widget(Paragraph::new(body).scroll((review.scroll, 0)), body_area);
}

fn draw_form(f: &mut Frame, form: &Form, area: Rect) {
    // One line per step; long ones wrap.
    let (steps, verb) = match &form.kind {
        FormKind::GhRepoCreate => (vec![crate::git::gh_repo_create_cmd(form)], "create"),
        FormKind::Release(ctx) => (crate::release::steps(form, ctx), "release"),
    };
    let w = 92.min(area.width.saturating_sub(2));
    let mut cmd = vec![Line::from("  command".fg(DIM))];
    for (i, step) in steps.iter().enumerate() {
        cmd.extend(hang_wrap(if i == 0 { "  $  " } else { "  && " }, step, w.saturating_sub(7) as usize));
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

/// Keeps the end of a path, which is the part that matters: `…/deep/dir`.
fn truncate_left(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max {
        s.to_string()
    } else if max == 0 {
        String::new()
    } else {
        format!("…{}", s.chars().skip(n - max + 1).collect::<String>())
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else if max == 0 {
        String::new()
    } else {
        let mut t: String = s.chars().take(max - 1).collect();
        t.push('…');
        t
    }
}
