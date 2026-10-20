//! Find and replace within a document: literal or regex, with or without case, whole
//! words or not. Every mode compiles to one `regex`, which folds case by Unicode's rules
//! instead of lowercasing text (which can change its length and so its offsets).

use std::ops::Range;

use regex::{Regex, RegexBuilder};
use ropey::Rope;

/// At most this many matches are found; a count beyond it shows as "10000+".
pub const MATCH_LIMIT: usize = 10_000;

/// What to look for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Query {
    /// The text typed.
    pub text: String,
    /// Match case.
    pub case_sensitive: bool,
    /// Match whole words only.
    pub whole_word: bool,
    /// `text` is a regular expression.
    pub regex: bool,
}

impl Query {
    /// The query compiled, or why it doesn't compile. An empty query finds nothing.
    ///
    /// # Errors
    /// If `text` is an invalid regular expression.
    pub fn compile(&self) -> Result<Option<Regex>, String> {
        if self.text.is_empty() {
            return Ok(None);
        }
        let pattern = if self.regex {
            self.text.clone()
        } else {
            regex::escape(&self.text)
        };
        let pattern = if self.whole_word {
            format!(r"\b(?:{pattern})\b")
        } else {
            pattern
        };
        RegexBuilder::new(&pattern)
            .case_insensitive(!self.case_sensitive)
            .multi_line(true)
            .build()
            .map(Some)
            .map_err(|err| err.to_string())
    }
}

/// Every non-empty match in `rope`, as char ranges, up to [`MATCH_LIMIT`].
#[must_use]
pub fn find_all(rope: &Rope, regex: &Regex) -> Vec<Range<usize>> {
    let haystack = rope.to_string();
    let mut chars = CharCounter::new(&haystack);
    regex
        .find_iter(&haystack)
        .filter(|found| !found.is_empty())
        .take(MATCH_LIMIT)
        .map(|found| chars.at(found.start())..chars.at(found.end()))
        .collect()
}

/// What replaces `matched`: `template` itself, or for a regex its `$1`-style groups filled
/// in from the match.
#[must_use]
pub fn replacement(regex: &Regex, is_regex: bool, matched: &str, template: &str) -> String {
    if !is_regex {
        return template.to_owned();
    }
    match regex.captures(matched) {
        Some(captures) => {
            let mut out = String::new();
            captures.expand(template, &mut out);
            out
        }
        None => template.to_owned(),
    }
}

/// Converts increasing byte offsets of one string to char offsets in a single pass.
struct CharCounter<'a> {
    text: &'a str,
    byte: usize,
    chars: usize,
}

impl<'a> CharCounter<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            text,
            byte: 0,
            chars: 0,
        }
    }

    fn at(&mut self, byte: usize) -> usize {
        if byte < self.byte {
            self.byte = 0;
            self.chars = 0;
        }
        self.chars += self.text[self.byte..byte].chars().count();
        self.byte = byte;
        self.chars
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find(text: &str, query: &Query) -> Vec<Range<usize>> {
        let regex = query.compile().unwrap().unwrap();
        find_all(&Rope::from(text), &regex)
    }

    fn literal(text: &str) -> Query {
        Query {
            text: text.into(),
            ..Query::default()
        }
    }

    #[test]
    fn literal_search_should_ignore_case_and_escape_specials_by_default() {
        assert_eq!(find("A.b a.B axb", &literal("a.b")), vec![0..3, 4..7]);
    }

    #[test]
    fn whole_words_and_case_should_narrow_the_matches() {
        let query = Query {
            whole_word: true,
            case_sensitive: true,
            ..literal("foo")
        };
        assert_eq!(find("foo foobar Foo foo", &query), vec![0..3, 15..18]);
    }

    #[test]
    fn matches_should_be_char_offsets_after_wide_text() {
        assert_eq!(find("中文 x 中文 x", &literal("x")), vec![3..4, 8..9]);
    }

    #[test]
    fn a_bad_regex_should_say_why() {
        let query = Query {
            regex: true,
            ..literal("(")
        };
        assert!(query.compile().is_err());
    }

    #[test]
    fn regex_replacement_should_fill_in_groups() {
        let query = Query {
            regex: true,
            ..literal(r"(\w+)=(\w+)")
        };
        let regex = query.compile().unwrap().unwrap();
        assert_eq!(replacement(&regex, true, "a=b", "$2=$1"), "b=a");
    }
}
