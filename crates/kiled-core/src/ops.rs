//! The editing commands, as VS Code has them, applied at every selection at once
//! (DESIGN.md §4.3). Each reduces to one [`Document::transact`], so it undoes in one step.

use std::ops::Range;

use ropey::Rope;

use crate::{
    document::{Document, EditKind},
    edit::{self, Bias, Change, len_chars},
    movement,
    search::{self, Query},
    selection::{Selection, Selections},
    text::{self, Indent, is_word_char},
};

/// Brackets typed in pairs, in every language.
const BRACKETS: [(char, char); 3] = [('(', ')'), ('[', ']'), ('{', '}')];

/// An auto-closed pair opens only before one of these (or a line's end): typing `(` before
/// a word types just `(`.
const CLOSE_BEFORE: &str = ";:.,=}])> \t\n";

/// One selection's edit: `range` becomes `text`, and the selection lands at
/// `anchor`..`head`, counted in chars from where `text` starts.
struct Edit {
    range: Range<usize>,
    text: String,
    anchor: usize,
    head: usize,
}

impl Edit {
    fn replace(range: Range<usize>, text: String) -> Self {
        let len = len_chars(&text);
        Self {
            range,
            text,
            anchor: len,
            head: len,
        }
    }

    fn delete(range: Range<usize>) -> Self {
        Self::replace(range, String::new())
    }
}

/// Edits at each selection with `f` (`None` leaves it be), as one transaction.
fn edit_each(
    doc: &mut Document,
    kind: EditKind,
    mut f: impl FnMut(&Rope, &Selection) -> Option<Edit>,
) -> bool {
    let rope = doc.rope();
    let planned: Vec<(Selection, Option<Edit>)> = doc
        .selections()
        .iter()
        .map(|selection| (*selection, f(rope, selection)))
        .collect();
    let changes: Vec<Change> = planned
        .iter()
        .filter_map(|(_, edit)| edit.as_ref())
        .map(|edit| Change {
            range: edit.range.clone(),
            text: edit.text.clone(),
        })
        .collect();
    doc.transact(changes, kind, |changes, before| {
        let selections = planned
            .iter()
            .map(|(selection, edit)| match edit {
                Some(edit) => {
                    let start = edit::map(changes, edit.range.start, Bias::Before);
                    Selection {
                        anchor: start + edit.anchor,
                        head: start + edit.head,
                        goal: None,
                        id: selection.id,
                    }
                }
                None => Selection {
                    anchor: edit::map(changes, selection.anchor, Bias::After),
                    head: edit::map(changes, selection.head, Bias::After),
                    ..*selection
                },
            })
            .collect();
        Selections::from_vec(selections, before.next_id())
    })
}

/// Maps selections through line-start edits (indent, comment): a cursor moves with the text
/// it is in, a selection keeps whole lines selected.
fn keep_lines_selected(changes: &[Change], before: &Selections) -> Selections {
    let mut after = before.clone();
    after.map(|selection| {
        if selection.is_empty() {
            let head = edit::map(changes, selection.head, Bias::After);
            return Selection {
                anchor: head,
                head,
                ..*selection
            };
        }
        let start = edit::map(changes, selection.start(), Bias::Before);
        let end = edit::map(changes, selection.end(), Bias::After);
        selection.with_range(start..end)
    });
    after
}

/// Every selection moves its head with `motion`; `extend` keeps its anchor.
pub fn move_heads(
    doc: &mut Document,
    extend: bool,
    mut motion: impl FnMut(&Rope, &Selection) -> usize,
) {
    let rope = doc.rope().clone();
    let mut selections = doc.selections().clone();
    selections.map(|selection| selection.moved(motion(&rope, selection), extend));
    doc.set_selections(selections);
}

/// Every selection moves up or down `lines`, keeping its goal column.
pub fn move_vertically(doc: &mut Document, lines: isize, extend: bool) {
    let rope = doc.rope().clone();
    let tab = doc.indent().width;
    let mut selections = doc.selections().clone();
    selections.map(|selection| {
        // Without extending, a selection first collapses to the end it moves toward.
        let from = if extend || selection.is_empty() {
            *selection
        } else {
            let edge = if lines < 0 {
                selection.start()
            } else {
                selection.end()
            };
            Selection {
                head: edge,
                ..*selection
            }
        };
        let (head, goal) = movement::vertical(&rope, &from, lines, tab);
        Selection {
            goal: Some(goal),
            ..selection.moved(head, extend)
        }
    });
    doc.set_selections(selections);
}

/// Types `text` over every selection.
pub fn insert(doc: &mut Document, text: &str) -> bool {
    let text = text::normalize(text).0;
    edit_each(doc, EditKind::Typing, |_, selection| {
        Some(Edit::replace(selection.range(), text.clone()))
    })
}

fn closer_of(open: char, quotes: &str) -> Option<char> {
    BRACKETS
        .iter()
        .find(|(o, _)| *o == open)
        .map(|(_, close)| *close)
        .or_else(|| quotes.contains(open).then_some(open))
}

fn is_closer(ch: char, quotes: &str) -> bool {
    BRACKETS.iter().any(|(_, close)| *close == ch) || quotes.contains(ch)
}

fn char_at(rope: &Rope, offset: usize) -> Option<char> {
    (offset < rope.len_chars()).then(|| rope.char(offset))
}

fn char_before(rope: &Rope, offset: usize) -> Option<char> {
    offset.checked_sub(1).map(|before| rope.char(before))
}

