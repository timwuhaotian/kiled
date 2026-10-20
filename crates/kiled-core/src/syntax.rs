//! A document's syntax tree, kept current edit by edit, and the highlights of the part on
//! screen (DESIGN.md §5.2).
//!
//! Each change edits the tree before the text changes, then one incremental reparse follows
//! the whole transaction. Highlighting runs the language's query over the visible bytes only,
//! so its cost follows the window, not the file.

use std::{ops::Range, sync::Arc};

use ropey::{Rope, iter::Chunks};
use streaming_iterator::StreamingIterator;
use tree_sitter::{InputEdit, Node, Parser, Point, QueryCursor, TextProvider, Tree};

use crate::{
    edit::Change,
    language::{Highlight, Highlighter, Language},
};

/// A parsed document.
pub struct Syntax {
    parser: Parser,
    tree: Tree,
    cursor: QueryCursor,
    highlighter: Option<Arc<Highlighter>>,
}

impl std::fmt::Debug for Syntax {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Syntax").finish_non_exhaustive()
    }
}

/// Feeds the rope to tree-sitter chunk by chunk, without copying it.
fn read_chunk(rope: &Rope, byte: usize) -> &[u8] {
    if byte >= rope.len_bytes() {
        return &[];
    }
    let (chunk, start, _, _) = rope.chunk_at_byte(byte);
    &chunk.as_bytes()[byte - start..]
}

/// Gives queries a node's text (for `#eq?` and `#match?`) straight from the rope.
struct RopeText<'a>(&'a Rope);

type ChunkBytes<'a> = std::iter::Map<Chunks<'a>, fn(&'a str) -> &'a [u8]>;

impl<'a> TextProvider<&'a [u8]> for RopeText<'a> {
    type I = ChunkBytes<'a>;

    fn text(&mut self, node: Node) -> Self::I {
        let len = self.0.len_bytes();
        let range = node.start_byte().min(len)..node.end_byte().min(len);
        self.0.byte_slice(range).chunks().map(str::as_bytes)
    }
}

/// Where char `offset` is, as tree-sitter counts: bytes, and (row, byte column).
fn point(rope: &Rope, offset: usize) -> (usize, Point) {
    let byte = rope.char_to_byte(offset);
    let row = rope.byte_to_line(byte);
    (
        byte,
        Point {
            row,
            column: byte - rope.line_to_byte(row),
        },
    )
}

impl Syntax {
    /// Parses `rope` as `language`. `None` if the grammar can't be loaded.
    pub fn new(language: &'static Language, rope: &Rope) -> Option<Self> {
        let mut parser = Parser::new();
        if let Err(err) = parser.set_language(&language.grammar()) {
            log::error!("{}: {err}", language.name);
            return None;
        }
        let tree = parser.parse_with_options(&mut |byte, _| read_chunk(rope, byte), None, None)?;
        Some(Self {
            parser,
            tree,
            cursor: QueryCursor::new(),
            highlighter: language.highlighter(),
        })
    }

    /// Tells the tree about `change`, before it is made to `rope`.
    pub fn edit(&mut self, rope: &Rope, change: &Change) {
        let (start_byte, start_position) = point(rope, change.range.start);
        let (old_end_byte, old_end_position) = point(rope, change.range.end);
        let new_end_position = match change.text.rfind('\n') {
            Some(last) => Point {
                row: start_position.row + change.text.matches('\n').count(),
                column: change.text.len() - last - 1,
            },
            None => Point {
                row: start_position.row,
                column: start_position.column + change.text.len(),
            },
        };
        self.tree.edit(&InputEdit {
            start_byte,
            old_end_byte,
            new_end_byte: start_byte + change.text.len(),
            start_position,
            old_end_position,
            new_end_position,
        });
    }

    /// Reparses after edits, reusing what didn't change.
    pub fn reparse(&mut self, rope: &Rope) {
        let parsed = self.parser.parse_with_options(
            &mut |byte, _| read_chunk(rope, byte),
            Some(&self.tree),
            None,
        );
        match parsed {
            Some(tree) => self.tree = tree,
            None => log::error!("reparse failed; keeping the old tree"),
        }
    }

    /// The tree, for structural queries.
    #[must_use]
    pub fn tree(&self) -> &Tree {
        &self.tree
    }

