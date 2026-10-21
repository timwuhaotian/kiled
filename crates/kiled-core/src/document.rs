//! A document: its text and selections, undo and redo, its syntax tree, the language server's
//! diagnostics, and the log of changes the server hasn't been sent yet (DESIGN.md §4).
//!
//! Every edit goes through [`Document::transact`], so undo, syntax, diagnostics and the
//! server can't disagree about what the text is.

use std::{
    ops::Range,
    time::{Duration, Instant},
};

use ropey::Rope;

use crate::{
    edit::{self, Bias, Change, len_chars},
    language::{Highlight, Language},
    selection::{Selection, Selections},
    syntax::Syntax,
    text::{self, Indent, LineEnding},
};

/// Above this size a file opens as plain text, without syntax or a language server: a parse
/// that long would stall typing.
pub const SYNTAX_LIMIT: usize = 8 * 1024 * 1024;

/// Typing (or deleting) this soon after the last, without moving the cursor, undoes as one.
const COALESCE_WITHIN: Duration = Duration::from_millis(1500);

/// What an edit was, for grouping undo steps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditKind {
    /// Typed text.
    Typing,
    /// Backspace and Delete.
    Deleting,
    /// Anything else: always its own undo step.
    Other,
}

/// A place as a language server counts: line, and UTF-16 code units into it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Position {
    /// Zero-based line.
    pub line: u32,
    /// UTF-16 code units from the line's start.
    pub character: u32,
}

/// A change as a language server applies it: in order, each against the text as the
/// previous one left it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LspChange {
    /// Start of the replaced text.
    pub start: Position,
    /// End of the replaced text.
    pub end: Position,
    /// What replaces it.
    pub text: String,
}

/// How bad a diagnostic is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// Error.
    Error,
    /// Warning.
    Warning,
    /// Information.
    Information,
    /// Hint.
    Hint,
}

/// A language server's report about part of the text, kept in place as the text changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    /// Chars it is about.
    pub range: Range<usize>,
    /// How bad it is.
    pub severity: Severity,
    /// What it says.
    pub message: String,
    /// Who says it (`rustc`, `ts`).
    pub source: Option<String>,
}

/// `offset` as a language server position.
#[must_use]
pub fn position_of(rope: &Rope, offset: usize) -> Position {
    let offset = offset.min(rope.len_chars());
    let line = rope.char_to_line(offset);
    let start = rope.line_to_char(line);
    let character = rope.char_to_utf16_cu(offset) - rope.char_to_utf16_cu(start);
    Position {
        line: u32::try_from(line).unwrap_or(u32::MAX),
        character: u32::try_from(character).unwrap_or(u32::MAX),
    }
}

/// The char offset of a language server position, clamped into the text: a line past the
/// end is the end, a character past its line is the line's end.
#[must_use]
pub fn offset_of(rope: &Rope, position: Position) -> usize {
    let line = position.line as usize;
    if line >= rope.len_lines() {
        return rope.len_chars();
    }
    let start = rope.line_to_char(line);
    let end = text::line_end(rope, line);
    let units = (rope.char_to_utf16_cu(start) + position.character as usize)
        .min(rope.char_to_utf16_cu(end));
    rope.utf16_cu_to_char(units).clamp(start, end)
}

/// `text` without its BOM, and whether it had one.
fn strip_bom(text: &str) -> (&str, bool) {
    match text.strip_prefix('\u{feff}') {
        Some(rest) => (rest, true),
        None => (text, false),
    }
}

/// Changes that undo or redo together.
#[derive(Debug)]
struct Step {
    forward: Vec<Change>,
    inverse: Vec<Change>,
}

/// One undo step: the changes, and the selections on either side of them.
#[derive(Debug)]
struct Entry {
    steps: Vec<Step>,
    before: Selections,
    after: Selections,
    kind: EditKind,
    at: Instant,
    revision_before: u64,
    revision_after: u64,
}

/// A text being edited.
#[derive(Debug)]
pub struct Document {
    rope: Rope,
    selections: Selections,
    line_ending: LineEnding,
    bom: bool,
    indent: Indent,
    language: Option<&'static Language>,
    syntax: Option<Syntax>,
    undo: Vec<Entry>,
    redo: Vec<Entry>,
    /// Which state of the text this is: each edit makes a new one, undo and redo go back to
    /// theirs. The document is dirty while it differs from the saved one.
    revision: u64,
    saved: u64,
    next_revision: u64,
    lsp_log: Vec<LspChange>,
    diagnostics: Vec<Diagnostic>,
    /// IME preedit: in the text, but in neither the undo history nor the server's log until
    /// it is committed.
    marked: Option<Range<usize>>,
}

