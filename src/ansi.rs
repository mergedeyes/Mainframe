//! Minimal ANSI SGR parser so colored compiler output survives in the output pane.

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

pub fn parse_line(raw: &str) -> Line<'static> {
    // Progress bars redraw with \r; only the last frame matters.
    let raw = raw.trim_end_matches('\r');
    let raw = raw.rsplit('\r').next().unwrap_or(raw);

    let mut spans = Vec::new();
    let mut style = Style::default();
    let mut buf = String::new();
    let mut chars = raw.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '\x1b' => {
                if chars.peek() != Some(&'[') {
                    chars.next();
                    continue;
                }
                chars.next();
                let mut params = String::new();
                let mut fin = None;
                for p in chars.by_ref() {
                    if ('@'..='~').contains(&p) {
                        fin = Some(p);
                        break;
                    }
                    params.push(p);
                }
                if fin == Some('m') {
                    if !buf.is_empty() {
                        spans.push(Span::styled(std::mem::take(&mut buf), style));
                    }
                    style = apply_sgr(style, &params);
                }
            }
            '\t' => buf.push_str("    "),
            c if c.is_control() => {}
            c => buf.push(c),
        }
    }
    if !buf.is_empty() {
        spans.push(Span::styled(buf, style));
    }
    Line::from(spans)
}

fn apply_sgr(mut style: Style, params: &str) -> Style {
    let nums: Vec<u16> = params.split(';').map(|p| p.parse().unwrap_or(0)).collect();
    let mut it = nums.into_iter();
    while let Some(n) = it.next() {
        style = match n {
            0 => Style::default(),
            1 => style.add_modifier(Modifier::BOLD),
            2 => style.add_modifier(Modifier::DIM),
            3 => style.add_modifier(Modifier::ITALIC),
            4 => style.add_modifier(Modifier::UNDERLINED),
            7 => style.add_modifier(Modifier::REVERSED),
            22 => style.remove_modifier(Modifier::BOLD | Modifier::DIM),
            23 => style.remove_modifier(Modifier::ITALIC),
            24 => style.remove_modifier(Modifier::UNDERLINED),
            27 => style.remove_modifier(Modifier::REVERSED),
            30..=37 => style.fg(basic(n - 30)),
            90..=97 => style.fg(bright(n - 90)),
            39 => style.fg(Color::Reset),
            40..=47 => style.bg(basic(n - 40)),
            100..=107 => style.bg(bright(n - 100)),
            49 => style.bg(Color::Reset),
            38 | 48 => match extended(&mut it) {
                Some(c) if n == 38 => style.fg(c),
                Some(c) => style.bg(c),
                None => style,
            },
            _ => style,
        };
    }
    style
}

fn extended(it: &mut impl Iterator<Item = u16>) -> Option<Color> {
    match it.next()? {
        5 => Some(Color::Indexed(it.next()? as u8)),
        2 => Some(Color::Rgb(it.next()? as u8, it.next()? as u8, it.next()? as u8)),
        _ => None,
    }
}

fn basic(n: u16) -> Color {
    [
        Color::Black,
        Color::Red,
        Color::Green,
        Color::Yellow,
        Color::Blue,
        Color::Magenta,
        Color::Cyan,
        Color::Gray,
    ][n as usize]
}

fn bright(n: u16) -> Color {
    [
        Color::DarkGray,
        Color::LightRed,
        Color::LightGreen,
        Color::LightYellow,
        Color::LightBlue,
        Color::LightMagenta,
        Color::LightCyan,
        Color::White,
    ][n as usize]
}
