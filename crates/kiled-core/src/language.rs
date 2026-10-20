//! The languages Kiled knows: how a file is recognized, its tree-sitter grammar and highlight
//! query, its comment syntax and the language server that serves it (DESIGN.md §5).
//!
//! Adding a language is one entry in [`LANGUAGES`] (and, for a new server, one in
//! [`SERVERS`]). A file no entry claims opens as plain text.

use std::{
    path::Path,
    sync::{Arc, OnceLock},
};

use tree_sitter::{Language as Grammar, Query};
use tree_sitter_language::LanguageFn;

/// What a capture in a highlight query is drawn as. Themes give each a color.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Highlight {
    /// Comments and doc comments.
    Comment,
    /// Keywords and storage modifiers.
    Keyword,
    /// String literals.
    String,
    /// Escapes inside strings.
    Escape,
    /// Numbers.
    Number,
    /// Constants, booleans, `null`.
    Constant,
    /// Function and method names, macros.
    Function,
    /// Types, constructors, modules.
    Type,
    /// Fields, properties and mapping keys.
    Property,
    /// `self`, `this` and other built-in names.
    Builtin,
    /// Plain identifiers.
    Variable,
    /// Operators.
    Operator,
    /// Brackets and delimiters.
    Punctuation,
    /// Attributes and decorators.
    Attribute,
    /// Markup tags (JSX).
    Tag,
    /// Labels, lifetimes, YAML anchors.
    Label,
}

impl Highlight {
    /// The highlight a capture name stands for; the most specific known prefix wins.
    #[must_use]
    pub fn of_capture(name: &str) -> Option<Self> {
        const TABLE: &[(&str, Highlight)] = &[
            ("comment", Highlight::Comment),
            ("keyword", Highlight::Keyword),
            ("conditional", Highlight::Keyword),
            ("repeat", Highlight::Keyword),
            ("include", Highlight::Keyword),
            ("exception", Highlight::Keyword),
            ("storageclass", Highlight::Keyword),
            ("string.special.key", Highlight::Property),
            ("string.escape", Highlight::Escape),
            ("string", Highlight::String),
            ("character", Highlight::String),
            ("escape", Highlight::Escape),
            ("number", Highlight::Number),
            ("float", Highlight::Number),
            ("boolean", Highlight::Constant),
            ("constant", Highlight::Constant),
            ("function", Highlight::Function),
            ("method", Highlight::Function),
            ("constructor", Highlight::Type),
            ("type", Highlight::Type),
            ("module", Highlight::Type),
            ("namespace", Highlight::Type),
            ("property", Highlight::Property),
            ("field", Highlight::Property),
            ("variable.builtin", Highlight::Builtin),
            ("variable.member", Highlight::Property),
            ("variable", Highlight::Variable),
            ("operator", Highlight::Operator),
            ("punctuation.special", Highlight::Operator),
            ("punctuation", Highlight::Punctuation),
            ("attribute", Highlight::Attribute),
            ("tag", Highlight::Tag),
            ("label", Highlight::Label),
        ];
        TABLE
            .iter()
            .filter(|(prefix, _)| {
                name == *prefix
                    || name
                        .strip_prefix(prefix)
                        .is_some_and(|rest| rest.starts_with('.'))
            })
            .max_by_key(|(prefix, _)| prefix.len())
            .map(|(_, highlight)| *highlight)
    }
}

/// A language server Kiled can start, found in the project's `node_modules` or on the login
/// shell's `PATH` (DESIGN.md §6).
#[derive(Debug, PartialEq, Eq)]
pub struct Server {
    /// Its name, as `[servers]` in `config.toml` and the status bar write it.
    pub name: &'static str,
    /// The program.
    pub command: &'static str,
    /// Arguments that make it speak LSP over stdio.
    pub args: &'static [&'static str],
    /// How to install it, shown when it isn't found.
    pub install: &'static str,
    /// Files whose folder is a project root for it, when no folder is open.
    pub root_markers: &'static [&'static str],
    /// The npm package it comes in, if it does: the project's own install is preferred, and
    /// an older major version than this is passed over.
    pub npm: Option<(&'static str, u32)>,
}