/// The text of `line` before `offset`, when it is all blanks (and not empty).
fn blank_before(rope: &Rope, offset: usize) -> Option<(usize, String)> {
    let line_start = rope.line_to_char(rope.char_to_line(offset));
    let before = rope.slice(line_start..offset).to_string();
    (!before.is_empty() && before.chars().all(|ch| ch == ' ' || ch == '\t'))
        .then_some((line_start, before))
}

/// `indent` less one level.
fn outdented(indent: &str, unit: Indent) -> String {
    if let Some(rest) = indent.strip_suffix('\t') {
        return rest.to_owned();
    }
    let spaces = indent.chars().rev().take_while(|ch| *ch == ' ').count();
    let remove = match spaces % unit.width {
        0 => unit.width.min(spaces),
        partial => partial,
    };
    indent[..indent.len() - remove].to_owned()
}

/// Types one char as VS Code does: brackets and the language's quotes close themselves
/// before blanks and closers, wrap a selection, and are typed over when already there; a
/// closing bracket on a blank line outdents it.
pub fn type_char(doc: &mut Document, ch: char) -> bool {
    let quotes = doc.language().map_or("", |language| language.quotes);
    let unit = doc.indent();
    let rope = doc.rope();
    let type_over = is_closer(ch, quotes)
        && doc
            .selections()
            .iter()
            .all(|selection| selection.is_empty() && char_at(rope, selection.head) == Some(ch));
    if type_over {
        move_heads(doc, false, |_, selection| selection.head + 1);
        return true;
    }
    let opens = closer_of(ch, quotes);
    edit_each(doc, EditKind::Typing, |rope, selection| {
        if let Some(close) = opens {
            if !selection.is_empty() {
                let inner = rope.slice(selection.range()).to_string();
                let len = len_chars(&inner);
                return Some(Edit {
                    range: selection.range(),
                    text: format!("{ch}{inner}{close}"),
                    anchor: 1,
                    head: 1 + len,
                });
            }
            let before_ok =
                char_at(rope, selection.head).is_none_or(|next| CLOSE_BEFORE.contains(next));
            let quote_ok = close != ch
                || char_before(rope, selection.head)
                    .is_none_or(|prev| !is_word_char(prev) && prev != ch && prev != '\\');
            if before_ok && quote_ok {
                return Some(Edit {
                    range: selection.range(),
                    text: format!("{ch}{close}"),
                    anchor: 1,
                    head: 1,
                });
            }
        }
        if selection.is_empty()
            && BRACKETS.iter().any(|(_, close)| *close == ch)
            && let Some((line_start, before)) = blank_before(rope, selection.head)
        {
            return Some(Edit::replace(
                line_start..selection.head,
                format!("{}{ch}", outdented(&before, unit)),
            ));
        }
        Some(Edit::replace(selection.range(), ch.to_string()))
    })
}

/// Enter: the new line keeps the indentation, gains a level after an opening bracket (or
/// the language's other openers), and between a bracket pair the closer goes a line further
/// down. A line left blank keeps no trailing blanks.
pub fn newline(doc: &mut Document) -> bool {
    let unit = doc.indent();
    let openers = doc
        .language()
        .map_or("{[(", |language| language.indent_after);
    edit_each(doc, EditKind::Other, |rope, selection| {
        let line = rope.char_to_line(selection.start());
        let line_start = rope.line_to_char(line);
        let before = rope.slice(line_start..selection.start()).to_string();
        let indent: String = before
            .chars()
            .take_while(|ch| *ch == ' ' || *ch == '\t')
            .collect();
        let last = before.trim_end().chars().last();
        let opens = last.is_some_and(|ch| openers.contains(ch));
        let mut text = format!("\n{indent}");
        if opens {
            text.push_str(&unit.unit());
        }
        let cursor = len_chars(&text);
        let pair = last.and_then(|open| closer_of(open, ""));
        if opens && pair.is_some() && char_at(rope, selection.end()) == pair {
            text.push('\n');
            text.push_str(&indent);
        }
        let range = if before.trim().is_empty() {
            line_start..selection.end()
        } else {
            selection.range()
        };
        Some(Edit {
            range,
            text,
            anchor: cursor,
            head: cursor,
        })
    })
}

/// Backspace: the selection, else the char before. Inside an empty bracket or quote pair
/// both go; in leading spaces, back to the previous indent stop.
pub fn backspace(doc: &mut Document) -> bool {
    let quotes = doc.language().map_or("", |language| language.quotes);
    let unit = doc.indent();
    edit_each(doc, EditKind::Deleting, |rope, selection| {
        if !selection.is_empty() {
            return Some(Edit::delete(selection.range()));
        }
        let head = selection.head;
        let before = char_before(rope, head)?;
        if closer_of(before, quotes).is_some_and(|close| char_at(rope, head) == Some(close)) {
            return Some(Edit::delete(head - 1..head + 1));
        }
        if !unit.tabs
            && let Some((line_start, blanks)) = blank_before(rope, head)
            && !blanks.contains('\t')
        {
            let col = blanks.len();
            let stop = (col - 1) / unit.width * unit.width;
            return Some(Edit::delete(line_start + stop..head));
        }
        Some(Edit::delete(head - 1..head))
    })
}

/// Forward delete: the selection, else the char after.
pub fn delete(doc: &mut Document) -> bool {
    edit_each(doc, EditKind::Deleting, |rope, selection| {
        if !selection.is_empty() {
            return Some(Edit::delete(selection.range()));
        }
        (selection.head < rope.len_chars())
            .then(|| Edit::delete(selection.head..selection.head + 1))
    })
}

/// Deletes the selection, else from the cursor to where `motion` goes.
pub fn delete_to(doc: &mut Document, motion: impl Fn(&Rope, usize) -> usize) -> bool {
    edit_each(doc, EditKind::Deleting, |rope, selection| {
        if !selection.is_empty() {
            return Some(Edit::delete(selection.range()));
        }
        let to = motion(rope, selection.head);
        let range = to.min(selection.head)..to.max(selection.head);
        (!range.is_empty()).then(|| Edit::delete(range))
    })
}

