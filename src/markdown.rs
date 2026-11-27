//! Markdown for the post editor: highlighting the source as you type, and a rendered preview.
//! Covers what blog posts use (headings, emphasis, code, links, lists, quotes, fences,
//! Liquid tags), not all of CommonMark.

use std::ops::Range;

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

use crate::textarea::wrap;

const MARK: Style = Style::new().fg(Color::DarkGray);
const CODE: Style = Style::new().fg(Color::Yellow);
const LINK: Style = Style::new().fg(Color::Blue).add_modifier(Modifier::UNDERLINED);
const LIQUID: Style = Style::new().fg(Color::Magenta);
const BULLET: Style = Style::new().fg(Color::Yellow);
const QUOTE: Style = Style::new().fg(Color::Gray).add_modifier(Modifier::ITALIC);

/// Styled parts of a line: byte ranges, sorted and not overlapping. Gaps are plain.
pub type Segments = Vec<(Range<usize>, Style)>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Code,
    Strong,
    Em,
    Link,
    Image,
    Liquid,
    Escape,
}

/// One piece of inline markup in a line: all of it is `outer`, the content `inner`; for
/// links and images `extra` is the URL.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Token {
    kind: Kind,
    outer: Range<usize>,
    inner: Range<usize>,
    extra: Range<usize>,
}

/// The inline markup in `s`, left to right.
fn tokens(s: &str) -> Vec<Token> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if let Some(t) = token_at(s, i) {
            i = t.outer.end;
            out.push(t);
        } else {
            i += s[i..].chars().next().map_or(1, char::len_utf8);
        }
    }
    out
}

fn token_at(s: &str, i: usize) -> Option<Token> {
    let rest = &s[i..];
    let tok = |kind, open: usize, inner_end: usize, close: usize| Token {
        kind,
        outer: i..inner_end + close,
        inner: i + open..inner_end,
        extra: 0..0,
    };
    let find = |from: usize, pat: &str| s[from..].find(pat).map(|j| from + j);
    let word_char = |c: Option<char>| c.is_some_and(char::is_alphanumeric);
    match rest.as_bytes()[0] {
        b'\\' if rest.len() > 1 && rest.as_bytes()[1].is_ascii_punctuation() => Some(tok(Kind::Escape, 1, i + 2, 0)),
        b'`' => {
            let end = find(i + 1, "`")?;
            Some(tok(Kind::Code, 1, end, 1))
        }
        b'{' if rest.starts_with("{%") || rest.starts_with("{{") => {
            let close = if rest.starts_with("{%") { "%}" } else { "}}" };
            let end = find(i + 2, close)?;
            Some(tok(Kind::Liquid, 0, end + 2, 0))
        }
        b'*' | b'_' => {
            let c = rest.as_bytes()[0] as char;
            // `snake_case` isn't emphasis.
            if c == '_' && word_char(s[..i].chars().next_back()) {
                return None;
            }
            let double = rest.as_bytes().get(1) == Some(&(c as u8));
            let marker = if double { &rest[..2] } else { &rest[..1] };
            let start = i + marker.len();
            if s[start..].starts_with(char::is_whitespace) || s[start..].is_empty() {
                return None;
            }
            let mut from = start;
            loop {
                let end = find(from, marker)?;
                let after = s[end + marker.len()..].chars().next();
                let ok = end > start
                    && !s[..end].ends_with(char::is_whitespace)
                    && !(c == '_' && word_char(after))
                    && !(!double && after == Some(c));
                if ok {
                    let kind = if double { Kind::Strong } else { Kind::Em };
                    return Some(tok(kind, marker.len(), end, marker.len()));
                }
                from = end + marker.len();
            }
        }
        b'[' | b'!' => {
            let image = rest.starts_with("![");
            if !image && !rest.starts_with('[') {
                return None;
            }
            let open = if image { 2 } else { 1 };
            let close = find(i + open, "]")?;
            if !s[close + 1..].starts_with('(') {
                return None;
            }
            let url_end = find(close + 2, ")")?;
            Some(Token {
                kind: if image { Kind::Image } else { Kind::Link },
                outer: i..url_end + 1,
                inner: i + open..close,
                extra: close + 2..url_end,
            })
        }
        _ => None,
    }
}

