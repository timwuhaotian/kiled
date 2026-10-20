//! Cursors and selections. A document always has at least one; several make a multi-cursor
//! edit. They are kept sorted and never overlap.

use std::ops::Range;

/// One cursor, with the text it selects between `anchor` (where the selection started) and
/// `head` (where the cursor is). Both are char offsets; equal means nothing is selected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    /// The fixed end.
    pub anchor: usize,
    /// The end that moves; the cursor is drawn here.
    pub head: usize,
    /// The display column vertical motion aims for, kept while passing shorter lines.
    pub goal: Option<usize>,
    /// Creation order: the oldest is the primary one, which Escape keeps.
    pub id: u64,
}

impl Selection {
    /// A cursor at `offset`.
    #[must_use]
    pub fn cursor(offset: usize) -> Self {
        Self::new(offset, offset)
    }

    /// From `anchor` to `head`.
    #[must_use]
    pub fn new(anchor: usize, head: usize) -> Self {
        Self {
            anchor,
            head,
            goal: None,
            id: 0,
        }
    }

    /// The selected chars, in order.
    #[must_use]
    pub fn range(&self) -> Range<usize> {
        self.start()..self.end()
    }

    /// The lower end.
    #[must_use]
    pub fn start(&self) -> usize {
        self.anchor.min(self.head)
    }

    /// The upper end.
    #[must_use]
    pub fn end(&self) -> usize {
        self.anchor.max(self.head)
    }

    /// Nothing selected: just a cursor.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    /// The head is before the anchor (selected backwards).
    #[must_use]
    pub fn reversed(&self) -> bool {
        self.head < self.anchor
    }

    /// The same selection with its head moved to `head`, keeping the anchor when `extend`.
    #[must_use]
    pub fn moved(self, head: usize, extend: bool) -> Self {
        Self {
            anchor: if extend { self.anchor } else { head },
            head,
            goal: None,
            id: self.id,
        }
    }

    /// The same selection with its ends replaced, keeping its identity and direction.
    #[must_use]
    pub fn with_range(self, range: Range<usize>) -> Self {
        let (anchor, head) = if self.reversed() {
            (range.end, range.start)
        } else {
            (range.start, range.end)
        };
        Self {
            anchor,
            head,
            goal: None,
            id: self.id,
        }
    }
}

/// Every selection of a document: sorted by start, non-overlapping, never empty.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selections {
    all: Vec<Selection>,
    next_id: u64,
}

impl Default for Selections {
    fn default() -> Self {
        Self::single(Selection::cursor(0))
    }
}

impl Selections {
    /// Just `selection`.
    #[must_use]
    pub fn single(selection: Selection) -> Self {
        Self {
            all: vec![Selection { id: 0, ..selection }],
            next_id: 1,
        }
    }

    /// `selections`, merged where they overlap. Ids are kept, so the primary stays the same
    /// one; new selections should come from [`Self::add`] or carry an id of `u64::MAX`.
    #[must_use]
    pub fn from_vec(mut selections: Vec<Selection>, next_id: u64) -> Self {
        if selections.is_empty() {
            selections.push(Selection::cursor(0));
        }
        let mut next_id = next_id;
        for selection in &mut selections {
            if selection.id == u64::MAX {
                selection.id = next_id;
                next_id += 1;
            }
        }
        let mut this = Self {
            all: selections,
            next_id,
        };
        this.normalize();
        this
    }

    /// Selections in document order.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = &Selection> {
        self.all.iter()
    }

    /// Selections in document order.
    #[must_use]
    pub fn as_slice(&self) -> &[Selection] {
        &self.all
    }

    /// How many there are.
    #[must_use]
    pub fn len(&self) -> usize {
        self.all.len()
    }

    /// Never true; here for clippy's sake.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.all.is_empty()
    }

    /// The id the next added selection gets.
    #[must_use]
    pub fn next_id(&self) -> u64 {
        self.next_id
    }

    /// The oldest selection: what Escape keeps, and what the status bar describes.
    #[must_use]
    pub fn primary(&self) -> &Selection {
        self.all
            .iter()
            .min_by_key(|selection| selection.id)
            .unwrap_or(&self.all[0])
    }

    /// The most recently added selection: where the view scrolls after adding one.
    #[must_use]
    pub fn newest(&self) -> &Selection {
        self.all
            .iter()
            .max_by_key(|selection| selection.id)
            .unwrap_or(&self.all[0])
    }

    /// Replaces each selection with `f` of it, then merges any that now overlap.
    pub fn map(&mut self, f: impl FnMut(&Selection) -> Selection) {
        self.all = self.all.iter().map(f).collect();
        self.normalize();
    }

    /// Adds `selection` as the newest.
    pub fn add(&mut self, selection: Selection) {
        self.all.push(Selection {
            id: self.next_id,
            ..selection
        });
        self.next_id += 1;
        self.normalize();
    }

    /// Only the primary selection remains.
    pub fn keep_primary(&mut self) {
        let primary = *self.primary();
        self.all = vec![primary];
    }

    /// Sorts and merges. Two selections merge when they overlap or, if either is a bare
    /// cursor, when they touch; the merged one keeps the older id and the later one's
    /// direction.
    fn normalize(&mut self) {
        self.all
            .sort_by_key(|selection| (selection.start(), selection.end()));
        let mut merged: Vec<Selection> = Vec::with_capacity(self.all.len());
        for selection in self.all.drain(..) {
            if let Some(last) = merged.last_mut() {
                let touching =
                    selection.start() == last.end() && (selection.is_empty() || last.is_empty());
                if selection.start() < last.end() || touching {
                    let range = last.start()..last.end().max(selection.end());
                    let id = last.id.min(selection.id);
                    let base = if selection.is_empty() {
                        *last
                    } else {
                        selection
                    };
                    *last = Selection {
                        id,
                        ..base.with_range(range)
                    };
                    continue;
                }
            }
            merged.push(selection);
        }
        self.all = merged;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ranges(selections: &Selections) -> Vec<Range<usize>> {
        selections.iter().map(Selection::range).collect()
    }

    #[test]
    fn overlapping_selections_should_merge_and_keep_the_older_id() {
        let mut selections = Selections::single(Selection::new(0, 5));
        selections.add(Selection::new(3, 8));
        let merged = ranges(&selections);
        assert_eq!(
            (
                merged.len(),
                merged.first().cloned(),
                selections.primary().id
            ),
            (1, Some(0..8), 0)
        );
    }

    #[test]
    fn cursors_at_the_same_place_should_merge() {
        let mut selections = Selections::single(Selection::cursor(4));
        selections.add(Selection::cursor(4));
        assert_eq!(selections.len(), 1);
    }

    #[test]
    fn adjacent_selections_should_stay_apart() {
        let mut selections = Selections::single(Selection::new(0, 3));
        selections.add(Selection::new(3, 6));
        assert_eq!(ranges(&selections), vec![0..3, 3..6]);
    }

    #[test]
    fn selections_should_stay_sorted_and_know_the_newest() {
        let mut selections = Selections::single(Selection::cursor(10));
        selections.add(Selection::cursor(2));
        assert_eq!(
            (
                ranges(&selections),
                selections.primary().head,
                selections.newest().head
            ),
            (vec![2..2, 10..10], 10, 2)
        );
    }

    #[test]
    fn keep_primary_should_drop_the_others() {
        let mut selections = Selections::single(Selection::cursor(10));
        selections.add(Selection::cursor(2));
        selections.keep_primary();
        assert_eq!(ranges(&selections), vec![10..10]);
    }
}
