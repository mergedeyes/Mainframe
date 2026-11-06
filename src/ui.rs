use ratatui::{
    Frame,
    layout::{Constraint, Layout, Position, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, BorderType, Clear, List, ListItem, Paragraph, Wrap},
};

use crate::{
    app::{AfterReview, App, DesktopReview, Focus, InstallPicker, Level, Mode},
    complete::Completer,
    form::{Form, FormKind},
    browser::{Preview, human_size},
    config::pretty_path,
    desktop::{self, Change, Status as DesktopStatus},
    git,
    install::{self, Installed},
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
    let right = if let Some(t) = &app.task {
        let el = t.started.elapsed();
        let frame = SPINNER[(el.as_millis() / 80) as usize % SPINNER.len()];
        Line::from(Span::styled(
            format!(" {frame} {} {:.1}s ", truncate(&t.label, 40), el.as_secs_f32()),
            Style::new().fg(ACCENT),
        ))
    } else if let Some(l) = &app.last_run {
        let (sym, col) = if l.ok { ("✔", Color::Green) } else { ("✘", Color::Red) };
        Line::from(Span::styled(format!(" {sym} {} ", truncate(&l.label, 40)), Style::new().fg(col)))
    } else {
        Line::default()
    };

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
    if !b.filter.is_empty() || matches!(app.mode, Mode::Filter) {
        title.push(Span::styled(format!("/{} ", b.filter), Style::new().fg(Color::Yellow)));
    }
    let count = format!(" {}/{} ", b.state.selected().map_or(0, |i| i + 1), b.visible.len());
    let mut block = pane(Line::from(title), app.focus == Focus::Browser)
        .title_bottom(Line::from(count).right_aligned().fg(DIM));
    if b.hidden_count > 0 {
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
            let name = if e.is_dir { format!("{}/", e.name) } else { e.name.clone() };
            let size = if e.is_dir { String::new() } else { human_size(e.size) };
            let name_w = inner_w.saturating_sub(size.len() + 3);
            let name = truncate(&name, name_w);
            let pad = inner_w.saturating_sub(2 + name.chars().count() + size.len());
            ListItem::new(Line::from(vec![
                Span::styled(format!("{mark} "), Style::new().fg(mark_col).bold()),
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
    let title = entry.map_or(String::new(), |e| format!(" {} ", e.name));
    let block = pane(Line::from(title).fg(DIM), false);
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
    app.output.height = height;
    // The pane may have shrunk (resize, zoom off) since the last scroll.
    app.output.scroll = app.output.scroll.min(app.output.max_scroll());

    let mut title = vec![Span::styled(" output ", Style::new().bold())];
    if let Some(t) = &app.task {
        let frame = SPINNER[(t.started.elapsed().as_millis() / 80) as usize % SPINNER.len()];
        title.push(Span::styled(format!("{frame} running {:.1}s ", t.started.elapsed().as_secs_f32()), Style::new().fg(ACCENT)));
    } else if let Some(l) = &app.last_run {
        let col = if l.ok { Color::Green } else { Color::Red };
        title.push(Span::styled(format!("{} {} ", if l.ok { "✔" } else { "✘" }, l.detail), Style::new().fg(col)));
    }
    let mut block = pane(Line::from(title), app.focus == Focus::Output);
    if app.output.scroll > 0 {
        block = block.title_bottom(
            Line::from(format!(" ↑ {} lines up · End to follow ", app.output.scroll)).right_aligned().fg(Color::Yellow),
        );
    } else if app.focus == Focus::Output {
        block = block.title_bottom(Line::from(" ↑↓ PgUp PgDn scroll · z zoom · Tab back ").right_aligned().fg(DIM));
    }

    let lines = &app.output.lines;
    let end = lines.len().saturating_sub(app.output.scroll);
    let start = end.saturating_sub(height);
    f.render_widget(Paragraph::new(lines[start..end].to_vec()).block(block), area);
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    match &app.mode {
        Mode::Prompt { kind, input, completer } => {
            let label = format!(" {}: ", kind.label());
            let label_w = label.chars().count() as u16;
            let x = area.x + label_w + input.chars().count() as u16;
            f.render_widget(
                Line::from(vec![Span::styled(label, Style::new().fg(ACCENT).bold()), Span::raw(input.clone())]),
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
        Mode::Filter => {
            f.render_widget(
                Line::from(vec![
                    Span::styled(" filter: ", Style::new().fg(ACCENT).bold()),
                    Span::raw(app.browser.filter.clone()),
                    Span::styled("   Enter keep · Esc clear", Style::new().fg(DIM)),
                ]),
                area,
            );
            let x = area.x + 9 + app.browser.filter.chars().count() as u16;
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

const HELP: &[(&str, &[(&str, &str)])] = &[
    (
        "navigate",
        &[
            ("j k ↑ ↓", "move"),
            ("h ← ⌫", "parent dir"),
            ("l →", "enter dir"),
            ("Enter", "enter dir / edit file"),
            ("Home End", "first / last"),
            ("PgUp PgDn ^u ^d", "page"),
            ("/", "filter"),
            ("c", "go to path / zoxide"),
            ("Z", "zoxide picker (fzf)"),
            ("Tab ⇧Tab", "complete in any prompt"),
            ("~", "home"),
            (".", "toggle hidden"),
            ("^r", "reload"),
        ],
    ),
    (
        "do",
        &[
            ("e", "edit in nano ($editor)"),
            ("o / O", "VS Code: file / project"),
            ("r", "run → output pane"),
            ("R", "run in full terminal"),
            ("b / B", "build / release build"),
            ("t", "test"),
            ("i", "install picker"),
            ("g", "git menu"),
            (":", "shell command"),
            ("!", "open shell here"),
            ("x  ^c", "kill running task"),
            (",", "edit config"),
        ],
    ),
    (
        "files",
        &[
            ("a / A", "new file / dir"),
            ("m F2", "rename / move"),
            ("d Del", "trash / delete"),
            ("y", "yank"),
            ("p / P", "paste copy / move"),
        ],
    ),
    (
        "view",
        &[
            ("v", "toggle preview"),
            ("w", "toggle output pane"),
            ("z", "zoom output"),
            ("Tab", "focus output (scroll)"),
            ("+ / -", "resize output"),
            ("q", "quit"),
        ],
    ),
];

fn help_column(sections: &'static [(&'static str, &'static [(&'static str, &'static str)])]) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for (title, keys) in sections {
        lines.push(Line::from(title.to_uppercase().fg(Color::Yellow).bold()));
        for (k, d) in keys.iter() {
            lines.push(Line::from(vec![
                Span::styled(format!("  {k:<16}"), Style::new().fg(ACCENT).bold()),
                Span::raw(*d),
            ]));
        }
        lines.push(Line::default());
    }
    lines
}

fn draw_help(f: &mut Frame, area: Rect) {
    let left = help_column(&HELP[..2]);
    let right = help_column(&HELP[2..]);
    let h = left.len().max(right.len()) as u16 + 2;
    let rect = popup(area, 92, h);
    f.render_widget(Clear, rect);
    let block = pane(Line::from(" keys ".bold()), true)
        .title_bottom(Line::from(" custom keys: , to edit config · any key to close ").right_aligned().fg(DIM));
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let [l, r] = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(inner);
    f.render_widget(Paragraph::new(left), l);
    f.render_widget(Paragraph::new(right), r);
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
    let rect = popup(area, 92, form.fields.len() as u16 + 9);
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
            spans.push(Span::styled(field.input.clone(), Style::new().fg(Color::Yellow).bold()));
            if active {
                let x = row.x + 2 + LABEL_W as u16 + field.input.chars().count() as u16;
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

    let cmd = match form.kind {
        FormKind::GhRepoCreate => crate::git::gh_repo_create_cmd(form),
    };
    f.render_widget(
        Paragraph::new(vec![
            Line::from("  command".fg(DIM)),
            Line::from(vec![Span::styled("  $ ", Style::new().fg(DIM)), Span::raw(cmd)]),
        ])
        .wrap(Wrap { trim: false }),
        cmd_area,
    );

    let button = if form.on_confirm_step() {
        Line::from(vec![
            Span::styled("  ▶ Enter: create  ", Style::new().bg(Color::Green).fg(Color::Black).bold()),
            Span::styled("   ↑ back · Esc cancel", Style::new().fg(DIM)),
        ])
    } else {
        Line::from("  Enter accept & next · type to replace · ↑/↓ move · Esc cancel".fg(DIM))
    };
    f.render_widget(button, button_area);

    let on_dir_field = form.fields.get(form.step).is_some_and(|fl| fl.dir);
    if let Some(c) = form.completer.as_ref().filter(|_| on_dir_field) {
        draw_completions(f, c, fields_area.x + 1 + LABEL_W as u16, fields_area.y + form.step as u16, false);
    }
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
