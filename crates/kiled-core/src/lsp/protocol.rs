//! The part of LSP Kiled speaks, built and read as JSON: opening and changing documents,
//! diagnostics, completion, hover, go to definition and formatting (DESIGN.md §6.2).
//!
//! Positions are UTF-16, the only encoding every server supports; [`crate::document`]
//! converts them.

use std::{
    fmt::Write as _,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

use crate::{
    document::{LspChange, Position, Severity},
    text::Indent,
};

/// What a server said it can do, as far as Kiled uses it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each is a capability a server has or hasn't"
)]
pub struct Capabilities {
    /// It wants the whole text on every change instead of the edits.
    pub full_sync: bool,
    /// Chars that open completion by themselves (`.`, `::`'s second `:`).
    pub completion_triggers: Vec<String>,
    /// It completes at all.
    pub completion: bool,
    /// It fills in a completion's details (auto-imports) when asked.
    pub completion_resolve: bool,
    /// Hover.
    pub hover: bool,
    /// Go to Definition.
    pub definition: bool,
    /// Format Document.
    pub formatting: bool,
    /// It answers `textDocument/diagnostic`: problems are asked for, not (only) pushed.
    pub pull_diagnostics: bool,
}

/// `file://` URI of `path`.
#[must_use]
pub fn uri_of(path: &Path) -> String {
    let mut uri = String::from("file://");
    for byte in path.to_string_lossy().bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' => {
                uri.push(char::from(byte));
            }
            _ => {
                let _ = write!(uri, "%{byte:02X}");
            }
        }
    }
    uri
}

/// The path a `file://` URI names.
#[must_use]
pub fn path_of(uri: &str) -> Option<PathBuf> {
    let encoded = uri.strip_prefix("file://")?;
    let mut bytes = Vec::with_capacity(encoded.len());
    let mut input = encoded.bytes();
    while let Some(byte) = input.next() {
        if byte == b'%' {
            let high = char::from(input.next()?).to_digit(16)?;
            let low = char::from(input.next()?).to_digit(16)?;
            bytes.push(u8::try_from(high * 16 + low).ok()?);
        } else {
            bytes.push(byte);
        }
    }
    Some(PathBuf::from(String::from_utf8(bytes).ok()?))
}

/// The one workspace folder Kiled opens.
#[must_use]
pub fn workspace_folders(root: &Path) -> Value {
    let name = root
        .file_name()
        .map_or_else(|| root.to_string_lossy(), |name| name.to_string_lossy());
    json!([{"uri": uri_of(root), "name": name}])
}

/// `initialize`'s params: what Kiled can do, which is little on purpose.
#[must_use]
pub fn initialize(root: &Path, options: Value) -> Value {
    let mut params = json!({
        "processId": std::process::id(),
        "clientInfo": {"name": "Kiled", "version": env!("CARGO_PKG_VERSION")},
        "rootUri": uri_of(root),
        "rootPath": root.to_string_lossy(),
        "workspaceFolders": workspace_folders(root),
        "capabilities": {
            "general": {"positionEncodings": ["utf-16"]},
            "workspace": {"configuration": true, "workspaceFolders": true},
            "window": {"workDoneProgress": true},
            "textDocument": {
                "synchronization": {"didSave": true, "dynamicRegistration": false},
                "completion": {
                    "completionItem": {
                        "snippetSupport": false,
                        "insertReplaceSupport": false,
                        "documentationFormat": ["plaintext"],
                        "resolveSupport": {"properties": ["additionalTextEdits", "detail"]},
                    },
                    "contextSupport": true,
                },
                "hover": {"contentFormat": ["markdown", "plaintext"]},
                "definition": {"linkSupport": true},
                "formatting": {"dynamicRegistration": false},
                "publishDiagnostics": {"relatedInformation": false},
                "diagnostic": {"dynamicRegistration": false, "relatedDocumentSupport": false},
            },
        },
    });
    params["initializationOptions"] = options;
    params
}

