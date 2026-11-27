//! Drawing: the screen is header, file list (+ preview), output pane and footer, with at
//! most one popup on top; each part lives in its own file.

mod browser;
mod cleanup;
mod deploy;
mod footer;
mod form;
mod header;
mod help;
mod installer;
mod notes_view;
mod output;
mod pickers;
mod post_editor;

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Position, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, BorderType, Clear, List, ListItem},
};

use crate::{
    app::{App, Mode},
    line::LineEdit,
    task::Task,
};

use self::{
    browser::{draw_browser, draw_preview},
    cleanup::draw_cleanup,
    deploy::draw_deploy,
    footer::draw_footer,
    form::draw_form,
    header::draw_header,
    help::{draw_git_menu, draw_help},
    installer::{draw_desktop_review, draw_install},
    notes_view::draw_notes,
    output::draw_output,
    pickers::{draw_blog_picker, draw_branch_picker, draw_file_finder, draw_project_picker, draw_task_picker},
    post_editor::draw_post_editor,
};

const ACCENT: Color = Color::Cyan;
const DIM: Color = Color::DarkGray;
/// Background of the selected row in lists.
const SELECTED: Color = Color::Rgb(40, 44, 52);
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
        Mode::Help => draw_help(f, f.area(), app.blog_root.is_some()),
        Mode::GitMenu => draw_git_menu(f, app.git.as_ref(), f.area()),
        Mode::Install(picker) => draw_install(f, picker, f.area()),
        Mode::Tasks(picker) => draw_task_picker(f, picker, f.area()),
        Mode::Branches(picker) => draw_branch_picker(f, picker, f.area()),
        Mode::Files(finder) => draw_file_finder(f, finder, f.area()),
        Mode::Projects(picker) => draw_project_picker(f, picker, f.area()),
        Mode::Notes(view) => draw_notes(f, view, f.area()),
        Mode::Form(form) => draw_form(f, form, f.area()),
        Mode::DesktopReview(review) => draw_desktop_review(f, review, f.area()),
        Mode::Deploy(picker) => draw_deploy(f, picker, f.area()),
        Mode::Cleanup(picker) => draw_cleanup(f, picker, f.area()),
        Mode::Blog(picker) => draw_blog_picker(f, picker, f.area()),
        Mode::Post(ed) => draw_post_editor(f, ed, f.area()),
        _ => {}
    }
}

fn pane(title: Line<'static>, focused: bool) -> Block<'static> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(if focused { ACCENT } else { DIM }))
        .title(title)
}

fn popup(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width.saturating_sub(2));
    let h = h.min(area.height.saturating_sub(2));
    Rect::new(area.x + (area.width - w) / 2, area.y + (area.height - h) / 2, w, h)
}

/// Key hints along the bottom border of a popup.
fn hints(keys: &str) -> Line<'static> {
    Line::from(keys.to_string()).right_aligned().fg(DIM)
}

/// Clears `rect` and draws a popup there, `keys` along its bottom border; returns the inside.
fn popup_pane(f: &mut Frame, rect: Rect, title: Line<'static>, keys: &str) -> Rect {
    f.render_widget(Clear, rect);
    let block = pane(title, true).title_bottom(hints(keys));
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    inner
}

/// The `›` line of a picker that filters as you type, with the cursor in it and, on the
/// right, how many items match.
fn filter_line(f: &mut Frame, filter: &LineEdit, count: Option<usize>, area: Rect) {
    f.render_widget(
        Line::from(vec![Span::styled(" › ", Style::new().fg(ACCENT).bold()), Span::raw(filter.text.clone())]),
        area,
    );
    if let Some(n) = count {
        f.render_widget(Line::from(format!("{n} ")).right_aligned().fg(DIM), area);
    }
    f.set_cursor_position(Position::new(
        (area.x + 3 + filter.col() as u16).min(area.right().saturating_sub(1)),
        area.y,
    ));
}

/// A list with the selection look every popup shares.
fn list(items: Vec<ListItem>) -> List {
    List::new(items).highlight_symbol("▌ ").highlight_style(Style::new().bg(SELECTED).add_modifier(Modifier::BOLD))
}

fn spinner(task: &Task) -> &'static str {
    spin_frame(task.started)
}

fn spin_frame(since: std::time::Instant) -> &'static str {
    SPINNER[(since.elapsed().as_millis() / 80) as usize % SPINNER.len()]
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