/// Every server Kiled knows how to start.
pub static SERVERS: [Server; 6] = [
    Server {
        name: "rust-analyzer",
        command: "rust-analyzer",
        args: &[],
        install: "rustup component add rust-analyzer",
        root_markers: &["Cargo.toml"],
        npm: None,
    },
    // TypeScript 7's compiler is its own language server, native and fast.
    Server {
        name: "typescript",
        command: "tsc",
        args: &["--lsp", "--stdio"],
        install: "npm install -g typescript",
        root_markers: &["package.json", "tsconfig.json", "jsconfig.json"],
        npm: Some(("typescript", 7)),
    },
    // For TypeScript 6 and older, which still ship tsserver.
    Server {
        name: "typescript-language-server",
        command: "typescript-language-server",
        args: &["--stdio"],
        install: "npm install -g typescript-language-server typescript@6",
        root_markers: &["package.json", "tsconfig.json", "jsconfig.json"],
        npm: Some(("typescript-language-server", 0)),
    },
    Server {
        name: "vscode-json-language-server",
        command: "vscode-json-language-server",
        args: &["--stdio"],
        install: "npm install -g vscode-langservers-extracted",
        root_markers: &["package.json"],
        npm: Some(("vscode-langservers-extracted", 0)),
    },
    Server {
        name: "yaml-language-server",
        command: "yaml-language-server",
        args: &["--stdio"],
        install: "npm install -g yaml-language-server",
        root_markers: &[],
        npm: Some(("yaml-language-server", 0)),
    },
    Server {
        name: "taplo",
        command: "taplo",
        args: &["lsp", "stdio"],
        install: "brew install taplo",
        root_markers: &["Cargo.toml", "pyproject.toml", ".taplo.toml"],
        npm: None,
    },
];

/// TypeScript's own server first, then the one for older TypeScript.
const TYPESCRIPT: &[&str] = &["typescript", "typescript-language-server"];

fn server(name: &str) -> Option<&'static Server> {
    SERVERS.iter().find(|server| server.name == name)
}

/// A compiled highlight query and what each of its captures is drawn as.
pub struct Highlighter {
    /// The query, compiled against the language's grammar.
    pub query: Query,
    /// By capture index.
    pub kinds: Vec<Option<Highlight>>,
    /// By capture index: how many dotted parts the name has. Of two captures of one node,
    /// the more specific wins (`string.special.key` over `string`).
    pub specificity: Vec<usize>,
}

/// One language.
pub struct Language {
    /// As the status bar shows it.
    pub name: &'static str,
    /// The `languageId` language servers know it by.
    pub lsp_id: &'static str,
    /// Lowercase extensions without the dot.
    pub extensions: &'static [&'static str],
    /// Whole file names, for files whose extension says nothing (`Cargo.lock`).
    pub file_names: &'static [&'static str],
    /// What starts a line comment, for Toggle Line Comment.
    pub line_comment: Option<&'static str>,
    /// Quotes typed in pairs, like brackets are.
    pub quotes: &'static str,
    /// A line ending in one of these indents the next one.
    pub indent_after: &'static str,
    /// Servers to try, in order: the first one installed serves the language.
    servers: &'static [&'static str],
    grammar: LanguageFn,
    queries: &'static [&'static str],
    highlighter: OnceLock<Option<Arc<Highlighter>>>,
}

impl std::fmt::Debug for Language {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Language")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

impl PartialEq for Language {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

impl Language {
    /// The tree-sitter grammar.
    pub fn grammar(&self) -> Grammar {
        Grammar::new(self.grammar)
    }

    /// The servers that can serve it, the preferred first.
    pub fn servers(&self) -> impl Iterator<Item = &'static Server> + use<> {
        self.servers.iter().filter_map(|name| server(name))
    }