impl Document {
    /// A document holding `text` as read from a file: its line ending and BOM are noted and
    /// taken out, and its indentation detected (else `fallback`).
    #[must_use]
    pub fn new(text: &str, language: Option<&'static Language>, fallback: Indent) -> Self {
        let (text, bom) = strip_bom(text);
        let (text, line_ending) = text::normalize(text);
        let rope = Rope::from(text);
        let indent = text::detect_indent(&rope.slice(..), fallback);
        let mut this = Self {
            rope,
            selections: Selections::default(),
            line_ending,
            bom,
            indent,
            language: None,
            syntax: None,
            undo: Vec::new(),
            redo: Vec::new(),
            revision: 0,
            saved: 0,
            next_revision: 1,
            lsp_log: Vec::new(),
            diagnostics: Vec::new(),
            marked: None,
        };
        this.set_language(language);
        this
    }

    /// The text.
    #[must_use]
    pub fn rope(&self) -> &Rope {
        &self.rope
    }

    /// The text as a file holds it: the original line ending and BOM back in place.
    #[must_use]
    pub fn text_for_save(&self) -> String {
        let mut out = String::with_capacity(self.rope.len_bytes() + 3);
        if self.bom {
            out.push('\u{feff}');
        }
        for chunk in self.rope.chunks() {
            match self.line_ending {
                LineEnding::Lf => out.push_str(chunk),
                LineEnding::Crlf => out.push_str(&chunk.replace('\n', "\r\n")),
            }
        }
        out
    }

    /// Every selection.
    #[must_use]
    pub fn selections(&self) -> &Selections {
        &self.selections
    }

    /// Moves the selections without editing; clamped into the text.
    pub fn set_selections(&mut self, mut selections: Selections) {
        let len = self.rope.len_chars();
        selections.map(|selection| Selection {
            anchor: selection.anchor.min(len),
            head: selection.head.min(len),
            ..*selection
        });
        self.selections = selections;
    }

    /// Saved with.
    #[must_use]
    pub fn line_ending(&self) -> LineEnding {
        self.line_ending
    }

    /// How it indents.
    #[must_use]
    pub fn indent(&self) -> Indent {
        self.indent
    }