/// Where Cmd-Backspace deletes back to: the line's start, or across the line break there.
#[must_use]
pub fn line_start_or_break(rope: &Rope, offset: usize) -> usize {
    let start = rope.line_to_char(rope.char_to_line(offset));
    if start == offset {
        offset.saturating_sub(1)
    } else {
        start
    }
}

/// Where Ctrl-K deletes up to: the line's end, or across the line break there.
#[must_use]
pub fn line_end_or_break(rope: &Rope, offset: usize) -> usize {
    let end = movement::end(rope, offset);
    if end == offset {
        (offset + 1).min(rope.len_chars())
    } else {
        end
    }
}

/// Lines a selection covers; one ending at a line's start doesn't cover that line.
fn covered_lines(rope: &Rope, selection: &Selection) -> Range<usize> {
    let first = rope.char_to_line(selection.start());
    let mut last = rope.char_to_line(selection.end());
    if last > first && rope.line_to_char(last) == selection.end() {
        last -= 1;
    }
    first..last + 1
}

/// Runs of whole lines the selections cover, merged where they touch.
fn line_blocks(doc: &Document) -> Vec<Range<usize>> {
    let mut blocks: Vec<Range<usize>> = Vec::new();
    for selection in doc.selections().iter() {
        let lines = covered_lines(doc.rope(), selection);
        match blocks.last_mut() {
            Some(last) if lines.start <= last.end => last.end = last.end.max(lines.end),
            _ => blocks.push(lines),
        }
    }
    blocks
}

fn lines_of(doc: &Document) -> Vec<usize> {
    line_blocks(doc).into_iter().flatten().collect()
}

fn is_blank(rope: &Rope, line: usize) -> bool {
    rope.line(line).chars().all(char::is_whitespace)
}

/// Tab: indents the lines of a selection spanning lines; otherwise types an indent to the
/// next stop.
pub fn tab(doc: &mut Document) -> bool {
    let rope = doc.rope();
    let multiline = doc
        .selections()
        .iter()
        .any(|selection| covered_lines(rope, selection).len() > 1);
    if multiline {
        return indent_lines(doc);
    }
    let unit = doc.indent();
    edit_each(doc, EditKind::Typing, |rope, selection| {
        let text = if unit.tabs {
            "\t".to_owned()
        } else {
            let col = text::column(rope, selection.start(), unit.width);
            " ".repeat(unit.width - col % unit.width)
        };
        Some(Edit::replace(selection.range(), text))
    })
}

/// Indents every covered line by one level; blank lines stay blank when there are several.
pub fn indent_lines(doc: &mut Document) -> bool {
    let unit = doc.indent().unit();
    let lines = lines_of(doc);
    let several = lines.len() > 1;
    let rope = doc.rope();
    let changes = lines
        .into_iter()
        .filter(|&line| !(several && is_blank(rope, line)))
        .map(|line| Change::insert(rope.line_to_char(line), unit.clone()))
        .collect();
    doc.transact(changes, EditKind::Other, keep_lines_selected)
}

/// Takes one level of indentation off every covered line.
pub fn outdent_lines(doc: &mut Document) -> bool {
    let unit = doc.indent();
    let rope = doc.rope();
    let changes = lines_of(doc)
        .into_iter()
        .filter_map(|line| {
            let indent = text::leading_whitespace(rope, line);
            let keep = outdented(&indent, unit);
            let start = rope.line_to_char(line);
            (keep.len() < indent.len())
                .then(|| Change::delete(start + len_chars(&keep)..start + len_chars(&indent)))
        })
        .collect();
    doc.transact(changes, EditKind::Other, keep_lines_selected)
}

/// Comments the covered lines out with the language's line comment, at their common
/// indentation, or, when every non-blank one already is, uncomments them.
pub fn toggle_line_comment(doc: &mut Document) -> bool {
    let Some(token) = doc.language().and_then(|language| language.line_comment) else {
        return false;
    };
    let rope = doc.rope();
    let lines = lines_of(doc);
    let mut targets: Vec<usize> = lines
        .iter()
        .copied()
        .filter(|&line| !is_blank(rope, line))
        .collect();
    if targets.is_empty() {
        targets = lines;
    }
    let commented = |line: usize| {
        let start = rope.line_to_char(line) + text::leading_whitespace(rope, line).chars().count();
        let len = len_chars(token);
        start + len <= rope.len_chars() && rope.slice(start..start + len) == token
    };
    let changes: Vec<Change> = if targets.iter().all(|&line| commented(line)) {
        targets
            .iter()
            .map(|&line| {
                let start =
                    rope.line_to_char(line) + text::leading_whitespace(rope, line).chars().count();
                let mut end = start + len_chars(token);
                if char_at(rope, end) == Some(' ') {
                    end += 1;
                }
                Change::delete(start..end)
            })
            .collect()
    } else {
        let column = targets
            .iter()
            .map(|&line| text::leading_whitespace(rope, line).chars().count())
            .min()
            .unwrap_or(0);
        targets
            .iter()
            .map(|&line| Change::insert(rope.line_to_char(line) + column, format!("{token} ")))
            .collect()
    };
    doc.transact(changes, EditKind::Other, keep_lines_selected)
}

/// The text of `lines` without the final line break.
fn block_text(rope: &Rope, lines: Range<usize>) -> String {
    let start = rope.line_to_char(lines.start);
    let end = text::line_end(rope, lines.end - 1);
    rope.slice(start..end).to_string()
}

