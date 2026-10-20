//! Where the cursor goes, as VS Code moves it: by char, by word, to the line's ends (Home
//! goes to the first non-blank, then to the start), and up and down by display column.

use ropey::Rope;

use crate::{
    selection::Selection,
    text::{self, is_word_char},
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Space,
    Newline,
    Word,
    Punctuation,
}

fn class(ch: char) -> Class {
    match ch {
        '\n' => Class::Newline,
        _ if ch.is_whitespace() => Class::Space,
        _ if is_word_char(ch) => Class::Word,
        _ => Class::Punctuation,
    }
}

fn char_at(rope: &Rope, offset: usize) -> Option<char> {
    (offset < rope.len_chars()).then(|| rope.char(offset))
}

fn char_before(rope: &Rope, offset: usize) -> Option<char> {
    offset.checked_sub(1).map(|before| rope.char(before))
}

/// One char left; a selection collapses to its start instead, unless extending.
#[must_use]
pub fn left(rope: &Rope, selection: &Selection, extend: bool) -> usize {
    let _ = rope;
    if !extend && !selection.is_empty() {
        selection.start()
    } else {
        selection.head.saturating_sub(1)
    }
}

/// One char right; a selection collapses to its end instead, unless extending.
#[must_use]
pub fn right(rope: &Rope, selection: &Selection, extend: bool) -> usize {
    if !extend && !selection.is_empty() {
        selection.end()
    } else {
        (selection.head + 1).min(rope.len_chars())
    }
}

/// The end of the next word (or run of punctuation). A line's end is a stop of its own.
pub fn word_right(rope: &Rope, offset: usize) -> usize {
    let mut at = offset;
    if char_at(rope, at) == Some('\n') {
        return at + 1;
    }
    while char_at(rope, at).is_some_and(|ch| class(ch) == Class::Space) {
        at += 1;
    }
    let Some(first) = char_at(rope, at).map(class) else {
        return at;
    };
    if first == Class::Newline {
        return at;
    }
    while char_at(rope, at).is_some_and(|ch| class(ch) == first) {
        at += 1;
    }
    at
}

/// The start of the previous word (or run of punctuation). A line's start is a stop of its
/// own.
pub fn word_left(rope: &Rope, offset: usize) -> usize {
    let mut at = offset;
    if char_before(rope, at) == Some('\n') {
        return at - 1;
    }
    while char_before(rope, at).is_some_and(|ch| class(ch) == Class::Space) {
        at -= 1;
    }
    let Some(first) = char_before(rope, at).map(class) else {
        return at;
    };
    if first == Class::Newline {
        return at;
    }
    while char_before(rope, at).is_some_and(|ch| class(ch) == first) {
        at -= 1;
    }
    at
}

/// The first non-blank char of the line; from there (or from inside the indentation), the
/// line's start.
#[must_use]
pub fn home(rope: &Rope, offset: usize) -> usize {
    let line = rope.char_to_line(offset);
    let start = rope.line_to_char(line);
    let first = start + text::leading_whitespace(rope, line).chars().count();
    let first = first.min(text::line_end(rope, line));
    if offset == first { start } else { first }
}

/// The end of the line, before its line break.
#[must_use]
pub fn end(rope: &Rope, offset: usize) -> usize {
    text::line_end(rope, rope.char_to_line(offset))
}

/// `lines` up (negative) or down from the head, at the selection's goal column. Past the
/// first line is the text's start, past the last its end. Returns the offset and the goal
/// to keep.
#[must_use]
pub fn vertical(rope: &Rope, selection: &Selection, lines: isize, tab: usize) -> (usize, usize) {
    let line = rope.char_to_line(selection.head);
    let goal = selection
        .goal
        .unwrap_or_else(|| text::column(rope, selection.head, tab));
    let Some(target) = line.checked_add_signed(lines) else {
        return (0, goal);
    };
    if target >= rope.len_lines() {
        return (rope.len_chars(), goal);
    }
    (text::offset_at_column(rope, target, goal, tab), goal)
}

/// The word (or run of punctuation, or of blanks) around `offset`, as a double-click
/// selects it. Prefers the word ending at `offset` when it sits between two runs.
#[must_use]
pub fn word_at(rope: &Rope, offset: usize) -> std::ops::Range<usize> {
    let (before, after) = (char_before(rope, offset), char_at(rope, offset));
    let pick = if after.is_some_and(is_word_char) || before.is_some_and(is_word_char) {
        Class::Word
    } else if let Some(ch) = after.filter(|ch| *ch != '\n') {
        class(ch)
    } else if let Some(ch) = before.filter(|ch| *ch != '\n') {
        class(ch)
    } else {
        return offset..offset;
    };
    let mut start = offset;
    while char_before(rope, start).is_some_and(|ch| class(ch) == pick) {
        start -= 1;
    }
    let mut end = offset;
    while char_at(rope, end).is_some_and(|ch| class(ch) == pick) {
        end += 1;
    }
    start..end
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_right_should_stop_at_word_ends_and_line_ends() {
        let rope = Rope::from("let foo.bar  = 1\nx");
        let mut stops = vec![];
        let mut at = 0;
        while at < rope.len_chars() {
            at = word_right(&rope, at);
            stops.push(at);
        }
        assert_eq!(stops, vec![3, 7, 8, 11, 14, 16, 17, 18]);
    }

    #[test]
    fn word_left_should_mirror_word_right() {
        let rope = Rope::from("a.b  cd\nx");
        let mut stops = vec![];
        let mut at = rope.len_chars();
        while at > 0 {
            at = word_left(&rope, at);
            stops.push(at);
        }
        assert_eq!(stops, vec![8, 7, 5, 2, 1, 0]);
    }

    #[test]
    fn home_should_toggle_between_the_first_non_blank_and_the_start() {
        let rope = Rope::from("    let x\n");
        assert_eq!((home(&rope, 8), home(&rope, 4), home(&rope, 2)), (4, 0, 4));
    }

    #[test]
    fn vertical_should_keep_the_goal_across_a_short_line() {
        let rope = Rope::from("abcdef\nab\nabcdef\n");
        let start = Selection::cursor(5);
        let (down, goal) = vertical(&rope, &start, 1, 4);
        let next = Selection {
            goal: Some(goal),
            ..Selection::cursor(down)
        };
        let (again, _) = vertical(&rope, &next, 1, 4);
        assert_eq!((down, again), (9, 15));
    }

    #[test]
    fn vertical_past_the_ends_should_go_to_the_text_ends() {
        let rope = Rope::from("abc\ndef");
        assert_eq!(
            (
                vertical(&rope, &Selection::cursor(2), -1, 4).0,
                vertical(&rope, &Selection::cursor(5), 1, 4).0
            ),
            (0, 7)
        );
    }

    #[test]
    fn word_at_should_select_the_word_under_or_before_the_cursor() {
        let rope = Rope::from("foo(bar_baz)  x");
        assert_eq!(
            (word_at(&rope, 5), word_at(&rope, 3), word_at(&rope, 13)),
            (4..11, 0..3, 12..14)
        );
    }
}
