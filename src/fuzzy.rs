//! Subsequence matching for the file finder: "apmo" finds `src/app/mod.rs`.

/// Scores `hay` against `query` (both expected lowercase), or None if the query's
/// characters don't all appear in order. Higher is better: consecutive runs, starts of
/// path segments and words, and matches in the file name count extra; long paths a little less.
pub fn score(query: &str, hay: &str) -> Option<i64> {
    if query.is_empty() {
        return Some(0);
    }
    let name_start = hay.rfind('/').map_or(0, |i| i + 1);
    let bytes = hay.as_bytes();
    let mut score = 0i64;
    let mut q = query.chars().peekable();
    let mut prev_end: Option<usize> = None;
    for (i, c) in hay.char_indices() {
        let Some(&want) = q.peek() else { break };
        if c != want {
            continue;
        }
        q.next();
        score += 1;
        if prev_end == Some(i) {
            score += 4;
        }
        if i == 0 || matches!(bytes[i - 1], b'/' | b'_' | b'-' | b'.' | b' ') {
            score += 3;
        }
        if i >= name_start {
            score += 2;
        }
        prev_end = Some(i + c.len_utf8());
    }
    if q.peek().is_some() {
        return None;
    }
    Some(score * 16 - hay.len() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_in_order_only() {
        assert!(score("apmo", "src/app/mod.rs").is_some());
        assert!(score("moap", "src/app/mod.rs").is_none());
        assert_eq!(score("", "anything"), Some(0));
    }

    #[test]
    fn prefers_file_names_and_runs() {
        let s = |h| score("main", h).unwrap();
        assert!(s("src/main.rs") > s("src/mainframe_helpers/x.rs"));
        assert!(s("src/main.rs") > s("m/a/i/n.rs"));
    }
}