/// Alt-Up / Alt-Down: the covered lines swap with the line above or below them.
pub fn move_lines(doc: &mut Document, up: bool) -> bool {
    let rope = doc.rope();
    let blocks = line_blocks(doc);
    let last_line = rope.len_lines() - 1;
    let blocked = if up {
        blocks.first().is_none_or(|block| block.start == 0)
    } else {
        blocks.last().is_none_or(|block| block.end > last_line)
    };
    if blocked {
        return false;
    }
    let mut changes = Vec::new();
    let mut shifts: Vec<(Range<usize>, isize)> = Vec::new();
    for block in &blocks {
        let region = if up {
            block.start - 1..block.end
        } else {
            block.start..block.end + 1
        };
        let start = rope.line_to_char(region.start);
        let end = text::line_end(rope, region.end - 1);
        let mut lines: Vec<String> = rope
            .slice(start..end)
            .to_string()
            .split('\n')
            .map(str::to_owned)
            .collect();
        let moved = if up {
            let above = lines.remove(0);
            let len = len_chars(&above);
            lines.push(above);
            -(len.cast_signed() + 1)
        } else {
            let below = lines.pop().unwrap_or_default();
            let len = len_chars(&below);
            lines.insert(0, below);
            len.cast_signed() + 1
        };
        let block_range = rope.line_to_char(block.start)..text::line_end(rope, block.end - 1) + 1;
        shifts.push((block_range, moved));
        changes.push(Change {
            range: start..end,
            text: lines.join("\n"),
        });
    }
    doc.transact(changes, EditKind::Other, |_, before| {
        let mut after = before.clone();
        after.map(|selection| {
            let shift = shifts
                .iter()
                .find(|(range, _)| range.contains(&selection.start()))
                .map_or(0, |(_, shift)| *shift);
            Selection {
                anchor: selection.anchor.saturating_add_signed(shift),
                head: selection.head.saturating_add_signed(shift),
                ..*selection
            }
        });
        after
    })
}

/// Shift-Alt-Up / Shift-Alt-Down: the covered lines are duplicated, and the selections go
/// with the copy in that direction: the upper one going up, the lower one going down.
pub fn copy_lines(doc: &mut Document, down: bool) -> bool {
    let rope = doc.rope();
    let blocks = line_blocks(doc);
    let mut changes = Vec::new();
    // Each block's chars, and how far into the text after it its selections land: by the
    // copy's length going down, as the copy then follows the block.
    let mut copies: Vec<(Range<usize>, usize)> = Vec::new();
    for block in &blocks {
        let text = block_text(rope, block.clone());
        let start = rope.line_to_char(block.start);
        let end = text::line_end(rope, block.end - 1);
        if down {
            changes.push(Change::insert(end, format!("\n{text}")));
            copies.push((start..end + 1, len_chars(&text) + 1));
        } else {
            changes.push(Change::insert(start, format!("{text}\n")));
            copies.push((start..end + 1, 0));
        }
    }
    doc.transact(changes, EditKind::Other, |changes, before| {
        let mut after = before.clone();
        after.map(|selection| {
            let Some((block, onto)) = copies
                .iter()
                .find(|(block, _)| block.contains(&selection.start()))
            else {
                return *selection;
            };
            // Both ends keep their place in the block, even one at the next line's start.
            let base = edit::map(changes, block.start, Bias::Before) + onto;
            Selection {
                anchor: base + (selection.anchor - block.start),
                head: base + (selection.head - block.start),
                ..*selection
            }
        });
        after
    })
}

/// Cmd-Shift-K: deletes the covered lines; each cursor ends up at its column on the line
/// that took their place.
pub fn delete_lines(doc: &mut Document) -> bool {
    let rope = doc.rope();
    let tab = doc.indent().width;
    let columns: Vec<usize> = doc
        .selections()
        .iter()
        .map(|selection| text::column(rope, selection.head, tab))
        .collect();
    let len = rope.len_chars();
    let changes = line_blocks(doc)
        .into_iter()
        .map(|block| {
            let start = rope.line_to_char(block.start);
            if block.end < rope.len_lines() {
                Change::delete(start..rope.line_to_char(block.end))
            } else if block.start > 0 {
                Change::delete(text::line_end(rope, block.start - 1)..len)
            } else {
                Change::delete(0..len)
            }
        })
        .collect();
    let deleted = doc.transact(changes, EditKind::Other, |changes, before| {
        let mut after = before.clone();
        after.map(|selection| {
            selection.moved(edit::map(changes, selection.head, Bias::Before), false)
        });
        after
    });
    if deleted {
        let rope = doc.rope().clone();
        let mut selections = doc.selections().clone();
        let mut index = 0;
        selections.map(|selection| {
            let col = columns.get(index).copied().unwrap_or(0);
            index += 1;
            let line = rope.char_to_line(selection.head);
            selection.moved(text::offset_at_column(&rope, line, col, tab), false)
        });
        doc.set_selections(selections);
    }
    deleted
}

/// Cmd-Enter / Cmd-Shift-Enter: a new line below (or above) each cursor's line, indented
/// like it, with the cursor on it.
pub fn insert_line(doc: &mut Document, below: bool) -> bool {
    let unit = doc.indent();
    let openers = doc
        .language()
        .map_or("{[(", |language| language.indent_after);
    edit_each(doc, EditKind::Other, |rope, selection| {
        let line = rope.char_to_line(selection.head);
        let mut indent = text::leading_whitespace(rope, line);
        if below {
            let end = text::line_end(rope, line);
            let opens = rope
                .slice(rope.line_to_char(line)..end)
                .to_string()
                .trim_end()
                .chars()
                .last()
                .is_some_and(|ch| openers.contains(ch));
            if opens {
                indent.push_str(&unit.unit());
            }
            Some(Edit::replace(end..end, format!("\n{indent}")))
        } else {
            let start = rope.line_to_char(line);
            let len = len_chars(&indent);
            Some(Edit {
                range: start..start,
                text: format!("{indent}\n"),
                anchor: len,
                head: len,
            })
        }
    })
}