    /// Highlighted spans of `bytes`, in order, not overlapping, each clipped to `bytes`. Inner
    /// nodes win over the nodes around them. Of two patterns capturing the same node, the more
    /// specific capture name wins, else the later pattern, as tree-sitter's own highlighter
    /// does: queries are written both ways (JSON's key pattern comes before its string one,
    /// JavaScript's variable pattern before its function ones).
    pub fn highlights(
        &mut self,
        rope: &Rope,
        bytes: Range<usize>,
    ) -> Vec<(Range<usize>, Highlight)> {
        let Some(highlighter) = self.highlighter.clone() else {
            return Vec::new();
        };
        let bytes = bytes.start.min(rope.len_bytes())..bytes.end.min(rope.len_bytes());
        if bytes.is_empty() {
            return Vec::new();
        }
        self.cursor.set_byte_range(bytes.clone());
        let mut captures: Vec<(usize, usize, usize, Highlight)> = Vec::new();
        let mut matches =
            self.cursor
                .captures(&highlighter.query, self.tree.root_node(), RopeText(rope));
        while let Some((found, index)) = matches.next() {
            let capture = found.captures[*index];
            let Some(Some(kind)) = highlighter.kinds.get(capture.index as usize) else {
                continue;
            };
            let start = capture.node.start_byte().max(bytes.start);
            let end = capture.node.end_byte().min(bytes.end);
            let specificity = highlighter
                .specificity
                .get(capture.index as usize)
                .copied()
                .unwrap_or(1);
            if start < end {
                captures.push((start, end, specificity, *kind));
            }
        }
        // Outer nodes first, so the nodes inside them paint over them; for one node, the more
        // specific capture paints last, then the later pattern (the sort is stable).
        captures.sort_by_key(|&(start, end, specificity, _)| {
            (start, std::cmp::Reverse(end), specificity)
        });
        let mut painted: Vec<Option<Highlight>> = vec![None; bytes.len()];
        for (start, end, _, kind) in captures {
            painted[start - bytes.start..end - bytes.start].fill(Some(kind));
        }
        let mut spans: Vec<(Range<usize>, Highlight)> = Vec::new();
        for (index, kind) in painted.into_iter().enumerate() {
            let at = bytes.start + index;
            match (kind, spans.last_mut()) {
                (Some(kind), Some((range, last))) if *last == kind && range.end == at => {
                    range.end = at + 1;
                }
                (Some(kind), _) => spans.push((at..at + 1, kind)),
                (None, _) => {}
            }
        }
        spans
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::language::{self, LANGUAGES};

    fn rust() -> &'static Language {
        &LANGUAGES[0]
    }

    fn kinds_of(syntax: &mut Syntax, rope: &Rope, word: &str) -> Vec<Highlight> {
        let text = rope.to_string();
        let start = text.find(word).unwrap();
        syntax
            .highlights(rope, start..start + word.len())
            .into_iter()
            .map(|(_, kind)| kind)
            .collect()
    }

    #[test]
    fn rust_keywords_strings_and_functions_should_highlight() {
        let rope = Rope::from("fn main() { let s = \"hi\"; }\n");
        let mut syntax = Syntax::new(rust(), &rope).unwrap();
        assert_eq!(
            (
                kinds_of(&mut syntax, &rope, "fn"),
                kinds_of(&mut syntax, &rope, "main"),
                kinds_of(&mut syntax, &rope, "\"hi\""),
            ),
            (
                vec![Highlight::Keyword],
                vec![Highlight::Function],
                vec![Highlight::String]
            )
        );
    }

    #[test]
    fn an_edit_and_reparse_should_follow_the_new_text() {
        let mut rope = Rope::from("let x = 1;\n");
        let mut syntax = Syntax::new(rust(), &rope).unwrap();
        let change = Change {
            range: 8..9,
            text: "\"one\"".into(),
        };
        syntax.edit(&rope, &change);
        rope.remove(8..9);
        rope.insert(8, "\"one\"");
        syntax.reparse(&rope);
        assert_eq!(
            kinds_of(&mut syntax, &rope, "\"one\""),
            vec![Highlight::String]
        );
    }

    #[test]
    fn later_patterns_should_win_for_the_same_node() {
        // JavaScript captures every identifier as a variable first, and a called one as a
        // function later.
        let js = language::for_path(std::path::Path::new("a.js")).unwrap();
        let rope = Rope::from("foo(bar);\n");
        let mut syntax = Syntax::new(js, &rope).unwrap();
        assert_eq!(
            (
                kinds_of(&mut syntax, &rope, "foo"),
                kinds_of(&mut syntax, &rope, "bar")
            ),
            (vec![Highlight::Function], vec![Highlight::Variable])
        );
    }

    #[test]
    fn the_more_specific_capture_should_win_for_the_same_node() {
        // JSON captures a pair's key as `string.special.key` before every string as `string`.
        let json = language::for_path(std::path::Path::new("a.json")).unwrap();
        let rope = Rope::from("{\"key\": \"value\"}\n");
        let mut syntax = Syntax::new(json, &rope).unwrap();
        assert_eq!(
            (
                kinds_of(&mut syntax, &rope, "\"key\""),
                kinds_of(&mut syntax, &rope, "\"value\"")
            ),
            (vec![Highlight::Property], vec![Highlight::String])
        );
    }

    #[test]
    fn highlights_should_clip_to_the_requested_bytes() {
        let rope = Rope::from("// a long comment\n");
        let mut syntax = Syntax::new(rust(), &rope).unwrap();
        assert_eq!(
            syntax.highlights(&rope, 3..6),
            vec![(3..6, Highlight::Comment)]
        );
    }

    #[test]
    fn multibyte_text_should_keep_byte_offsets_right() {
        let mut rope = Rope::from("let 名 = \"中文\";\n");
        let mut syntax = Syntax::new(rust(), &rope).unwrap();
        let at = rope.to_string().chars().count() - 1;
        let change = Change::insert(at, " // 注释");
        syntax.edit(&rope, &change);
        rope.insert(at, " // 注释");
        syntax.reparse(&rope);
        assert_eq!(
            kinds_of(&mut syntax, &rope, "// 注释"),
            vec![Highlight::Comment]
        );
    }
}
