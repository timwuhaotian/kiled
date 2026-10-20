//! Fuzzy matching for Quick Open and the Command Palette: the query's chars must appear in
//! order; matches at word starts, in a row, and in the file name rank higher.

/// How well `query` matches `candidate` (higher is better) and which char indices matched,
/// or `None` if it doesn't. Case is ignored; spaces in the query are ignored.
#[must_use]
pub fn score(query: &str, candidate: &str) -> Option<(i64, Vec<usize>)> {
    let query: Vec<char> = query
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect();
    let chars: Vec<char> = candidate.chars().collect();
    if query.is_empty() {
        return Some((0, Vec::new()));
    }
    // The file name is where a match counts most; try it alone first.
    let name_start = chars
        .iter()
        .rposition(|ch| *ch == '/')
        .map_or(0, |slash| slash + 1);
    let (positions, in_name) = match matched(&query, &chars, name_start) {
        Some(positions) => (positions, true),
        None => (matched(&query, &chars, 0)?, false),
    };
    let mut total: i64 = if in_name { 40 } else { 0 };
    let mut previous: Option<usize> = None;
    for &at in &positions {
        let before = at.checked_sub(1).map(|index| chars[index]);
        let boundary = match before {
            None => true,
            Some(prev) => {
                matches!(prev, '/' | '_' | '-' | '.' | ' ')
                    || (prev.is_lowercase() && chars[at].is_uppercase())
            }
        };
        if boundary {
            total += 16;
        }
        match previous {
            Some(previous) if previous + 1 == at => total += 12,
            Some(previous) => {
                total -= i64::try_from((at - previous - 1).min(10)).unwrap_or(10);
            }
            None => total -= i64::try_from(at.saturating_sub(name_start).min(10)).unwrap_or(10),
        }
        previous = Some(at);
    }
    total -= i64::try_from(chars.len() / 8).unwrap_or(0);
    Some((total, positions))
}

/// Indices of `query` in `chars` from `from` on, each the first possible after the last.
fn matched(query: &[char], chars: &[char], from: usize) -> Option<Vec<usize>> {
    let mut positions = Vec::with_capacity(query.len());
    let mut at = from;
    for &wanted in query {
        let found = (at..chars.len()).find(|&index| lower(chars[index]) == wanted)?;
        positions.push(found);
        at = found + 1;
    }
    Some(positions)
}

fn lower(ch: char) -> char {
    ch.to_lowercase().next().unwrap_or(ch)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rank<'a>(query: &str, candidates: &[&'a str]) -> Vec<&'a str> {
        let mut scored: Vec<(i64, &str)> = candidates
            .iter()
            .filter_map(|candidate| score(query, candidate).map(|(score, _)| (score, *candidate)))
            .collect();
        scored.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
        scored.into_iter().map(|(_, candidate)| candidate).collect()
    }

    #[test]
    fn chars_must_appear_in_order() {
        assert!(score("mr", "src/main.rs").is_some());
        assert!(score("ms", "src/main.rs").is_some() && score("sm", "main.rs").is_none());
    }

    #[test]
    fn file_names_and_word_starts_should_rank_first() {
        assert_eq!(
            rank(
                "edit",
                &[
                    "src/editor.rs",
                    "docs/credits.md",
                    "crates/kiled/src/edit_element.rs"
                ]
            ),
            vec![
                "src/editor.rs",
                "crates/kiled/src/edit_element.rs",
                "docs/credits.md"
            ]
        );
    }

    #[test]
    fn camel_case_humps_should_count_as_word_starts() {
        let (with_humps, _) = score("ws", "WorkSpace.ts").unwrap();
        let (without, _) = score("ws", "wishes.ts").unwrap();
        assert!(with_humps > without);
    }

    #[test]
    fn positions_should_point_at_the_matched_chars() {
        assert_eq!(score("ab", "x/a_b").unwrap().1, vec![2, 4]);
    }
}