    /// The highlight query, compiled on first use. A query that doesn't compile is logged
    /// once, and the language is drawn without colors.
    pub fn highlighter(&self) -> Option<Arc<Highlighter>> {
        self.highlighter
            .get_or_init(|| {
                let source = self.queries.concat();
                match Query::new(&self.grammar(), &source) {
                    Ok(query) => {
                        let names = query.capture_names();
                        let kinds = names
                            .iter()
                            .map(|name| Highlight::of_capture(name))
                            .collect();
                        let specificity =
                            names.iter().map(|name| name.split('.').count()).collect();
                        Some(Arc::new(Highlighter {
                            query,
                            kinds,
                            specificity,
                        }))
                    }
                    Err(err) => {
                        log::error!("{} highlight query: {err}", self.name);
                        None
                    }
                }
            })
            .clone()
    }
}

const JS_QUERY: &str = tree_sitter_javascript::HIGHLIGHT_QUERY;
const JSX_QUERY: &str = tree_sitter_javascript::JSX_HIGHLIGHT_QUERY;
const TS_QUERY: &str = tree_sitter_typescript::HIGHLIGHTS_QUERY;

/// Every language Kiled highlights and serves.
pub static LANGUAGES: [Language; 9] = [
    Language {
        name: "Rust",
        lsp_id: "rust",
        extensions: &["rs"],
        file_names: &[],
        line_comment: Some("//"),
        // No `'`: it starts lifetimes far more often than char literals.
        quotes: "\"",
        indent_after: "{[(",
        servers: &["rust-analyzer"],
        grammar: tree_sitter_rust::LANGUAGE,
        queries: &[tree_sitter_rust::HIGHLIGHTS_QUERY],
        highlighter: OnceLock::new(),
    },
    // TypeScript's query adds to JavaScript's, as the grammar's tree-sitter.json says.
    Language {
        name: "TypeScript",
        lsp_id: "typescript",
        extensions: &["ts", "mts", "cts"],
        file_names: &[],
        line_comment: Some("//"),
        quotes: "\"'`",
        indent_after: "{[(",
        servers: TYPESCRIPT,
        grammar: tree_sitter_typescript::LANGUAGE_TYPESCRIPT,
        queries: &[JS_QUERY, TS_QUERY],
        highlighter: OnceLock::new(),
    },
    Language {
        name: "TypeScript JSX",
        lsp_id: "typescriptreact",
        extensions: &["tsx"],
        file_names: &[],
        line_comment: Some("//"),
        quotes: "\"'`",
        indent_after: "{[(",
        servers: TYPESCRIPT,
        grammar: tree_sitter_typescript::LANGUAGE_TSX,
        queries: &[JS_QUERY, JSX_QUERY, TS_QUERY],
        highlighter: OnceLock::new(),
    },
    Language {
        name: "JavaScript",
        lsp_id: "javascript",
        extensions: &["js", "mjs", "cjs"],
        file_names: &[],
        line_comment: Some("//"),
        quotes: "\"'`",
        indent_after: "{[(",
        servers: TYPESCRIPT,
        grammar: tree_sitter_javascript::LANGUAGE,
        queries: &[JS_QUERY, JSX_QUERY],
        highlighter: OnceLock::new(),
    },
    Language {
        name: "JavaScript JSX",
        lsp_id: "javascriptreact",
        extensions: &["jsx"],
        file_names: &[],
        line_comment: Some("//"),
        quotes: "\"'`",
        indent_after: "{[(",
        servers: TYPESCRIPT,
        grammar: tree_sitter_javascript::LANGUAGE,
        queries: &[JS_QUERY, JSX_QUERY],
        highlighter: OnceLock::new(),
    },
    // JSON with comments: the files whose tools allow them.
    Language {
        name: "JSON with Comments",
        lsp_id: "jsonc",
        extensions: &["jsonc"],
        file_names: &[
            "tsconfig.json",
            "jsconfig.json",
            ".eslintrc.json",
            ".babelrc",
            "devcontainer.json",
        ],
        line_comment: Some("//"),
        quotes: "\"",
        indent_after: "{[",
        servers: &["vscode-json-language-server"],
        grammar: tree_sitter_json::LANGUAGE,
        queries: &[tree_sitter_json::HIGHLIGHTS_QUERY],
        highlighter: OnceLock::new(),
    },
    Language {
        name: "JSON",
        lsp_id: "json",
        extensions: &["json"],
        file_names: &[".prettierrc"],
        line_comment: None,
        quotes: "\"",
        indent_after: "{[",
        servers: &["vscode-json-language-server"],
        grammar: tree_sitter_json::LANGUAGE,
        queries: &[tree_sitter_json::HIGHLIGHTS_QUERY],
        highlighter: OnceLock::new(),
    },
    Language {
        name: "YAML",
        lsp_id: "yaml",
        extensions: &["yaml", "yml"],
        file_names: &[],
        line_comment: Some("#"),
        quotes: "\"'",
        indent_after: ":",
        servers: &["yaml-language-server"],
        grammar: tree_sitter_yaml::LANGUAGE,
        queries: &[tree_sitter_yaml::HIGHLIGHTS_QUERY],
        highlighter: OnceLock::new(),
    },
    Language {
        name: "TOML",
        lsp_id: "toml",
        extensions: &["toml"],
        file_names: &["Cargo.lock", "uv.lock", "poetry.lock"],
        line_comment: Some("#"),
        quotes: "\"'",
        indent_after: "[{",
        servers: &["taplo"],
        grammar: tree_sitter_toml_ng::LANGUAGE,
        queries: &[tree_sitter_toml_ng::HIGHLIGHTS_QUERY],
        highlighter: OnceLock::new(),
    },
];

/// Every language.
pub fn all() -> impl Iterator<Item = &'static Language> {
    LANGUAGES.iter()
}