/// Cmd-A.
pub fn select_all(doc: &mut Document) {
    let len = doc.rope().len_chars();
    doc.set_selections(Selections::single(Selection::new(0, len)));
}

/// Cmd-L: selects the whole line; again, the next line too.
pub fn expand_line_selection(doc: &mut Document) {
    let rope = doc.rope().clone();
    let mut selections = doc.selections().clone();
    selections.map(|selection| {
        let first = rope.char_to_line(selection.start());
        // A selection of whole lines ends at the next line's start, so this reaches one more.
        let next = rope.char_to_line(selection.end()) + 1;
        let end = if next < rope.len_lines() {
            rope.line_to_char(next)
        } else {
            rope.len_chars()
        };
        Selection {
            anchor: rope.line_to_char(first),
            head: end,
            goal: None,
            id: selection.id,
        }
    });
    doc.set_selections(selections);
}

/// Selects the word at every cursor that has nothing selected. Returns whether any did.
fn select_words(doc: &mut Document) -> bool {
    let rope = doc.rope().clone();
    let mut grew = false;
    let mut selections = doc.selections().clone();
    selections.map(|selection| {
        let word = movement::word_at(&rope, selection.head);
        if !selection.is_empty() || word.is_empty() || !is_word_char(rope.char(word.start)) {
            return *selection;
        }
        grew = true;
        Selection {
            anchor: word.start,
            head: word.end,
            goal: None,
            id: selection.id,
        }
    });
    doc.set_selections(selections);
    grew
}

/// What Cmd-D did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NextOccurrence {
    /// Cursors grew to the words they were in; later presses match whole words.
    SelectedWords,
    /// Another occurrence was added as the newest selection.
    Added,
    /// Every occurrence is already selected.
    None,
}

fn occurrences(doc: &Document, needle: &str, whole_word: bool) -> Vec<Range<usize>> {
    let query = Query {
        text: needle.to_owned(),
        case_sensitive: true,
        whole_word,
        regex: false,
    };
    match query.compile() {
        Ok(Some(regex)) => search::find_all(doc.rope(), &regex),
        _ => Vec::new(),
    }
}

/// Cmd-D: selects the word at the cursor, then each next occurrence of the newest selection,
/// wrapping around the end.
pub fn add_next_occurrence(doc: &mut Document, whole_word: bool) -> NextOccurrence {
    if doc.selections().iter().any(Selection::is_empty) {
        return if select_words(doc) {
            NextOccurrence::SelectedWords
        } else {
            NextOccurrence::None
        };
    }
    let newest = *doc.selections().newest();
    let needle = doc.rope().slice(newest.range()).to_string();
    let found = occurrences(doc, &needle, whole_word);
    let taken = |range: &Range<usize>| {
        doc.selections()
            .iter()
            .any(|selection| selection.range() == *range)
    };
    let next = found
        .iter()
        .filter(|range| range.start >= newest.end())
        .chain(found.iter().filter(|range| range.start < newest.end()))
        .find(|range| !taken(range))
        .cloned();
    match next {
        Some(range) => {
            let mut selections = doc.selections().clone();
            selections.add(Selection::new(range.start, range.end));
            doc.set_selections(selections);
            NextOccurrence::Added
        }
        None => NextOccurrence::None,
    }
}

/// Cmd-Shift-L: every occurrence of the primary selection (or of the word at the cursor,
/// as a whole word) becomes a selection.
pub fn select_all_occurrences(doc: &mut Document) {
    let primary = *doc.selections().primary();
    let (range, whole_word) = if primary.is_empty() {
        (movement::word_at(doc.rope(), primary.head), true)
    } else {
        (primary.range(), false)
    };
    if range.is_empty() {
        return;
    }
    let needle = doc.rope().slice(range.clone()).to_string();
    let found = occurrences(doc, &needle, whole_word);
    if found.is_empty() {
        return;
    }
    let first = found
        .iter()
        .position(|candidate| *candidate == range)
        .unwrap_or(0);
    let selections: Vec<Selection> = found
        .iter()
        .enumerate()
        .map(|(index, found)| Selection {
            // The occurrence the cursor was in stays the primary.
            id: if index == first { 0 } else { u64::MAX },
            ..Selection::new(found.start, found.end)
        })
        .collect();
    doc.set_selections(Selections::from_vec(selections, 1));
}

/// Cmd-Alt-Up / Cmd-Alt-Down: a new cursor on the line above the topmost cursor (or below
/// the bottom one), at its goal column.
pub fn add_cursor_vertically(doc: &mut Document, up: bool) -> bool {
    let rope = doc.rope().clone();
    let tab = doc.indent().width;
    let edge = if up {
        doc.selections().iter().next().copied()
    } else {
        doc.selections().iter().last().copied()
    };
    let Some(edge) = edge else {
        return false;
    };
    let line = rope.char_to_line(edge.head);
    let target = if up {
        line.checked_sub(1)
    } else {
        Some(line + 1)
    };
    let Some(target) = target.filter(|&target| target < rope.len_lines()) else {
        return false;
    };
    let goal = edge
        .goal
        .unwrap_or_else(|| text::column(&rope, edge.head, tab));
    let head = text::offset_at_column(&rope, target, goal, tab);
    let mut selections = doc.selections().clone();
    selections.add(Selection {
        goal: Some(goal),
        ..Selection::cursor(head)
    });
    doc.set_selections(selections);
    true
}