// ── highlighting the source ────────────────────────────────────────────────

/// How a line of source looks in the editor; `fence` carries "inside a ``` block" from line
/// to line.
pub fn highlight(line: &str, fence: &mut bool) -> Segments {
    let trimmed = line.trim_start();
    let all = |style| vec![(0..line.len(), style)];
    if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
        *fence = !*fence;
        return all(MARK);
    }
    if *fence {
        return all(CODE);
    }
    if let Some(level) = heading_level(line) {
        return vec![(0..level + 1, MARK), (level + 1..line.len(), heading_style(level))];
    }
    if is_rule(line) {
        return all(MARK);
    }
    let mut segs = Vec::new();
    let mut start = 0;
    if let Some((marker_end, _)) = block_marker(line) {
        let style = if trimmed.starts_with('>') { MARK } else { BULLET };
        segs.push((0..marker_end, style));
        start = marker_end;
    }
    let base = if trimmed.starts_with('>') { QUOTE } else { Style::new() };
    inline_segments(line, start..line.len(), base, &mut segs);
    segs
}

/// Styles the inline markup in `s[range]`, markers dimmed, recursing into emphasis and links.
fn inline_segments(s: &str, range: Range<usize>, base: Style, segs: &mut Segments) {
    let part = &s[range.clone()];
    let off = range.start;
    let mut at = off;
    let plain = |segs: &mut Segments, from: usize, to: usize| {
        if from < to && base != Style::new() {
            segs.push((from..to, base));
        }
    };
    for t in tokens(part) {
        let (outer, inner) = (t.outer.start + off..t.outer.end + off, t.inner.start + off..t.inner.end + off);
        plain(segs, at, outer.start);
        match t.kind {
            Kind::Code => {
                segs.push((outer.start..inner.start, MARK));
                segs.push((inner.clone(), CODE));
                segs.push((inner.end..outer.end, MARK));
            }
            Kind::Liquid => segs.push((outer.clone(), LIQUID)),
            Kind::Escape => segs.push((outer.clone(), MARK)),
            Kind::Strong | Kind::Em => {
                let style = base.patch(emphasis(t.kind));
                segs.push((outer.start..inner.start, MARK));
                inline_segments(s, inner.clone(), style, segs);
                segs.push((inner.end..outer.end, MARK));
            }
            Kind::Link | Kind::Image => {
                segs.push((outer.start..inner.start, MARK));
                inline_segments(s, inner.clone(), base.patch(LINK), segs);
                segs.push((inner.end..outer.end, MARK));
            }
        }
        at = outer.end;
    }
    plain(segs, at, range.end);
}

fn emphasis(kind: Kind) -> Style {
    match kind {
        Kind::Strong => Style::new().add_modifier(Modifier::BOLD),
        _ => Style::new().add_modifier(Modifier::ITALIC),
    }
}

fn heading_style(level: usize) -> Style {
    let s = Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD);
    if level == 1 { s.add_modifier(Modifier::UNDERLINED) } else { s }
}

/// `## Title` → 2.
fn heading_level(line: &str) -> Option<usize> {
    let n = line.bytes().take_while(|&b| b == b'#').count();
    ((1..=6).contains(&n) && line[n..].starts_with(' ')).then_some(n)
}

fn is_rule(line: &str) -> bool {
    let t: String = line.chars().filter(|c| !c.is_whitespace()).collect();
    t.len() >= 3 && (t.chars().all(|c| c == '-') || t.chars().all(|c| c == '*') || t.chars().all(|c| c == '_'))
}

