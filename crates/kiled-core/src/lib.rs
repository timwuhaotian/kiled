//! Kiled's editing core, with no UI: the text and its selections, undo, the edits every
//! command makes, tree-sitter syntax, search, files on disk, and the language server client
//! (DESIGN.md §3).
#![deny(missing_docs)]

pub mod document;
pub mod edit;
pub mod files;
pub mod fuzzy;
pub mod language;
pub mod lsp;
pub mod movement;
pub mod ops;
pub mod search;
pub mod selection;
pub mod syntax;
mod sys;
pub mod text;