    /// Its language, if Kiled knows it.
    #[must_use]
    pub fn language(&self) -> Option<&'static Language> {
        self.language
    }

    /// Changes the language (e.g. after Save As), parsing the text again.
    pub fn set_language(&mut self, language: Option<&'static Language>) {
        self.language = language;
        self.syntax = language
            .filter(|_| self.rope.len_bytes() <= SYNTAX_LIMIT)
            .and_then(|language| Syntax::new(language, &self.rope));
    }

    /// Whether the text is small enough for syntax and a language server.
    #[must_use]
    pub fn is_large(&self) -> bool {
        self.rope.len_bytes() > SYNTAX_LIMIT
    }

    /// The syntax tree, if the document has one.
    #[must_use]
    pub fn syntax(&self) -> Option<&Syntax> {
        self.syntax.as_ref()
    }

    /// Highlighted spans (bytes) for `lines`.
    pub fn highlights(&mut self, lines: Range<usize>) -> Vec<(Range<usize>, Highlight)> {
        let last = self.rope.len_lines();
        let start = self.rope.line_to_byte(lines.start.min(last));
        let end = self.rope.line_to_byte(lines.end.min(last));
        match &mut self.syntax {
            Some(syntax) => syntax.highlights(&self.rope, start..end),
            None => Vec::new(),
        }
    }

    /// Which state of the text this is; changes with every edit, undo and redo.
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Differs from what was last saved (or loaded).
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.revision != self.saved
    }

    /// The text as it is now was saved.
    pub fn mark_saved(&mut self) {
        self.saved = self.revision;
    }

    /// Edits the text: `changes` apply at once, and `place` says where the selections go,
    /// given the changes (sorted and merged) and the selections before them. Returns
    /// whether anything changed.
    pub fn transact(
        &mut self,
        changes: Vec<Change>,
        kind: EditKind,
        place: impl FnOnce(&[Change], &Selections) -> Selections,
    ) -> bool {
        self.commit_marked();
        self.transact_now(changes, kind, place)
    }

    fn transact_now(
        &mut self,
        changes: Vec<Change>,
        kind: EditKind,
        place: impl FnOnce(&[Change], &Selections) -> Selections,
    ) -> bool {
        let changes = edit::coalesce(changes);
        if changes.is_empty() {
            return false;
        }
        let before = self.selections.clone();
        let after = place(&changes, &before);
        let inverse = self.apply(&changes, true);
        self.set_selections(after);
        let after = self.selections.clone();
        self.record(
            Step {
                forward: changes,
                inverse,
            },
            before,
            after,
            kind,
        );
        true
    }

    /// Makes `changes` to the text, the tree and the diagnostics; logs them for the server
    /// when `log`. Returns the changes that undo them.
    fn apply(&mut self, changes: &[Change], log: bool) -> Vec<Change> {
        let starts = edit::new_starts(changes);
        let inverse = changes
            .iter()
            .zip(&starts)
            .map(|(change, &start)| Change {
                range: start..start + len_chars(&change.text),
                text: self.rope.slice(change.range.clone()).to_string(),
            })
            .collect();
        // Last first: each change's offsets are still right when it is made, and the log
        // then reads in order for the server.
        for change in changes.iter().rev() {
            if log {
                self.lsp_log.push(LspChange {
                    start: position_of(&self.rope, change.range.start),
                    end: position_of(&self.rope, change.range.end),
                    text: change.text.clone(),
                });
            }
            if let Some(syntax) = &mut self.syntax {
                syntax.edit(&self.rope, change);
            }
            self.rope.remove(change.range.clone());
            self.rope.insert(change.range.start, &change.text);
        }
        if let Some(syntax) = &mut self.syntax {
            syntax.reparse(&self.rope);
        }
        for diagnostic in &mut self.diagnostics {
            let start = edit::map(changes, diagnostic.range.start, Bias::After);
            let end = edit::map(changes, diagnostic.range.end, Bias::Before).max(start);
            diagnostic.range = start..end;
        }
        inverse
    }

    fn record(&mut self, step: Step, before: Selections, after: Selections, kind: EditKind) {
        let now = Instant::now();
        self.redo.clear();
        let revision = self.next_revision;
        self.next_revision += 1;
        let at_saved = self.revision == self.saved;
        match self.undo.last_mut() {
            Some(last)
                if kind != EditKind::Other
                    && last.kind == kind
                    && last.after == before
                    && last.revision_after == self.revision
                    && !at_saved
                    && now.duration_since(last.at) < COALESCE_WITHIN =>
            {
                last.steps.push(step);
                last.after = after;
                last.at = now;
                last.revision_after = revision;
            }
            _ => self.undo.push(Entry {
                steps: vec![step],
                before,
                after,
                kind,
                at: now,
                revision_before: self.revision,
                revision_after: revision,
            }),
        }
        self.revision = revision;
    }

    /// Undoes the last step. Returns whether there was one.
    pub fn undo(&mut self) -> bool {
        self.commit_marked();
        let Some(entry) = self.undo.pop() else {
            return false;
        };
        for step in entry.steps.iter().rev() {
            self.apply(&step.inverse, true);
        }
        self.set_selections(entry.before.clone());
        self.revision = entry.revision_before;
        self.redo.push(entry);
        true
    }

    /// Redoes the last undone step. Returns whether there was one.
    pub fn redo(&mut self) -> bool {
        self.commit_marked();
        let Some(mut entry) = self.redo.pop() else {
            return false;
        };
        for step in &entry.steps {
            self.apply(&step.forward, true);
        }
        self.set_selections(entry.after.clone());
        self.revision = entry.revision_after;
        // Typing after a redo starts a step of its own.
        entry.kind = EditKind::Other;
        self.undo.push(entry);
        true
    }

    /// Replaces the whole text with `text` from disk, as one undoable step, and marks it
    /// saved. Selections stay where they were, as far as the new text reaches.
    pub fn reload(&mut self, text: &str) {
        self.commit_marked();
        let (text, bom) = strip_bom(text);
        let (text, line_ending) = text::normalize(text);
        if self.rope != text.as_str() {
            let all = 0..self.rope.len_chars();
            let len = len_chars(&text);
            self.transact_now(
                vec![Change { range: all, text }],
                EditKind::Other,
                |_, before| {
                    let mut after = before.clone();
                    after.map(|selection| Selection {
                        anchor: selection.anchor.min(len),
                        head: selection.head.min(len),
                        ..*selection
                    });
                    after
                },
            );
        }
        self.line_ending = line_ending;
        self.bom = bom;
        self.mark_saved();
    }

    /// Takes the changes the language server hasn't been sent.
    pub fn take_lsp_changes(&mut self) -> Vec<LspChange> {
        std::mem::take(&mut self.lsp_log)
    }

    /// The language server's latest diagnostics, moved along with the edits since.
    #[must_use]
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    /// Replaces the diagnostics.
    pub fn set_diagnostics(&mut self, mut diagnostics: Vec<Diagnostic>) {
        diagnostics.sort_by_key(|diagnostic| (diagnostic.range.start, diagnostic.severity));
        self.diagnostics = diagnostics;
    }

    /// The IME preedit, if one is being composed.
    #[must_use]
    pub fn marked(&self) -> Option<Range<usize>> {
        self.marked.clone()
    }

    /// Composes: `text` replaces `range` (else the preedit, else the primary selection) and
    /// is marked as preedit, with `selected` (chars within `text`) selected. Nothing of this
    /// reaches undo or the server until [`Self::commit_marked`].
    pub fn set_marked(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
    ) {
        let range = range
            .or_else(|| self.marked.clone())
            .unwrap_or_else(|| self.selections.primary().range());
        let range = range.start.min(self.rope.len_chars())..range.end.min(self.rope.len_chars());
        let primary = *self.selections.primary();
        self.apply(
            &[Change {
                range: range.clone(),
                text: text.to_owned(),
            }],
            false,
        );
        let len = len_chars(text);
        self.marked = (len > 0).then(|| range.start..range.start + len);
        let selected = selected.unwrap_or(len..len);
        let cursor = Selection {
            anchor: range.start + selected.start.min(len),
            head: range.start + selected.end.min(len),
            ..primary
        };
        self.set_selections(Selections::single(cursor));
    }

    /// Types the preedit for real: out of the text without a trace, then in as typing.
    pub fn commit_marked(&mut self) {
        let Some(marked) = self.marked.take() else {
            return;
        };
        let text = self.rope.slice(marked.clone()).to_string();
        self.apply(&[Change::delete(marked.clone())], false);
        let primary = *self.selections.primary();
        self.selections = Selections::single(Selection {
            anchor: marked.start,
            head: marked.start,
            ..primary
        });
        let end = marked.start + len_chars(&text);
        self.transact_now(
            vec![Change::insert(marked.start, text)],
            EditKind::Typing,
            |_, before| {
                let mut after = before.clone();
                after.map(|selection| selection.moved(end, false));
                after
            },
        );
    }

    /// Drops the preedit, as an IME cancelling its composition.
    pub fn cancel_marked(&mut self) {
        if let Some(marked) = self.marked.take() {
            self.apply(&[Change::delete(marked.clone())], false);
            let primary = *self.selections.primary();
            self.set_selections(Selections::single(primary.moved(marked.start, false)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(text: &str) -> Document {
        Document::new(text, None, Indent::default())
    }

    fn type_at_cursors(doc: &mut Document, text: &str) {
        let changes = doc
            .selections()
            .iter()
            .map(|selection| Change {
                range: selection.range(),
                text: text.into(),
            })
            .collect();
        doc.transact(changes, EditKind::Typing, |changes, before| {
            let mut after = before.clone();
            after.map(|selection| {
                let head = edit::map(changes, selection.end(), Bias::After);
                selection.moved(head, false)
            });
            after
        });
    }

    fn text(doc: &Document) -> String {
        doc.rope().to_string()
    }

    #[test]
    fn crlf_and_bom_should_survive_a_round_trip() {
        let doc = doc("\u{feff}a\r\nb\r\n");
        assert_eq!(
            (text(&doc), doc.text_for_save()),
            ("a\nb\n".to_owned(), "\u{feff}a\r\nb\r\n".to_owned())
        );
    }

    #[test]
    fn typing_should_insert_at_every_cursor() {
        let mut doc = doc("ab\ncd\n");
        doc.set_selections(Selections::from_vec(
            vec![Selection::cursor(1), Selection::cursor(4)],
            1,
        ));
        type_at_cursors(&mut doc, "X");
        assert_eq!(
            (
                text(&doc),
                doc.selections().iter().map(|s| s.head).collect::<Vec<_>>()
            ),
            ("aXb\ncXd\n".to_owned(), vec![2, 6])
        );
    }

    #[test]
    fn typing_should_undo_as_one_step_and_redo() {
        let mut doc = doc("");
        for ch in ["a", "b", "c"] {
            type_at_cursors(&mut doc, ch);
        }
        doc.undo();
        let undone = text(&doc);
        doc.redo();
        assert_eq!((undone, text(&doc)), (String::new(), "abc".to_owned()));
    }

    #[test]
    fn moving_the_cursor_should_end_an_undo_step() {
        let mut doc = doc("");
        type_at_cursors(&mut doc, "a");
        doc.set_selections(Selections::single(Selection::cursor(0)));
        type_at_cursors(&mut doc, "b");
        doc.undo();
        assert_eq!(text(&doc), "a");
    }

    #[test]
    fn undoing_back_to_the_saved_text_should_make_it_clean() {
        let mut doc = doc("x");
        type_at_cursors(&mut doc, "a");
        let dirty = doc.is_dirty();
        doc.undo();
        assert_eq!((dirty, doc.is_dirty()), (true, false));
    }

    #[test]
    fn typing_after_a_save_should_not_join_the_step_before_it() {
        let mut doc = doc("");
        type_at_cursors(&mut doc, "a");
        doc.mark_saved();
        type_at_cursors(&mut doc, "b");
        doc.undo();
        assert_eq!((text(&doc), doc.is_dirty()), ("a".to_owned(), false));
    }

    #[test]
    fn the_lsp_log_should_apply_in_order_to_the_old_text() {
        let mut doc = doc("ab\ncd\n");
        doc.set_selections(Selections::from_vec(
            vec![Selection::cursor(1), Selection::cursor(4)],
            1,
        ));
        type_at_cursors(&mut doc, "中");
        let log = doc.take_lsp_changes();
        let mut replay = Rope::from("ab\ncd\n");
        for change in &log {
            let start = offset_of(&replay, change.start);
            let end = offset_of(&replay, change.end);
            replay.remove(start..end);
            replay.insert(start, &change.text);
        }
        assert_eq!(replay.to_string(), text(&doc));
    }

    #[test]
    fn utf16_positions_should_count_surrogate_pairs() {
        let rope = Rope::from("a😀b\nc");
        assert_eq!(
            (
                position_of(&rope, 2),
                offset_of(
                    &rope,
                    Position {
                        line: 0,
                        character: 3
                    }
                ),
                offset_of(
                    &rope,
                    Position {
                        line: 0,
                        character: 99
                    }
                ),
                offset_of(
                    &rope,
                    Position {
                        line: 9,
                        character: 0
                    }
                )
            ),
            (
                Position {
                    line: 0,
                    character: 3
                },
                2,
                3,
                5
            )
        );
    }

    #[test]
    fn diagnostics_should_move_with_the_text() {
        let mut doc = doc("let x = y;\n");
        doc.set_diagnostics(vec![Diagnostic {
            range: 8..9,
            severity: Severity::Error,
            message: "unknown y".into(),
            source: None,
        }]);
        doc.set_selections(Selections::single(Selection::cursor(0)));
        type_at_cursors(&mut doc, "// ");
        assert_eq!(doc.diagnostics()[0].range, 11..12);
    }

    #[test]
    fn a_preedit_should_reach_undo_and_the_server_only_when_committed() {
        let mut doc = doc("x");
        doc.set_selections(Selections::single(Selection::cursor(1)));
        doc.set_marked(None, "zh", None);
        doc.set_marked(None, "zhong", None);
        let composing = (text(&doc), doc.take_lsp_changes().len());
        doc.set_marked(None, "中", None);
        doc.commit_marked();
        let log = doc.take_lsp_changes();
        doc.undo();
        assert_eq!(
            (composing, log.len(), text(&doc)),
            (("xzhong".to_owned(), 0), 1, "x".to_owned())
        );
    }

    #[test]
    fn reload_should_replace_the_text_undoably_and_mark_it_saved() {
        let mut doc = doc("old\n");
        doc.reload("new\r\n");
        let reloaded = (text(&doc), doc.is_dirty(), doc.line_ending());
        doc.undo();
        assert_eq!(
            (reloaded, text(&doc)),
            (
                ("new\n".to_owned(), false, LineEnding::Crlf),
                "old\n".to_owned()
            )
        );
    }

    #[test]
    fn reload_should_save_with_the_bom_the_file_has_now() {
        let mut gained = doc("a\n");
        gained.reload("\u{feff}b\n");
        let mut lost = doc("\u{feff}a\n");
        lost.reload("b\n");
        assert_eq!(
            (gained.text_for_save(), lost.text_for_save()),
            ("\u{feff}b\n".to_owned(), "b\n".to_owned())
        );
    }
}