/// A list or quote marker at the start of `line`: where it ends, and how the preview shows it.
fn block_marker(line: &str) -> Option<(usize, String)> {
    let indent = line.len() - line.trim_start_matches(' ').len();
    let rest = &line[indent..];
    if rest.starts_with("> ") || rest == ">" {
        return Some((indent + rest.len().min(2), "│ ".into()));
    }
    let (mark_len, shown) =
        if rest.len() > 1 && matches!(rest.as_bytes()[0], b'-' | b'*' | b'+') && rest.as_bytes()[1] == b' ' {
            (2, "• ".to_string())
        } else {
            let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
            let tail = &rest[digits..];
            if digits == 0 || digits > 9 || !(tail.starts_with(". ") || tail.starts_with(") ")) {
                return None;
            }
            (digits + 2, format!("{}. ", &rest[..digits]))
        };
    let after = &rest[mark_len..];
    // A task's box takes the bullet's place; a numbered task keeps its number.
    let boxed = |b: &str| if shown == "• " { b.to_string() } else { format!("{shown}{b}") };
    let (task_len, shown) = match after.get(..4) {
        Some("[ ] ") => (4, boxed("☐ ")),
        Some("[x] " | "[X] ") => (4, boxed("☑ ")),
        _ => (0, shown),
    };
    let lead = " ".repeat(indent);
    Some((indent + mark_len + task_len, format!("{lead}{shown}")))
}

// ── preview ────────────────────────────────────────────────────────────────

/// Text with styled parts, markers already gone.
#[derive(Default)]
struct Styled {
    text: String,
    segs: Segments,
}

impl Styled {
    fn push(&mut self, s: &str, style: Style) {
        let start = self.text.len();
        self.text.push_str(s);
        if style != Style::new() && !s.is_empty() {
            self.segs.push((start..self.text.len(), style));
        }
    }
}

fn render_inline(s: &str, base: Style, out: &mut Styled) {
    let mut at = 0;
    for t in tokens(s) {
        out.push(&s[at..t.outer.start], base);
        match t.kind {
            Kind::Code => out.push(&s[t.inner.clone()], base.patch(CODE)),
            Kind::Liquid => out.push(&s[t.outer.clone()], base.patch(MARK)),
            Kind::Escape => out.push(&s[t.inner.clone()], base),
            Kind::Strong | Kind::Em => render_inline(&s[t.inner.clone()], base.patch(emphasis(t.kind)), out),
            Kind::Link => render_inline(&s[t.inner.clone()], base.patch(LINK), out),
            Kind::Image => out.push(&format!("[image: {}]", &s[t.inner.clone()]), base.patch(MARK)),
        }
        at = t.outer.end;
    }
    out.push(&s[at..], base);
}