/// What `initialize`'s result says the server can do.
#[must_use]
pub fn capabilities(result: &Value) -> Capabilities {
    let caps = &result["capabilities"];
    let sync = match &caps["textDocumentSync"] {
        Value::Number(kind) => kind.as_u64(),
        sync => sync["change"].as_u64(),
    };
    let provided = |value: &Value| !value.is_null() && *value != Value::Bool(false);
    Capabilities {
        full_sync: sync == Some(1),
        completion_triggers: caps["completionProvider"]["triggerCharacters"]
            .as_array()
            .map(|chars| {
                chars
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default(),
        completion: provided(&caps["completionProvider"]),
        completion_resolve: caps["completionProvider"]["resolveProvider"]
            .as_bool()
            .unwrap_or(false),
        hover: provided(&caps["hoverProvider"]),
        definition: provided(&caps["definitionProvider"]),
        formatting: provided(&caps["documentFormattingProvider"]),
        pull_diagnostics: provided(&caps["diagnosticProvider"]),
    }
}

/// A position as JSON.
#[must_use]
pub fn position(at: Position) -> Value {
    json!({"line": at.line, "character": at.character})
}

fn read_position(value: &Value) -> Option<Position> {
    Some(Position {
        line: u32::try_from(value["line"].as_u64()?).ok()?,
        character: u32::try_from(value["character"].as_u64()?).ok()?,
    })
}

fn read_range(value: &Value) -> Option<(Position, Position)> {
    Some((
        read_position(&value["start"])?,
        read_position(&value["end"])?,
    ))
}

/// `textDocument/didOpen`'s params.
#[must_use]
pub fn did_open(uri: &str, language_id: &str, version: i64, text: &str) -> Value {
    json!({"textDocument": {
        "uri": uri, "languageId": language_id, "version": version, "text": text,
    }})
}

/// `textDocument/didChange`'s params: the edits, or the whole text for a server that wants
/// that (`full`).
#[must_use]
pub fn did_change(uri: &str, version: i64, changes: &[LspChange], full: Option<&str>) -> Value {
    let content: Vec<Value> = match full {
        Some(text) => vec![json!({"text": text})],
        None => changes
            .iter()
            .map(|change| {
                json!({
                    "range": {"start": position(change.start), "end": position(change.end)},
                    "text": change.text,
                })
            })
            .collect(),
    };
    json!({"textDocument": {"uri": uri, "version": version}, "contentChanges": content})
}

/// Params naming just the document: `didSave`, `didClose`, `formatting` minus options.
#[must_use]
pub fn document(uri: &str) -> Value {
    json!({"textDocument": {"uri": uri}})
}

/// Params for a request at a place in a document: hover, definition, completion.
#[must_use]
pub fn document_position(uri: &str, at: Position) -> Value {
    json!({"textDocument": {"uri": uri}, "position": position(at)})
}

/// `textDocument/completion`'s params; `trigger` is the char that opened it, if one did.
#[must_use]
pub fn completion_params(uri: &str, at: Position, trigger: Option<char>) -> Value {
    let mut params = document_position(uri, at);
    params["context"] = match trigger {
        Some(ch) => json!({"triggerKind": 2, "triggerCharacter": ch.to_string()}),
        None => json!({"triggerKind": 1}),
    };
    params
}

/// `textDocument/formatting`'s params.
#[must_use]
pub fn formatting_params(uri: &str, indent: Indent) -> Value {
    json!({
        "textDocument": {"uri": uri},
        "options": {"tabSize": indent.width, "insertSpaces": !indent.tabs},
    })
}

/// A diagnostic as the server sent it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    /// Start.
    pub start: Position,
    /// End.
    pub end: Position,
    /// How bad.
    pub severity: Severity,
    /// What it says.
    pub message: String,
    /// Who says it.
    pub source: Option<String>,
}

fn diagnostic_list(list: &Value) -> Vec<Diagnostic> {
    list.as_array()
        .map(|list| {
            list.iter()
                .filter_map(|item| {
                    let (start, end) = read_range(&item["range"])?;
                    let severity = match item["severity"].as_u64() {
                        Some(2) => Severity::Warning,
                        Some(3) => Severity::Information,
                        Some(4) => Severity::Hint,
                        _ => Severity::Error,
                    };
                    Some(Diagnostic {
                        start,
                        end,
                        severity,
                        message: item["message"].as_str()?.to_owned(),
                        source: item["source"].as_str().map(str::to_owned),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// `textDocument/publishDiagnostics`: the document's URI and its diagnostics.
#[must_use]
pub fn diagnostics(params: &Value) -> Option<(String, Vec<Diagnostic>)> {
    let uri = params["uri"].as_str()?.to_owned();
    Some((uri, diagnostic_list(&params["diagnostics"])))
}

/// `textDocument/diagnostic`'s result: the document's diagnostics, or `None` when the
/// server says they are unchanged.
#[must_use]
pub fn pulled_diagnostics(result: &Value) -> Option<Vec<Diagnostic>> {
    (result["kind"] == "full").then(|| diagnostic_list(&result["items"]))
}

/// An edit a server asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextEdit {
    /// Start of what is replaced.
    pub start: Position,
    /// End of what is replaced.
    pub end: Position,
    /// What replaces it.
    pub text: String,
}

fn read_edit(value: &Value) -> Option<TextEdit> {
    let (start, end) = read_range(&value["range"])?;
    Some(TextEdit {
        start,
        end,
        text: value["newText"].as_str()?.to_owned(),
    })
}

/// A list of `TextEdit`s (formatting's result, a completion's extra edits).
#[must_use]
pub fn text_edits(value: &Value) -> Vec<TextEdit> {
    value
        .as_array()
        .map(|edits| edits.iter().filter_map(read_edit).collect())
        .unwrap_or_default()
}

/// One completion.
#[derive(Clone, Debug, PartialEq)]
pub struct CompletionItem {
    /// As listed.
    pub label: String,
    /// Shown after the label: a type or a signature.
    pub detail: Option<String>,
    /// LSP's kind number (function, field, keyword…).
    pub kind: Option<u64>,
    /// What typing narrows it by.
    pub filter_text: String,
    /// How it sorts among equals.
    pub sort_text: String,
    /// What is inserted.
    pub insert_text: String,
    /// Where it is inserted, if the server says.
    pub edit: Option<TextEdit>,
    /// Further edits (an import), if already known.
    pub additional: Vec<TextEdit>,
    /// The item as sent, for `completionItem/resolve`.
    pub raw: Value,
}

/// `textDocument/completion`'s result: the items, and whether typing more should ask again
/// instead of narrowing these.
#[must_use]
pub fn completion(result: &Value) -> (Vec<CompletionItem>, bool) {
    let (items, incomplete) = match result {
        Value::Array(items) => (items.as_slice(), false),
        Value::Object(_) => (
            result["items"].as_array().map_or(&[][..], Vec::as_slice),
            result["isIncomplete"].as_bool().unwrap_or(false),
        ),
        _ => (&[][..], false),
    };
    let items = items
        .iter()
        .filter_map(|item| {
            let label = item["label"].as_str()?.to_owned();
            let edit = read_edit(&item["textEdit"]);
            let insert_text = edit.as_ref().map_or_else(
                || {
                    item["insertText"]
                        .as_str()
                        .map_or_else(|| label.clone(), str::to_owned)
                },
                |edit| edit.text.clone(),
            );
            Some(CompletionItem {
                detail: item["detail"].as_str().map(str::to_owned),
                kind: item["kind"].as_u64(),
                filter_text: item["filterText"]
                    .as_str()
                    .map_or_else(|| label.clone(), str::to_owned),
                sort_text: item["sortText"]
                    .as_str()
                    .map_or_else(|| label.clone(), str::to_owned),
                insert_text,
                edit,
                additional: text_edits(&item["additionalTextEdits"]),
                raw: item.clone(),
                label,
            })
        })
        .collect();
    (items, incomplete)
}

/// A hover's contents as plain text: code fences dropped, rules turned into blank lines,
/// markdown's backslash escapes undone.
#[must_use]
pub fn hover(result: &Value) -> Option<String> {
    fn piece(value: &Value) -> Option<String> {
        match value {
            Value::String(text) => Some(text.clone()),
            Value::Object(_) => value["value"].as_str().map(str::to_owned),
            _ => None,
        }
    }
    let contents = &result["contents"];
    let raw = match contents {
        Value::Array(parts) => parts
            .iter()
            .filter_map(piece)
            .collect::<Vec<_>>()
            .join("\n\n"),
        _ => piece(contents)?,
    };
    let mut lines: Vec<String> = Vec::new();
    for line in raw.lines() {
        let trimmed = line.trim_end();
        if trimmed.starts_with("```") {
            continue;
        }
        if trimmed == "---" {
            lines.push(String::new());
            continue;
        }
        let mut out = String::with_capacity(trimmed.len());
        let mut chars = trimmed.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch == '\\' && chars.peek().is_some_and(char::is_ascii_punctuation) {
                continue;
            }
            out.push(ch);
        }
        lines.push(out);
    }
    let text = lines.join("\n");
    let mut text = text.trim().to_owned();
    while text.contains("\n\n\n") {
        text = text.replace("\n\n\n", "\n\n");
    }
    (!text.is_empty()).then_some(text)
}

/// A place in a file, from Go to Definition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    /// Its file.
    pub uri: String,
    /// Start.
    pub start: Position,
    /// End.
    pub end: Position,
}

/// `textDocument/definition`'s result: a location, a list of them, or of links.
#[must_use]
pub fn locations(result: &Value) -> Vec<Location> {
    let one = |value: &Value| -> Option<Location> {
        let uri = value["uri"]
            .as_str()
            .or_else(|| value["targetUri"].as_str())?;
        let range = if value["targetSelectionRange"].is_object() {
            &value["targetSelectionRange"]
        } else if value["range"].is_object() {
            &value["range"]
        } else {
            &value["targetRange"]
        };
        let (start, end) = read_range(range)?;
        Some(Location {
            uri: uri.to_owned(),
            start,
            end,
        })
    };
    match result {
        Value::Array(items) => items.iter().filter_map(one).collect(),
        Value::Object(_) => one(result).into_iter().collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uris_should_round_trip_paths_with_spaces_and_unicode() {
        let path = Path::new("/Users/me/my code/中文.rs");
        let uri = uri_of(path);
        assert_eq!(
            (uri.as_str(), path_of(&uri)),
            (
                "file:///Users/me/my%20code/%E4%B8%AD%E6%96%87.rs",
                Some(path.to_owned())
            )
        );
    }

    #[test]
    fn capabilities_should_read_sync_kind_and_triggers() {
        let caps = capabilities(&json!({"capabilities": {
            "textDocumentSync": {"change": 1},
            "completionProvider": {"triggerCharacters": [".", ":"], "resolveProvider": true},
            "hoverProvider": true,
            "documentFormattingProvider": false,
        }}));
        assert_eq!(
            caps,
            Capabilities {
                full_sync: true,
                completion_triggers: vec![".".into(), ":".into()],
                completion: true,
                completion_resolve: true,
                hover: true,
                definition: false,
                formatting: false,
                pull_diagnostics: false,
            }
        );
    }

    #[test]
    fn completion_should_prefer_the_text_edit_and_read_lists_or_objects() {
        let (items, incomplete) = completion(&json!({"isIncomplete": true, "items": [
            {"label": "len", "insertText": "len()"},
            {"label": "push", "textEdit": {"range": {
                "start": {"line": 0, "character": 2}, "end": {"line": 0, "character": 4}
            }, "newText": "push"}},
        ]}));
        assert_eq!(
            (
                incomplete,
                items[0].insert_text.as_str(),
                items[1].edit.as_ref().map(|edit| edit.start.character)
            ),
            (true, "len()", Some(2))
        );
        assert_eq!(completion(&json!([{"label": "x"}])).0.len(), 1);
    }

    #[test]
    fn hover_should_drop_fences_and_rules() {
        let text = hover(&json!({"contents": {"kind": "markdown",
            "value": "```rust\nfn len(&self) -> usize\n```\n\n---\n\nReturns the length\\_of it."}}));
        assert_eq!(
            text.as_deref(),
            Some("fn len(&self) -> usize\n\nReturns the length_of it.")
        );
    }

    #[test]
    fn locations_should_read_links_and_plain_locations() {
        let range =
            json!({"start": {"line": 3, "character": 1}, "end": {"line": 3, "character": 4}});
        let links = locations(&json!([{"targetUri": "file:///a.rs", "targetRange": range,
            "targetSelectionRange": range}]));
        let plain = locations(&json!({"uri": "file:///b.rs", "range": range}));
        assert_eq!(
            (links[0].uri.as_str(), plain[0].start.line),
            ("file:///a.rs", 3)
        );
    }

    #[test]
    fn pulled_diagnostics_should_be_none_when_unchanged() {
        let item = json!({"range": {"start": {"line": 0, "character": 0},
            "end": {"line": 0, "character": 1}}, "message": "bad"});
        assert_eq!(
            (
                pulled_diagnostics(&json!({"kind": "full", "items": [item]}))
                    .map(|list| list.len()),
                pulled_diagnostics(&json!({"kind": "unchanged", "resultId": "1"}))
            ),
            (Some(1), None)
        );
    }

    #[test]
    fn diagnostics_should_default_to_errors() {
        let (uri, list) = diagnostics(&json!({"uri": "file:///a.rs", "diagnostics": [
            {"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}},
             "message": "bad", "source": "rustc"},
            {"range": {"start": {"line": 1, "character": 0}, "end": {"line": 1, "character": 1}},
             "message": "meh", "severity": 2},
        ]}))
        .unwrap();
        assert_eq!(
            (
                uri.as_str(),
                list.iter().map(|d| d.severity).collect::<Vec<_>>()
            ),
            ("file:///a.rs", vec![Severity::Error, Severity::Warning])
        );
    }
}