/// Escape: only the primary selection remains.
pub fn keep_primary(doc: &mut Document) -> bool {
    if doc.selections().len() == 1 {
        return false;
    }
    let mut selections = doc.selections().clone();
    selections.keep_primary();
    doc.set_selections(selections);
    true
}

/// What Cmd-C copies: the selections' text, one per line, or with nothing selected the
/// cursors' whole lines (`true`), which paste back as lines.
pub fn copy(doc: &Document) -> (String, bool) {
    let rope = doc.rope();
    if doc.selections().iter().all(Selection::is_empty) {
        let mut lines: Vec<usize> = doc
            .selections()
            .iter()
            .map(|selection| rope.char_to_line(selection.head))
            .collect();
        lines.dedup();
        let mut text = String::new();
        for line in lines {
            text.push_str(&block_text(rope, line..line + 1));
            text.push('\n');
        }
        return (text, true);
    }
    let parts: Vec<String> = doc
        .selections()
        .iter()
        .filter(|selection| !selection.is_empty())
        .map(|selection| rope.slice(selection.range()).to_string())
        .collect();
    (parts.join("\n"), false)
}

/// Cmd-X: copies as [`copy`] does, then deletes what was copied.
pub fn cut(doc: &mut Document) -> (String, bool) {
    let copied = copy(doc);
    if copied.1 {
        delete_lines(doc);
    } else {
        edit_each(doc, EditKind::Other, |_, selection| {
            (!selection.is_empty()).then(|| Edit::delete(selection.range()))
        });
    }
    copied
}

/// Cmd-V: with as many cursors as the text has lines, one line each; whole lines copied
/// from bare cursors go above the cursors' lines; otherwise the text replaces every
/// selection.
pub fn paste(doc: &mut Document, text: &str, whole_lines: bool) -> bool {
    let text = text::normalize(text).0;
    let count = doc.selections().len();
    let lines: Vec<&str> = text
        .strip_suffix('\n')
        .unwrap_or(&text)
        .split('\n')
        .collect();
    if count > 1 && lines.len() == count {
        let mut parts = lines.into_iter();
        return edit_each(doc, EditKind::Other, |_, selection| {
            Some(Edit::replace(
                selection.range(),
                parts.next().unwrap_or_default().to_owned(),
            ))
        });
    }
    if whole_lines && doc.selections().iter().all(Selection::is_empty) {
        let rope = doc.rope();
        let changes = doc
            .selections()
            .iter()
            .map(|selection| {
                Change::insert(
                    rope.line_to_char(rope.char_to_line(selection.head)),
                    text.clone(),
                )
            })
            .collect();
        return doc.transact(changes, EditKind::Other, |changes, before| {
            let mut after = before.clone();
            after.map(|selection| {
                let head = edit::map(changes, selection.head, Bias::After);
                selection.moved(head, false)
            });
            after
        });
    }
    edit_each(doc, EditKind::Other, |_, selection| {
        Some(Edit::replace(selection.range(), text.clone()))
    })
}

/// Applies edits from elsewhere (a formatter, a completion's extra edits), keeping the
/// cursors where they were in the text around them. A set with a range backwards, past the
/// end or overlapping another changes nothing: it was made for some other text.
pub fn apply_edits(doc: &mut Document, changes: Vec<Change>) -> bool {
    if !fits(&changes, doc.rope().len_chars()) {
        return false;
    }
    doc.transact(changes, EditKind::Other, |changes, before| {
        let mut after = before.clone();
        after.map(|selection| Selection {
            anchor: edit::map(changes, selection.anchor, Bias::After),
            head: edit::map(changes, selection.head, Bias::After),
            ..*selection
        });
        after
    })
}

/// Whether `changes` can apply at once to a text of `len` chars: each range forwards and in
/// the text, none overlapping another (inserts may share a place, as LSP allows).
fn fits(changes: &[Change], len: usize) -> bool {
    let mut ranges: Vec<&Range<usize>> = changes.iter().map(|change| &change.range).collect();
    ranges.sort_by_key(|range| (range.start, range.end));
    ranges
        .iter()
        .all(|range| range.start <= range.end && range.end <= len)
        && ranges.windows(2).all(|pair| pair[0].end <= pair[1].start)
}

/// Replaces `range` with `text` and puts a single cursor after it: accepting a completion
/// or a replace.
pub fn replace_range(doc: &mut Document, range: Range<usize>, text: &str) -> bool {
    let end = range.start + len_chars(text);
    doc.transact(
        vec![Change {
            range,
            text: text.to_owned(),
        }],
        EditKind::Other,
        |_, before| {
            let mut after = before.clone();
            after.keep_primary();
            after.map(|selection| selection.moved(end, false));
            after
        },
    )
}

