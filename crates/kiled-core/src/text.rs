//! Text as the editor holds it: a rope with `\n` lines, plus what was normalized away on load
//! (the file's line ending) and how it indents (DESIGN.md §4.1).
//!
//! Offsets are chars throughout the core. Bytes appear only at the tree-sitter edge, UTF-16 at
//! the LSP and IME edges, and display columns (tabs expanded, wide chars counted twice) where
//! the grid is drawn.

use ropey::{Rope, RopeSlice};
use unicode_width::UnicodeWidthChar;

/// The line ending a file is saved with. Text in memory always uses `\n`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LineEnding {
    /// `\n`
    #[default]
    Lf,
    /// `\r\n`
    Crlf,
}

impl LineEnding {
    /// As the status bar shows it.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Lf => "LF",
            Self::Crlf => "CRLF",
        }
    }

    /// The bytes written for a line break.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lf => "\n",
            Self::Crlf => "\r\n",
        }
    }
}

/// How a document indents: with tabs, or with `width` spaces per level. `width` is also how
/// wide a tab is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Indent {
    /// Tabs instead of spaces.
    pub tabs: bool,
    /// Columns per level.
    pub width: usize,
}

impl Default for Indent {
    fn default() -> Self {
        Self {
            tabs: false,
            width: 4,
        }
    }
}

impl Indent {
    /// One level, as inserted.
    #[must_use]
    pub fn unit(self) -> String {
        if self.tabs {
            "\t".to_owned()
        } else {
            " ".repeat(self.width)
        }
    }

    /// As the status bar shows it.
    #[must_use]
    pub fn label(self) -> String {
        if self.tabs {
            format!("Tab Size: {}", self.width)
        } else {
            format!("Spaces: {}", self.width)
        }
    }
}

/// `text` with every `\r\n` and lone `\r` as `\n`, and the line ending to save it with: CRLF
/// when most of its breaks were, else LF.
#[must_use]
pub fn normalize(text: &str) -> (String, LineEnding) {
    if !text.contains('\r') {
        return (text.to_owned(), LineEnding::Lf);
    }
    let crlf = text.matches("\r\n").count();
    let lf = text.matches('\n').count() - crlf;
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let ending = if crlf > lf {
        LineEnding::Crlf
    } else {
        LineEnding::Lf
    };
    (normalized, ending)
}

/// The indentation most of `text`'s indented lines use, else `fallback`. Spaces count by the
/// step between consecutive indented lines, so a file indented by 2 reads as 2 even where it
/// nests four deep.
#[must_use]
pub fn detect_indent(text: &RopeSlice, fallback: Indent) -> Indent {
    let mut tabs = 0usize;
    let mut spaces = 0usize;
    let mut steps = [0usize; 9];
    let mut previous = 0usize;
    for line in text.lines().take(10_000) {
        let mut chars = line.chars();
        match chars.next() {
            Some('\t') => {
                tabs += 1;
                continue;
            }
            Some(' ') => {}
            _ => {
                if line.chars().any(|ch| !ch.is_whitespace()) {
                    previous = 0;
                }
                continue;
            }
        }
        let width = 1 + chars.take_while(|ch| *ch == ' ').count();
        if line.chars().nth(width).is_none_or(|ch| ch == '\n') {
            continue;
        }
        spaces += 1;
        let step = width.abs_diff(previous);
        if (2..=8).contains(&step) {
            steps[step] += 1;
        }
        previous = width;
    }
    if tabs == 0 && spaces == 0 {
        return fallback;
    }
    if tabs > spaces {
        return Indent {
            tabs: true,
            width: fallback.width,
        };
    }
    let width = (2..=8)
        .max_by_key(|&step| (steps[step], usize::from(step == 4), usize::from(step == 2)))
        .filter(|&step| steps[step] > 0)
        .unwrap_or(fallback.width);
    Indent { tabs: false, width }
}

/// The chars of line `line` without its `\n`.
#[must_use]
pub fn line_len(rope: &Rope, line: usize) -> usize {
    let slice = rope.line(line);
    let len = slice.len_chars();
    if len > 0 && slice.char(len - 1) == '\n' {
        len - 1
    } else {
        len
    }
}