/// The preview of `text` at `width` columns, and for each block the source line it starts
/// on with the preview row it starts at, to scroll the preview along with the cursor.
pub fn preview(text: &str, width: usize) -> (Vec<Line<'static>>, Vec<(usize, usize)>) {
    let width = width.max(10);
    let lines: Vec<&str> = text.split('\n').collect();
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut anchors = Vec::new();
    let mut i = 0;
    // List items follow each other directly; other blocks are separated by one empty row.
    let mut tight = false;
    let gap = |out: &mut Vec<Line<'static>>| {
        if out.last().is_some_and(|l| l.width() > 0) {
            out.push(Line::default());
        }
    };
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            i += 1;
            continue;
        }
        if !std::mem::take(&mut tight) {
            gap(&mut out);
        }
        anchors.push((i, out.len()));
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            let lang = trimmed.trim_start_matches(['`', '~']).trim();
            if !lang.is_empty() {
                out.push(Line::from(Span::styled(format!("  {lang}"), MARK)));
            }
            i += 1;
            while i < lines.len()
                && !(lines[i].trim_start().starts_with("```") || lines[i].trim_start().starts_with("~~~"))
            {
                let s = Styled { text: lines[i].to_string(), segs: vec![(0..lines[i].len(), CODE)] };
                out.extend(rows(&s, "  ", "  ", Style::new(), width));
                i += 1;
            }
            i += 1;
            continue;
        }
        if let Some(level) = heading_level(line) {
            let mut s = Styled::default();
            render_inline(line[level + 1..].trim(), heading_style(level), &mut s);
            out.extend(rows(&s, "", "", Style::new(), width));
            i += 1;
            continue;
        }
        if is_rule(line) {
            out.push(Line::from(Span::styled("─".repeat(width), MARK)));
            i += 1;
            continue;
        }
        if trimmed.starts_with("{%") && trimmed.ends_with("%}") {
            out.push(Line::from(Span::styled(trimmed.to_string(), LIQUID)));
            i += 1;
            continue;
        }
        // A list item or quote with the lines that continue it, or a paragraph.
        let marker = block_marker(line);
        let quote = trimmed.starts_with('>');
        let start = marker.as_ref().map_or(0, |(end, _)| *end);
        let mut joined = line[start..].trim().to_string();
        i += 1;
        while i < lines.len() {
            let next = lines[i];
            let t = next.trim_start();
            let continues = !t.is_empty()
                && heading_level(next).is_none()
                && !t.starts_with("```")
                && !is_rule(next)
                && match (&marker, quote) {
                    (_, true) => t.starts_with('>'),
                    (Some(_), false) => block_marker(next).is_none() && next.starts_with(' '),
                    (None, false) => block_marker(next).is_none(),
                };
            if !continues {
                break;
            }
            let text = if quote { t.trim_start_matches('>').trim() } else { t.trim() };
            joined.push(' ');
            joined.push_str(text);
            i += 1;
        }
        let base = if quote { QUOTE } else { Style::new() };
        let mut s = Styled::default();
        render_inline(&joined, base, &mut s);
        match marker {
            Some((_, shown)) if quote => out.extend(rows(&s, &shown, &shown, MARK, width)),
            Some((_, shown)) => {
                let rest = " ".repeat(shown.chars().count());
                out.extend(rows(&s, &shown, &rest, BULLET, width));
            }
            None => out.extend(rows(&s, "", "", Style::new(), width)),
        }
        let is_item = |l: &str| block_marker(l).is_some() && !l.trim_start().starts_with('>');
        tight = is_item(line) && lines.get(i).is_some_and(|n| is_item(n));
    }
    (out, anchors)
}

/// `s` wrapped to `width`, behind `first` on the first row and `rest` on the others.
fn rows(s: &Styled, first: &str, rest: &str, lead_style: Style, width: usize) -> Vec<Line<'static>> {
    let lead_w = first.chars().count().max(rest.chars().count());
    let starts = wrap(&s.text, width.saturating_sub(lead_w).max(1));
    starts
        .iter()
        .enumerate()
        .map(|(n, &from)| {
            let to = starts.get(n + 1).copied().unwrap_or(s.text.len());
            let lead = if n == 0 { first } else { rest };
            let mut spans = vec![Span::styled(lead.to_string(), lead_style)];
            spans.extend(spans_in(&s.text, &s.segs, from..to, Style::new()));
            Line::from(spans)
        })
        .collect()
}

