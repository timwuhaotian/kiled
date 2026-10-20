//! Changes to the text. Every command reduces to a set of changes that apply at once to the
//! same text: sorted, non-overlapping, in char offsets of the text before any of them.

use std::ops::Range;

/// `range` of the text before the edit becomes `text`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    /// What is replaced.
    pub range: Range<usize>,
    /// What replaces it.
    pub text: String,
}

impl Change {
    /// Inserts `text` at `at`.
    pub fn insert(at: usize, text: impl Into<String>) -> Self {
        Self {
            range: at..at,
            text: text.into(),
        }
    }

    /// Deletes `range`.
    #[must_use]
    pub fn delete(range: Range<usize>) -> Self {
        Self {
            range,
            text: String::new(),
        }
    }

    /// How many chars the text grows by (negative: shrinks).
    #[must_use]
    pub fn delta(&self) -> isize {
        len_chars(&self.text).cast_signed() - self.range.len().cast_signed()
    }

    /// Changes nothing.
    #[must_use]
    pub fn is_noop(&self) -> bool {
        self.range.is_empty() && self.text.is_empty()
    }
}

/// Chars in `text`.
#[must_use]
pub fn len_chars(text: &str) -> usize {
    text.chars().count()
}

/// Which side of text inserted exactly at an offset the offset ends up on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bias {
    /// Stays before inserted text.
    Before,
    /// Moves past inserted text.
    After,
}

/// Sorts `changes` and merges any that overlap (or both insert at one place), so they can
/// apply at once. Merged changes replace the union of their ranges with their texts in order.
#[must_use]
pub fn coalesce(mut changes: Vec<Change>) -> Vec<Change> {
    changes.retain(|change| !change.is_noop());
    changes.sort_by_key(|change| (change.range.start, change.range.end));
    let mut out: Vec<Change> = Vec::with_capacity(changes.len());
    for change in changes {
        if let Some(last) = out.last_mut()
            && (change.range.start < last.range.end
                || (change.range.start == last.range.start && change.range.start == last.range.end))
        {
            last.range.end = last.range.end.max(change.range.end);
            last.text.push_str(&change.text);
            continue;
        }
        out.push(change);
    }
    out
}

/// Where `offset` in the text before `changes` is after them. An offset inside a replaced
/// range goes to the replacement's start (`Before`) or end (`After`).
#[must_use]
pub fn map(changes: &[Change], offset: usize, bias: Bias) -> usize {
    let mut shift: isize = 0;
    for change in changes {
        if offset < change.range.start
            || (offset == change.range.start && bias == Bias::Before && change.range.is_empty())
        {
            break;
        }
        if offset < change.range.end || (offset == change.range.end && change.range.is_empty()) {
            let start = change.range.start.saturating_add_signed(shift);
            return match bias {
                Bias::Before => start,
                Bias::After => start + len_chars(&change.text),
            };
        }
        // At or after the end of a replaced range: right after its replacement.
        shift += change.delta();
    }
    offset.saturating_add_signed(shift)
}

/// Where each change's replacement starts in the text after all of them.
#[must_use]
pub fn new_starts(changes: &[Change]) -> Vec<usize> {
    let mut shift: isize = 0;
    changes
        .iter()
        .map(|change| {
            let start = change.range.start.saturating_add_signed(shift);
            shift += change.delta();
            start
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_should_shift_offsets_after_a_change() {
        let changes = [Change::insert(2, "abc"), Change::delete(5..7)];
        assert_eq!(
            [0, 2, 4, 6, 9].map(|offset| map(&changes, offset, Bias::After)),
            [0, 5, 7, 8, 10]
        );
    }

    #[test]
    fn map_should_respect_the_bias_at_an_insertion_point() {
        let changes = [Change::insert(3, "xy")];
        assert_eq!(
            (
                map(&changes, 3, Bias::Before),
                map(&changes, 3, Bias::After)
            ),
            (3, 5)
        );
    }

    #[test]
    fn map_should_send_an_offset_inside_a_replacement_to_its_ends() {
        let changes = [Change {
            range: 2..6,
            text: "z".into(),
        }];
        assert_eq!(
            (
                map(&changes, 4, Bias::Before),
                map(&changes, 4, Bias::After),
                map(&changes, 6, Bias::Before),
                map(&changes, 8, Bias::Before)
            ),
            (2, 3, 3, 5)
        );
    }

    #[test]
    fn coalesce_should_merge_overlapping_deletes() {
        let merged = coalesce(vec![Change::delete(4..6), Change::delete(2..5)]);
        assert_eq!(merged, vec![Change::delete(2..6)]);
    }

    #[test]
    fn new_starts_should_account_for_earlier_changes() {
        let changes = [Change::insert(1, "ab"), Change::delete(3..5)];
        assert_eq!(new_starts(&changes), vec![1, 5]);
    }
}