/// The language of the file at `path`: by its whole name first, then its extension.
#[must_use]
pub fn for_path(path: &Path) -> Option<&'static Language> {
    let name = path.file_name()?.to_str()?;
    if let Some(language) = all().find(|language| language.file_names.contains(&name)) {
        return Some(language);
    }
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    all().find(|language| language.extensions.contains(&extension.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_highlight_query_should_compile_against_its_grammar() {
        let failed: Vec<&str> = all()
            .filter(|language| language.highlighter().is_none())
            .map(|language| language.name)
            .collect();
        assert_eq!(failed, Vec::<&str>::new());
    }

    #[test]
    fn for_path_should_go_by_name_then_extension() {
        let name = |path: &str| for_path(Path::new(path)).map(|language| language.name);
        assert_eq!(
            [
                name("src/main.rs"),
                name("App.TSX"),
                name("tsconfig.json"),
                name("package.json"),
                name("Cargo.lock"),
                name("ci.yml"),
                name("README.md"),
            ],
            [
                Some("Rust"),
                Some("TypeScript JSX"),
                Some("JSON with Comments"),
                Some("JSON"),
                Some("TOML"),
                Some("YAML"),
                None
            ]
        );
    }

    #[test]
    fn capture_names_should_map_by_their_most_specific_prefix() {
        assert_eq!(
            [
                "function.method",
                "string.special.key",
                "string.special",
                "variable.builtin",
                "variable.parameter",
                "punctuation.bracket",
                "embedded",
            ]
            .map(Highlight::of_capture),
            [
                Some(Highlight::Function),
                Some(Highlight::Property),
                Some(Highlight::String),
                Some(Highlight::Builtin),
                Some(Highlight::Variable),
                Some(Highlight::Punctuation),
                None
            ]
        );
    }

    #[test]
    fn every_language_server_should_be_known() {
        assert!(all().all(|language| language.servers().count() == language.servers.len()));
    }
}