/// The spans of `text[range]`, styled by `segs` (gaps get `base`).
pub fn spans_in(text: &str, segs: &Segments, range: Range<usize>, base: Style) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    let mut at = range.start;
    for (r, style) in segs {
        let (from, to) = (r.start.max(range.start), r.end.min(range.end));
        if from >= to {
            continue;
        }
        if at < from {
            out.push(Span::styled(text[at..from].to_string(), base));
        }
        out.push(Span::styled(text[from..to].to_string(), base.patch(*style)));
        at = to;
    }
    if at < range.end {
        out.push(Span::styled(text[at..range.end].to_string(), base));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(s: &str) -> Vec<(Kind, &str)> {
        tokens(s).into_iter().map(|t| (t.kind, &s[t.inner])).collect()
    }

    fn plain(l: &Line) -> String {
        l.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn finds_inline_markup() {
        assert_eq!(
            kinds("a **b** _c_ `d` [e](f) ![g](h) {% i %}"),
            vec![
                (Kind::Strong, "b"),
                (Kind::Em, "c"),
                (Kind::Code, "d"),
                (Kind::Link, "e"),
                (Kind::Image, "g"),
                (Kind::Liquid, "{% i %}"),
            ]
        );
        // Not emphasis: snake_case, a lone star, a space after the marker, an unclosed one.
        assert!(kinds("snake_case_name 2 * 3 * 4 ** x **open").is_empty());
        assert_eq!(kinds(r"\*not em\*"), vec![(Kind::Escape, "*"), (Kind::Escape, "*")]);
        assert_eq!(kinds("[text] (not a link)"), vec![]);
    }

    #[test]
    fn highlights_markers_dim_and_content_styled() {
        let mut fence = false;
        let line = "- **fett** und `code`";
        let segs = highlight(line, &mut fence);
        let shown: Vec<(&str, Style)> = segs.iter().map(|(r, s)| (&line[r.clone()], *s)).collect();
        assert_eq!(shown[0], ("- ", BULLET));
        assert_eq!(shown[1], ("**", MARK));
        assert!(shown[2].0 == "fett" && shown[2].1.add_modifier.contains(Modifier::BOLD));
        assert_eq!(&shown[4..], &[("`", MARK), ("code", CODE), ("`", MARK)]);

        assert_eq!(highlight("## Titel", &mut fence)[1], (3..8, heading_style(2)));
        assert_eq!(highlight("```rust", &mut fence), vec![(0..7, MARK)]);
        assert_eq!(highlight("let **x** = 1;", &mut fence), vec![(0..14, CODE)]);
        highlight("```", &mut fence);
        assert!(!fence);
        // Segments never overlap and stay inside the line.
        let line = "> _quote **mit** [link](u)_ \\* ende";
        let segs = highlight(line, &mut fence);
        assert!(segs.windows(2).all(|w| w[0].0.end <= w[1].0.start), "{segs:?}");
        assert!(segs.last().unwrap().0.end <= line.len());
    }

    #[test]
    fn previews_blocks() {
        let text = "# Titel\n\nEin **fetter**\nAbsatz mit [Link](https://x).\n\n- eins\n- zwei\n  weiter\n\n\
                    > Zitat\n> zwei\n\n```sh\nls -la\n```\n---\n1. [ ] offen\n![Bild](a.png)";
        let (lines, anchors) = preview(text, 60);
        let shown: Vec<String> = lines.iter().map(plain).collect();
        assert_eq!(
            shown,
            vec![
                "Titel",
                "",
                "Ein fetter Absatz mit Link.",
                "",
                "• eins",
                "• zwei weiter",
                "",
                "│ Zitat zwei",
                "",
                "  sh",
                "  ls -la",
                "",
                "─".repeat(60).as_str(),
                "",
                "1. ☐ offen",
                "",
                "[image: Bild]",
            ]
        );
        assert_eq!(anchors[0], (0, 0));
        assert_eq!(anchors[1], (2, 2));
        assert_eq!(anchors[2], (5, 4));
        assert_eq!(anchors[3], (6, 5));
    }

    #[test]
    fn preview_wraps_with_hanging_indent() {
        let (lines, _) = preview("- one two three four", 12);
        let shown: Vec<String> = lines.iter().map(plain).collect();
        assert_eq!(shown, vec!["• one two ", "  three four"]);
    }

    #[test]
    fn spans_cut_at_ranges() {
        let text = "abcdef";
        let segs = vec![(1..4, CODE)];
        let spans = spans_in(text, &segs, 2..6, Style::new());
        let parts: Vec<(&str, Style)> = spans.iter().map(|s| (s.content.as_ref(), s.style)).collect();
        assert_eq!(parts, vec![("cd", CODE), ("ef", Style::new())]);
    }
}
