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
                match chars.next() {
                    Some('[') => {}
                    // OSC (e.g. cargo's OSC 8 hyperlinks): skip to BEL or ST (ESC \).
                    Some(']') => {
                        while let Some(o) = chars.next() {
                            if o == '\x07' || (o == '\x1b' && chars.next_if_eq(&'\\').is_some()) {
                                break;
                            }
                        }
                        continue;
                    }
                    // Charset selection like ESC ( B carries one more byte.
                    Some('(' | ')') => {
                        chars.next();
                        continue;
                    }
                    _ => continue,
                }
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
            9 => style.add_modifier(Modifier::CROSSED_OUT),
            22 => style.remove_modifier(Modifier::BOLD | Modifier::DIM),
            23 => style.remove_modifier(Modifier::ITALIC),
            24 => style.remove_modifier(Modifier::UNDERLINED),
            27 => style.remove_modifier(Modifier::REVERSED),
            29 => style.remove_modifier(Modifier::CROSSED_OUT),
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

/// The way back: a line as text with SGR codes, so saved output keeps its colors and
/// `parse_line` reads it in again.
pub fn encode_line(line: &Line) -> String {
    let mut out = String::new();
    let mut styled = false;
    for span in &line.spans {
        let style = line.style.patch(span.style);
        let codes = sgr(style);
        if !codes.is_empty() || styled {
            out.push_str(&format!("\x1b[0{codes}m"));
            styled = !codes.is_empty();
        }
        // Escapes can't be in parsed text, but a hand-built line could carry one.
        out.extend(span.content.chars().filter(|&c| c != '\x1b'));
    }
    if styled {
        out.push_str("\x1b[0m");
    }
    out
}

/// `;1;31`-style parameters for `style`, empty for the default look.
fn sgr(style: Style) -> String {
    let mut codes = String::new();
    for (m, code) in [
        (Modifier::BOLD, 1),
        (Modifier::DIM, 2),
        (Modifier::ITALIC, 3),
        (Modifier::UNDERLINED, 4),
        (Modifier::REVERSED, 7),
        (Modifier::CROSSED_OUT, 9),
    ] {
        if style.add_modifier.contains(m) {
            codes.push_str(&format!(";{code}"));
        }
    }
    if let Some(c) = style.fg.and_then(|c| color_code(c, false)) {
        codes.push_str(&c);
    }
    if let Some(c) = style.bg.and_then(|c| color_code(c, true)) {
        codes.push_str(&c);
    }
    codes
}

fn color_code(color: Color, bg: bool) -> Option<String> {
    let base = if bg { 10 } else { 0 };
    let basic = [Color::Black, Color::Red, Color::Green, Color::Yellow, Color::Blue, Color::Magenta, Color::Cyan, Color::Gray];
    let bright = [
        Color::DarkGray,
        Color::LightRed,
        Color::LightGreen,
        Color::LightYellow,
        Color::LightBlue,
        Color::LightMagenta,
        Color::LightCyan,
        Color::White,
    ];
    let lead = if bg { 48 } else { 38 };
    Some(match color {
        Color::Reset => return None,
        Color::Indexed(n) => format!(";{lead};5;{n}"),
        Color::Rgb(r, g, b) => format!(";{lead};2;{r};{g};{b}"),
        c => {
            if let Some(i) = basic.iter().position(|&b| b == c) {
                format!(";{}", 30 + base + i)
            } else {
                format!(";{}", 90 + base + bright.iter().position(|&b| b == c)?)
            }
        }
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    fn text(l: &Line) -> String {
        l.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn strips_osc8_hyperlinks() {
        // What cargo prints for "Finished `release` profile".
        let raw = "\x1b[1m\x1b[92mFinished\x1b[0m \x1b]8;;https://doc.rust-lang.org/cargo/x\x1b\\`release` profile\x1b]8;;\x1b\\ [optimized]";
        assert_eq!(text(&parse_line(raw)), "Finished `release` profile [optimized]");
        let bel = "a\x1b]8;;http://x\x07link\x1b]8;;\x07b";
        assert_eq!(text(&parse_line(bel)), "alinkb");
    }

    #[test]
    fn encodes_what_it_parses() {
        let raw = "\x1b[1;92mFinished\x1b[0m plain \x1b[38;5;208mindexed\x1b[48;2;1;2;3m rgb bg\x1b[0m \x1b[2;9;3mdim\x1b[0m";
        let line = parse_line(raw);
        assert_eq!(parse_line(&encode_line(&line)), line);
        // Unstyled lines stay plain text.
        assert_eq!(encode_line(&parse_line("just text")), "just text");
        // Line-wide styles end up on each span.
        let l = Line::from("x").style(Style::new().fg(Color::Red));
        assert_eq!(parse_line(&encode_line(&l)).spans[0].style.fg, Some(Color::Red));
    }

    #[test]
    fn keeps_colors() {
        let l = parse_line("\x1b[31mred\x1b[0m plain");
        assert_eq!(l.spans[0].style.fg, Some(Color::Red));
        assert_eq!(text(&l), "red plain");
    }
}