/// Char offset of the end of `line`, before its `\n`.
#[must_use]
pub fn line_end(rope: &Rope, line: usize) -> usize {
    rope.line_to_char(line) + line_len(rope, line)
}

/// How many columns `ch` takes when it starts at column `col`.
#[must_use]
pub fn char_width(ch: char, col: usize, tab: usize) -> usize {
    match ch {
        '\t' => tab - col % tab,
        _ => ch.width().unwrap_or(0),
    }
}

/// The display column of char `offset` within its line.
#[must_use]
pub fn column(rope: &Rope, offset: usize, tab: usize) -> usize {
    let line = rope.char_to_line(offset);
    let start = rope.line_to_char(line);
    rope.slice(start..offset)
        .chars()
        .fold(0, |col, ch| col + char_width(ch, col, tab))
}

/// The char offset in `line` whose column is nearest `col`, without passing the line's end.
/// A column inside a wide char or a tab lands before it unless it is past its middle.
#[must_use]
pub fn offset_at_column(rope: &Rope, line: usize, col: usize, tab: usize) -> usize {
    let start = rope.line_to_char(line);
    let mut at = 0;
    for (index, ch) in rope.slice(start..line_end(rope, line)).chars().enumerate() {
        let width = char_width(ch, at, tab);
        if col < at + width {
            let past_middle = (col - at) * 2 >= width && width > 1;
            return start + index + usize::from(past_middle);
        }
        at += width;
    }
    line_end(rope, line)
}

/// Leading spaces and tabs of `line`.
#[must_use]
pub fn leading_whitespace(rope: &Rope, line: usize) -> String {
    rope.line(line)
        .chars()
        .take_while(|ch| *ch == ' ' || *ch == '\t')
        .collect()
}

/// What words are made of, for word motions, double-click and whole-word search.
#[must_use]
pub fn is_word_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_should_turn_crlf_into_lf_and_remember_it() {
        assert_eq!(
            normalize("a\r\nb\r\nc"),
            ("a\nb\nc".to_owned(), LineEnding::Crlf)
        );
        assert_eq!(
            normalize("a\nb\r\nc\n"),
            ("a\nb\nc\n".to_owned(), LineEnding::Lf)
        );
    }

    #[test]
    fn normalize_should_turn_a_lone_cr_into_a_line_break() {
        assert_eq!(normalize("a\rb").0, "a\nb");
    }

    #[test]
    fn detect_indent_should_find_the_step_not_the_depth() {
        let text = Rope::from("fn a() {\n  if x {\n    y();\n  }\n}\n");
        assert_eq!(
            detect_indent(&text.slice(..), Indent::default()),
            Indent {
                tabs: false,
                width: 2
            }
        );
    }

    #[test]
    fn detect_indent_should_prefer_tabs_when_most_lines_use_them() {
        let text = Rope::from("a\n\tb\n\tc\n  d\n");
        assert!(detect_indent(&text.slice(..), Indent::default()).tabs);
    }

    #[test]
    fn detect_indent_should_fall_back_without_indented_lines() {
        let text = Rope::from("a\nb\n");
        assert_eq!(
            detect_indent(&text.slice(..), Indent::default()),
            Indent::default()
        );
    }

    #[test]
    fn column_should_expand_tabs_and_count_wide_chars_twice() {
        let rope = Rope::from("\tx中y");
        assert_eq!(
            [1, 2, 3, 4].map(|offset| column(&rope, offset, 4)),
            [4, 5, 7, 8]
        );
    }

    #[test]
    fn offset_at_column_should_round_to_the_nearer_side_of_a_wide_char() {
        let rope = Rope::from("a中b\n");
        assert_eq!(
            [0, 1, 2, 3, 9].map(|col| offset_at_column(&rope, 0, col, 4)),
            [0, 1, 2, 2, 3]
        );
    }

    #[test]
    fn line_len_should_not_count_the_line_break() {
        let rope = Rope::from("ab\ncd");
        assert_eq!((line_len(&rope, 0), line_len(&rope, 1)), (2, 2));
    }
}
