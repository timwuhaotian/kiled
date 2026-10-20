//! Real language servers, end to end: start, open a file, and ask for diagnostics,
//! completion, hover and a definition (DESIGN.md §6.5). Each test skips itself when its
//! server isn't installed; `KILED_TEST_PATH` adds directories to look in.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    time::Duration,
};

use futures::{StreamExt, executor::block_on, future::select};
use kiled_core::{
    document::{Document, Position},
    language::{self, Server},
    lsp::{Client, Event, find_server, protocol, server_path},
    ops,
    selection::{Selection, Selections},
    text::Indent,
};
use serde_json::{Value, json};

fn search_path() -> OsString {
    let extra = std::env::var_os("KILED_TEST_PATH").unwrap_or_default();
    let mut dirs: Vec<PathBuf> = std::env::split_paths(&extra).collect();
    dirs.extend(std::env::split_paths(&server_path(None)));
    std::env::join_paths(dirs).unwrap_or_default()
}

#[expect(clippy::unwrap_used, reason = "test helper: a failure fails the test")]
fn project(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("kiled-servers-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for (path, text) in files {
        let path = dir.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    dir.canonicalize().unwrap()
}

/// Waits for `future` up to `seconds`.
async fn within<T>(seconds: u64, future: impl Future<Output = T>) -> Option<T> {
    let (done, timeout) = futures::channel::oneshot::channel::<()>();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(seconds));
        let _ = done.send(());
    });
    match select(Box::pin(future), timeout).await {
        futures::future::Either::Left((value, _)) => Some(value),
        futures::future::Either::Right(_) => None,
    }
}

struct Session {
    client: Client,
    events: futures::channel::mpsc::UnboundedReceiver<Event>,
    uri: String,
}

/// Starts `server` for `root` and opens `file` in it, or `None` if it isn't installed.
fn open(server: &str, root: &Path, file: &str, language_id: &str) -> Option<Session> {
    let spec: &Server = language::SERVERS.iter().find(|spec| spec.name == server)?;
    let path = search_path();
    let Some(program) = find_server(spec, root, &path) else {
        eprintln!("skipped: {server} isn't installed");
        return None;
    };
    let (client, events) = Client::start(&program, spec.args, root, &path).ok()?;
    let init =
        block_on(client.request("initialize", protocol::initialize(root, Value::Null))).ok()?;
    assert!(
        protocol::capabilities(&init).completion,
        "{server} completes"
    );
    client.notify("initialized", json!({}));
    let file = root.join(file);
    let uri = protocol::uri_of(&file);
    let text = std::fs::read_to_string(&file).ok()?;
    client.notify(
        "textDocument/didOpen",
        protocol::did_open(&uri, language_id, 0, &text),
    );
    Some(Session {
        client,
        events,
        uri,
    })
}