/// Selects `range`, alone, as Go to Definition and Find do.
pub fn select_range(doc: &mut Document, range: Range<usize>) {
    let selection = Selection::new(range.start, range.end);
    doc.set_selections(Selections::single(selection));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::language::{self, Language};

    /// A document from text with `|` marking cursors and `[` `]` marking selections.
    fn doc_in(marked: &str, language: Option<&'static Language>) -> Document {
        let mut text = String::new();
        let mut selections = Vec::new();
        let mut anchor = None;
        for ch in marked.chars() {
            let at = text.chars().count();
            match ch {
                '|' => selections.push(Selection::cursor(at)),
                '[' => anchor = Some(at),
                ']' => selections.push(Selection::new(anchor.take().unwrap(), at)),
                _ => text.push(ch),
            }
        }
        let mut doc = Document::new(&text, language, Indent::default());
        let selections = selections
            .into_iter()
            .map(|selection| Selection {
                id: u64::MAX,
                ..selection
            })
            .collect();
        doc.set_selections(Selections::from_vec(selections, 0));
        doc
    }

    fn doc(marked: &str) -> Document {
        doc_in(marked, None)
    }

    fn rust(marked: &str) -> Document {
        doc_in(marked, language::for_path(std::path::Path::new("a.rs")))
    }

    fn js(marked: &str) -> Document {
        doc_in(marked, language::for_path(std::path::Path::new("a.js")))
    }

    /// The document written back with its cursors and selections marked.
    fn shown(doc: &Document) -> String {
        let mut out = String::new();
        let marks: Vec<(usize, char)> = doc
            .selections()
            .iter()
            .flat_map(|selection| {
                if selection.is_empty() {
                    vec![(selection.head, '|')]
                } else {
                    vec![(selection.start(), '['), (selection.end(), ']')]
                }
            })
            .collect();
        for (index, ch) in doc.rope().chars().chain(std::iter::once('\0')).enumerate() {
            for (_, mark) in marks.iter().filter(|(at, _)| *at == index) {
                out.push(*mark);
            }
            if ch != '\0' {
                out.push(ch);
            }
        }
        out
    }

    #[test]
    fn brackets_should_close_themselves_before_blanks_but_not_before_words() {
        let mut before_blank = doc("f|\n");
        type_char(&mut before_blank, '(');
        let mut before_word = doc("|x");
        type_char(&mut before_word, '(');
        assert_eq!(
            (shown(&before_blank), shown(&before_word)),
            ("f(|)\n".to_owned(), "(|x".to_owned())
        );
    }

    #[test]
    fn a_closer_should_be_typed_over() {
        let mut doc = doc("f(|)");
        type_char(&mut doc, ')');
        assert_eq!(shown(&doc), "f()|");
    }

    #[test]
    fn a_bracket_should_wrap_the_selection() {
        let mut doc = doc("[abc]");
        type_char(&mut doc, '[');
        assert_eq!(shown(&doc), "[[abc]]");
    }

    #[test]
    fn quotes_should_pair_only_where_the_language_pairs_them() {
        let mut in_js = js("x = |");
        type_char(&mut in_js, '\'');
        let mut lifetime = rust("&|");
        type_char(&mut lifetime, '\'');
        let mut after_word = js("don|");
        type_char(&mut after_word, '\'');
        assert_eq!(
            (shown(&in_js), shown(&lifetime), shown(&after_word)),
            ("x = '|'".to_owned(), "&'|".to_owned(), "don'|".to_owned())
        );
    }

    #[test]
    fn enter_between_braces_should_open_an_indented_line() {
        let mut doc = rust("fn f() {|}");
        newline(&mut doc);
        assert_eq!(shown(&doc), "fn f() {\n    |\n}");
    }

    #[test]
    fn enter_should_keep_the_indentation_and_trim_a_blank_line() {
        let mut doc = doc("    let x;|");
        newline(&mut doc);
        newline(&mut doc);
        assert_eq!(shown(&doc), "    let x;\n\n    |");
    }

    #[test]
    fn a_closing_brace_on_a_blank_line_should_outdent_it() {
        let mut doc = rust("fn f() {\n    x;\n    |");
        type_char(&mut doc, '}');
        assert_eq!(shown(&doc), "fn f() {\n    x;\n}|");
    }

    #[test]
    fn backspace_should_delete_an_empty_pair_and_indent_stops() {
        let mut pair = doc("f(|)");
        backspace(&mut pair);
        // Indented by 4: from 6 spaces in, back to the stop at 4.
        let mut stops = doc("    a\n        b\n      |x");
        backspace(&mut stops);
        assert_eq!(
            (shown(&pair), shown(&stops)),
            ("f|".to_owned(), "    a\n        b\n    |x".to_owned())
        );
    }

    #[test]
    fn typing_should_happen_at_every_cursor() {
        let mut doc = doc("a|\nb|\n");
        insert(&mut doc, "x");
        assert_eq!(shown(&doc), "ax|\nbx|\n");
    }

    #[test]
    fn tab_should_indent_lines_of_a_multiline_selection_and_shift_tab_undo_it() {
        let mut doc = doc("[a\nb]\n");
        tab(&mut doc);
        let indented = shown(&doc);
        outdent_lines(&mut doc);
        assert_eq!(
            (indented, shown(&doc)),
            ("[    a\n    b]\n".to_owned(), "[a\nb]\n".to_owned())
        );
    }

    #[test]
    fn tab_should_type_up_to_the_next_stop() {
        let mut doc = doc("ab|");
        tab(&mut doc);
        assert_eq!(shown(&doc), "ab  |");
    }

    #[test]
    fn toggle_comment_should_comment_at_the_common_indent_then_uncomment() {
        let mut doc = rust("    [a\n\n      b]\n");
        toggle_line_comment(&mut doc);
        let commented = doc.rope().to_string();
        toggle_line_comment(&mut doc);
        assert_eq!(
            (commented, doc.rope().to_string()),
            (
                "    // a\n\n    //   b\n".to_owned(),
                "    a\n\n      b\n".to_owned()
            )
        );
    }

    #[test]
    fn move_lines_should_swap_with_the_neighbour_and_carry_the_cursor() {
        let mut doc = doc("one\ntw|o\nthree");
        move_lines(&mut doc, true);
        let up = shown(&doc);
        move_lines(&mut doc, false);
        move_lines(&mut doc, false);
        assert_eq!(
            (up, shown(&doc)),
            ("tw|o\none\nthree".to_owned(), "one\nthree\ntw|o".to_owned())
        );
    }

    #[test]
    fn copy_lines_down_should_put_the_cursor_on_the_copy() {
        let mut doc = doc("a|b\nc");
        copy_lines(&mut doc, true);
        assert_eq!(shown(&doc), "ab\na|b\nc");
    }

    #[test]
    fn copy_lines_should_select_just_the_copy_in_the_direction_copied() {
        let copied = |marked: &str, down: bool| {
            let mut doc = doc(marked);
            copy_lines(&mut doc, down);
            shown(&doc)
        };
        assert_eq!(
            (
                copied("[ab\n]cd\nef\n", true),
                copied("ab\n[cd\n]ef\n", false),
                copied("[ab]c\nxyz\n", false),
                copied("[a]\nb\n|c\n", true),
                copied("[a]\nb\n|c\n", false),
            ),
            (
                "ab\n[ab\n]cd\nef\n".to_owned(),
                "ab\n[cd\n]cd\nef\n".to_owned(),
                "[ab]c\nabc\nxyz\n".to_owned(),
                "a\n[a]\nb\nc\n|c\n".to_owned(),
                "[a]\na\nb\n|c\nc\n".to_owned(),
            )
        );
    }

    #[test]
    #[expect(clippy::reversed_empty_ranges, reason = "a server's backwards range")]
    fn edits_from_a_server_should_apply_whole_or_not_at_all() {
        let edit = |range: Range<usize>, text: &str| Change {
            range,
            text: text.into(),
        };
        let applied = |changes: Vec<Change>| {
            let mut doc = doc("|hello world");
            let changed = apply_edits(&mut doc, changes);
            (changed, shown(&doc))
        };
        assert_eq!(
            (
                applied(vec![edit(5..2, "x")]),
                applied(vec![edit(0..5, "a"), edit(3..8, "b")]),
                applied(vec![edit(6..99, "x")]),
                applied(vec![edit(5..5, "!"), edit(5..5, "?"), edit(0..5, "HI")]),
            ),
            (
                (false, "|hello world".to_owned()),
                (false, "|hello world".to_owned()),
                (false, "|hello world".to_owned()),
                (true, "HI|!? world".to_owned()),
            )
        );
    }

    #[test]
    fn delete_lines_should_keep_the_cursor_column() {
        let mut doc = doc("one\ntw|o\nthree");
        delete_lines(&mut doc);
        assert_eq!(shown(&doc), "one\nth|ree");
    }

    #[test]
    fn cmd_d_should_select_the_word_then_add_whole_word_matches() {
        let mut doc = doc("fo|o foobar foo");
        let first = add_next_occurrence(&mut doc, true);
        let second = add_next_occurrence(&mut doc, true);
        let third = add_next_occurrence(&mut doc, true);
        assert_eq!(
            (first, second, third, shown(&doc)),
            (
                NextOccurrence::SelectedWords,
                NextOccurrence::Added,
                NextOccurrence::None,
                "[foo] foobar [foo]".to_owned()
            )
        );
    }

    #[test]
    fn select_all_occurrences_should_keep_the_cursors_one_primary() {
        let mut doc = doc("x y x| x");
        select_all_occurrences(&mut doc);
        assert_eq!(
            (shown(&doc), doc.selections().primary().start()),
            ("[x] y [x] [x]".to_owned(), 4)
        );
    }

    #[test]
    fn add_cursor_below_should_keep_the_column() {
        let mut doc = doc("ab|c\nx\nabc");
        add_cursor_vertically(&mut doc, false);
        add_cursor_vertically(&mut doc, false);
        assert_eq!(shown(&doc), "ab|c\nx|\nab|c");
    }

    #[test]
    fn copy_without_a_selection_should_take_the_line_and_paste_it_above() {
        let mut doc = doc("a|b\ncd");
        let (text, whole) = copy(&doc);
        doc.set_selections(Selections::single(Selection::cursor(4)));
        paste(&mut doc, &text, whole);
        assert_eq!(
            (text.as_str(), shown(&doc)),
            ("ab\n", "ab\nab\nc|d".to_owned())
        );
    }

    #[test]
    fn paste_should_spread_lines_over_as_many_cursors() {
        let mut doc = doc("|\n|\n");
        paste(&mut doc, "a\nb", false);
        assert_eq!(shown(&doc), "a|\nb|\n");
    }

    #[test]
    fn cmd_l_should_select_whole_lines_one_more_each_time() {
        let mut doc = doc("a|b\ncd\nef");
        expand_line_selection(&mut doc);
        let once = shown(&doc);
        expand_line_selection(&mut doc);
        assert_eq!(
            (once, shown(&doc)),
            ("[ab\n]cd\nef".to_owned(), "[ab\ncd\n]ef".to_owned())
        );
    }

    #[test]
    fn insert_line_below_should_indent_after_an_opener() {
        let mut doc = rust("fn f() {|\n}\n");
        insert_line(&mut doc, true);
        assert_eq!(shown(&doc), "fn f() {\n    |\n}\n");
    }

    #[test]
    fn cmd_backspace_should_delete_to_the_line_start_then_join() {
        let mut doc = doc("ab\ncd|");
        delete_to(&mut doc, line_start_or_break);
        delete_to(&mut doc, line_start_or_break);
        assert_eq!(shown(&doc), "ab|");
    }

    #[test]
    fn undo_should_restore_the_selections_of_a_multi_cursor_edit() {
        let mut doc = doc("a|\nb|\n");
        insert(&mut doc, "x");
        doc.undo();
        assert_eq!(shown(&doc), "a|\nb|\n");
    }
}