impl Session {
    /// Asks until the answer passes `done` (a server still loading answers emptily).
    fn ask(&self, method: &str, params: &Value, done: impl Fn(&Value) -> bool) -> Option<Value> {
        for _ in 0..60 {
            let answer = block_on(within(10, self.client.request(method, params.clone())))?;
            if let Ok(value) = answer
                && done(&value)
            {
                return Some(value);
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        None
    }

    /// The first diagnostics on `line`, pushed or (for a server that waits to be asked)
    /// pulled; a server publishes syntax errors before type errors.
    fn diagnostics(&mut self, pulls: bool, line: u32) -> Vec<protocol::Diagnostic> {
        let on_line = |list: &[protocol::Diagnostic]| list.iter().any(|d| d.start.line == line);
        if pulls {
            return self
                .ask(
                    "textDocument/diagnostic",
                    &protocol::document(&self.uri),
                    |value| protocol::pulled_diagnostics(value).is_some_and(|list| on_line(&list)),
                )
                .and_then(|value| protocol::pulled_diagnostics(&value))
                .unwrap_or_default();
        }
        let uri = self.uri.clone();
        block_on(within(60, async {
            while let Some(event) = self.events.next().await {
                if let Event::Notification { method, params } = event
                    && method == "textDocument/publishDiagnostics"
                    && let Some((from, list)) = protocol::diagnostics(&params)
                    && from == uri
                    && on_line(&list)
                {
                    return list;
                }
            }
            Vec::new()
        }))
        .unwrap_or_default()
    }
}

fn at(line: u32, character: u32) -> Position {
    Position { line, character }
}

#[test]
#[ignore = "needs rust-analyzer; run with --ignored"]
fn rust_analyzer_should_complete_hover_and_find_definitions() {
    let root = project(
        "rust",
        &[
            (
                "Cargo.toml",
                "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
            ),
            (
                "src/main.rs",
                "fn helper(n: u32) -> u32 { n + 1 }\n\nfn main() {\n    let v: Vec<u32> = Vec::new();\n    let x: u32 = \"no\";\n    helper(2);\n    v.\n}\n",
            ),
        ],
    );
    let Some(mut session) = open("rust-analyzer", &root, "src/main.rs", "rust") else {
        return;
    };
    let diagnostics = session.diagnostics(false, 4);
    let completion = session
        .ask(
            "textDocument/completion",
            &protocol::completion_params(&session.uri, at(6, 6), Some('.')),
            |value| !protocol::completion(value).0.is_empty(),
        )
        .map(|value| protocol::completion(&value).0)
        .unwrap_or_default();
    let hover = session
        .ask(
            "textDocument/hover",
            &protocol::document_position(&session.uri, at(5, 6)),
            |value| protocol::hover(value).is_some(),
        )
        .and_then(|value| protocol::hover(&value));
    let definition = session
        .ask(
            "textDocument/definition",
            &protocol::document_position(&session.uri, at(5, 6)),
            |value| !protocol::locations(value).is_empty(),
        )
        .map(|value| protocol::locations(&value))
        .unwrap_or_default();
    session.client.shutdown();
    assert!(
        diagnostics.iter().any(|d| d.start.line == 4),
        "{diagnostics:?}"
    );
    assert!(
        completion.iter().any(|item| item.label.starts_with("push")),
        "{:?}",
        completion
            .iter()
            .map(|item| &item.label)
            .collect::<Vec<_>>()
    );
    assert!(hover.is_some_and(|text| text.contains("fn helper")));
    assert_eq!(
        definition.first().map(|location| location.start.line),
        Some(0)
    );
}

#[test]
#[ignore = "needs TypeScript 7; run with --ignored"]
fn typescript_should_complete_hover_and_report_problems() {
    let root = project(
        "ts",
        &[
            (
                "tsconfig.json",
                "{ \"compilerOptions\": { \"strict\": true } }\n",
            ),
            (
                "app.ts",
                "interface User { name: string; age: number }\nconst user: User = { name: \"a\", age: 1 };\nconst n: number = \"three\";\nuser.\n",
            ),
        ],
    );
    let Some(mut session) = open("typescript", &root, "app.ts", "typescript") else {
        return;
    };
    let diagnostics = session.diagnostics(true, 2);
    let completion = session
        .ask(
            "textDocument/completion",
            &protocol::completion_params(&session.uri, at(3, 5), Some('.')),
            |value| !protocol::completion(value).0.is_empty(),
        )
        .map(|value| protocol::completion(&value).0)
        .unwrap_or_default();
    let hover = session
        .ask(
            "textDocument/hover",
            &protocol::document_position(&session.uri, at(1, 7)),
            |value| protocol::hover(value).is_some(),
        )
        .and_then(|value| protocol::hover(&value));
    session.client.shutdown();
    assert!(
        diagnostics.iter().any(|d| d.start.line == 2),
        "{diagnostics:?}"
    );
    assert!(
        completion.iter().any(|item| item.label == "name"),
        "{:?}",
        completion
            .iter()
            .map(|item| &item.label)
            .collect::<Vec<_>>()
    );
    assert!(hover.is_some_and(|text| text.contains("User")));
}

#[test]
#[ignore = "needs rust-analyzer; run with --ignored"]
fn rust_analyzer_should_follow_incremental_edits() {
    let text = "fn main() {\n    let v: Vec<u32> = Vec::new();\n    v.len();\n}\n";
    let root = project(
        "rust-edits",
        &[
            (
                "Cargo.toml",
                "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
            ),
            ("src/main.rs", text),
        ],
    );
    let Some(session) = open("rust-analyzer", &root, "src/main.rs", "rust") else {
        return;
    };
    // Type a new line after `v.len();` as the editor would: Enter, then "v." key by key.
    let rust = language::for_path(Path::new("main.rs"));
    let mut doc = Document::new(text, rust, Indent::default());
    let end_of_len = text.find("v.len();").unwrap() + "v.len();".len();
    doc.set_selections(Selections::single(Selection::cursor(end_of_len)));
    let mut version = 0;
    let mut send = |doc: &mut Document| {
        version += 1;
        let changes = doc.take_lsp_changes();
        session.client.notify(
            "textDocument/didChange",
            protocol::did_change(&session.uri, version, &changes, None),
        );
    };
    ops::newline(&mut doc);
    send(&mut doc);
    for ch in "v.".chars() {
        ops::type_char(&mut doc, ch);
        send(&mut doc);
    }
    let head = doc.selections().primary().head;
    let position = kiled_core::document::position_of(doc.rope(), head);
    let completion = session
        .ask(
            "textDocument/completion",
            &protocol::completion_params(&session.uri, position, Some('.')),
            |value| !protocol::completion(value).0.is_empty(),
        )
        .map(|value| protocol::completion(&value).0)
        .unwrap_or_default();
    session.client.shutdown();
    assert!(
        completion.iter().any(|item| item.label.starts_with("push")),
        "at {position:?}: {:?}",
        completion
            .iter()
            .map(|item| &item.label)
            .take(5)
            .collect::<Vec<_>>()
    );
}
