//! The window's language servers: one per server and project root, started when the first
//! file in its language opens, and ended when the window closes (DESIGN.md §6).
//!
//! Editors call in with what a message needs; this never reads an editor while the editor
//! is calling it, only from its own tasks (a server finishing `initialize`, an event).

use std::{
    collections::BTreeMap,
    ffi::OsString,
    path::{Path, PathBuf},
    sync::Arc,
};

use futures::StreamExt;
use gpui::{AppContext as _, Context, EventEmitter, SharedString, Task, WeakEntity};
use kiled_core::{
    document::LspChange,
    language::{self, Language},
    lsp::{Client, Event, find_program, find_server, login_path, protocol, server_path},
};
use ropey::Rope;
use serde_json::{Value, json};

use crate::{editor::Editor, settings::Settings};

/// A server's index in the window's list.
pub type ServerId = usize;

/// What the servers say that the window shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LspEvent {
    /// A server's error or warning for the user.
    Message(SharedString),
}

/// Where a server is in its life.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    /// Waiting for the login shell's `PATH` to find it.
    Waiting,
    /// Started; `initialize` not yet answered.
    Starting,
    Running,
    /// Shut down with the folder it served.
    Stopped,
    /// Not found, failed to start, or exited; says why.
    Failed(SharedString),
}

struct Server {
    /// The servers that can serve these documents, the preferred first.
    candidates: Vec<&'static language::Server>,
    /// The one that does: the first installed, once looked for.
    spec: &'static language::Server,
    root: PathBuf,
    state: State,
    client: Option<Arc<Client>>,
    caps: protocol::Capabilities,
    /// Open documents it serves, by editor and URI.
    docs: Vec<(WeakEntity<Editor>, String)>,
    /// Work in progress (`$/progress`), by token.
    progress: BTreeMap<String, String>,
    /// Reads what the server sends; dropped with the server.
    events_task: Option<Task<()>>,
}

pub struct Lsp {
    /// `PATH` to find and run servers with; `None` until the login shell answered.
    path: Option<OsString>,
    looking_up_path: Option<Task<()>>,
    servers: Vec<Server>,
}

impl EventEmitter<LspEvent> for Lsp {}

/// Where a server serving `path` runs: the open folder holding it, else the nearest folder
/// above it with one of the server's root markers, else the file's own folder.
fn root_for(path: &Path, folder: Option<&Path>, markers: &[&str]) -> PathBuf {
    if let Some(folder) = folder
        && path.starts_with(folder)
    {
        return folder.to_owned();
    }
    let dir = path.parent().unwrap_or_else(|| Path::new("/"));
    dir.ancestors()
        .find(|dir| markers.iter().any(|marker| dir.join(marker).exists()))
        .unwrap_or(dir)
        .to_owned()
}

/// `initializationOptions` a server needs to do its part.
fn initialization_options(server: &str) -> Value {
    match server {
        // Formats only when told it may.
        "vscode-json-language-server" => json!({"provideFormatter": true}),
        _ => Value::Null,
    }
}

impl Lsp {
    pub fn new(_: &mut Context<Self>) -> Self {
        Self {
            path: None,
            looking_up_path: None,
            servers: Vec::new(),
        }
    }

    /// Asks the login shell for `PATH` once, then starts the servers waiting for it.
    fn look_up_path(&mut self, cx: &mut Context<Self>) {
        if self.path.is_some() || self.looking_up_path.is_some() {
            return;
        }
        let lookup = cx.background_spawn(async move { server_path(login_path().as_deref()) });
        self.looking_up_path = Some(cx.spawn(async move |this, cx| {
            let path = lookup.await;
            let _ = this.update(cx, |this, cx| {
                this.path = Some(path);
                this.looking_up_path = None;
                for id in 0..this.servers.len() {
                    if this.servers[id].state == State::Waiting {
                        this.start(id, cx);
                    }
                }
            });
        }));
    }

    /// Registers an open document with the server for its language and root, starting the
    /// server if this is its first. `None` when the language has no server, or it is off.
    #[expect(clippy::too_many_arguments, reason = "what didOpen needs, passed in")]
    pub fn attach(
        &mut self,
        editor: WeakEntity<Editor>,
        language: &'static Language,
        path: &Path,
        folder: Option<&Path>,
        version: i64,
        rope: &Rope,
        cx: &mut Context<Self>,
    ) -> Option<ServerId> {
        let settings = cx.global::<Settings>();
        let candidates: Vec<&'static language::Server> = language
            .servers()
            .filter(|server| !settings.servers.get(server.name).is_some_and(Vec::is_empty))
            .collect();
        let spec = *candidates.first()?;
        let root = root_for(path, folder, spec.root_markers);
        let uri = protocol::uri_of(path);
        let existing = self
            .servers
            .iter()
            .position(|server| server.candidates[0].name == spec.name && server.root == root);
        let id = match existing {
            Some(id) if !matches!(self.servers[id].state, State::Stopped) => id,
            _ => {
                self.servers.push(Server {
                    candidates,
                    spec,
                    root,
                    state: State::Waiting,
                    client: None,
                    caps: protocol::Capabilities::default(),
                    docs: Vec::new(),
                    progress: BTreeMap::new(),
                    events_task: None,
                });
                let id = self.servers.len() - 1;
                if self.path.is_some() {
                    self.start(id, cx);
                } else {
                    self.look_up_path(cx);
                }
                id
            }
        };
        let server = &mut self.servers[id];
        if server.state == State::Running
            && let Some(client) = &server.client
        {
            client.notify(
                "textDocument/didOpen",
                protocol::did_open(&uri, language.lsp_id, version, &rope.to_string()),
            );
        }
        server.docs.push((editor, uri));
        cx.notify();
        Some(id)
    }

    /// The document is closed.
    pub fn detach(&mut self, id: ServerId, editor: &WeakEntity<Editor>, uri: &str) {
        let Some(server) = self.servers.get_mut(id) else {
            return;
        };
        server.docs.retain(|(open, _)| open != editor);
        if server.state == State::Running
            && let Some(client) = &server.client
        {
            client.notify("textDocument/didClose", protocol::document(uri));
        }
    }

    fn running(&self, id: ServerId) -> Option<&Server> {
        self.servers
            .get(id)
            .filter(|server| server.state == State::Running)
    }

    /// The document changed; `rope` is its text now, for a server that wants it whole.
    pub fn did_change(
        &self,
        id: ServerId,
        uri: &str,
        version: i64,
        changes: &[LspChange],
        rope: &Rope,
    ) {
        let Some(server) = self.running(id) else {
            return;
        };
        let Some(client) = &server.client else {
            return;
        };
        let full = server.caps.full_sync.then(|| rope.to_string());
        client.notify(
            "textDocument/didChange",
            protocol::did_change(uri, version, changes, full.as_deref()),
        );
    }

    pub fn did_save(&self, id: ServerId, uri: &str) {
        if let Some(client) = self.running(id).and_then(|server| server.client.as_ref()) {
            client.notify("textDocument/didSave", protocol::document(uri));
        }
    }

    /// Sends a request to a running server.
    pub fn request(
        &self,
        id: ServerId,
        method: &str,
        params: Value,
    ) -> Option<impl Future<Output = Result<Value, String>> + Send + 'static + use<>> {
        let client = self.running(id)?.client.as_ref()?;
        Some(client.request(method, params))
    }

    /// What a running server can do.
    pub fn capabilities(&self, id: ServerId) -> Option<&protocol::Capabilities> {
        self.running(id).map(|server| &server.caps)
    }

    /// What the status bar says about a server, and whether it is a failure.
    pub fn status(&self, id: ServerId) -> Option<(SharedString, bool)> {
        let server = self.servers.get(id)?;
        let name = server.spec.name;
        Some(match &server.state {
            State::Waiting | State::Starting => (format!("{name}: starting…").into(), false),
            State::Running => match server.progress.values().next() {
                Some(progress) => (format!("{name}: {progress}").into(), false),
                None => (name.into(), false),
            },
            State::Stopped => (format!("{name}: stopped").into(), false),
            State::Failed(why) => (format!("{name}: {why}").into(), true),
        })
    }

    /// Ends every server politely, for a folder that closes.
    pub fn stop_all(&mut self, cx: &mut Context<Self>) {
        for server in &mut self.servers {
            if let Some(client) = server.client.take() {
                client.shutdown();
            }
            server.state = State::Stopped;
            server.docs.clear();
        }
        cx.notify();
    }

    /// For quitting, when nothing can wait: every server and what it started get SIGTERM.
    pub fn terminate_all(&self) {
        for client in self
            .servers
            .iter()
            .filter_map(|server| server.client.as_ref())
        {
            client.terminate();
        }
    }

    fn start(&mut self, id: ServerId, cx: &mut Context<Self>) {
        let Some(path) = self.path.clone() else {
            return;
        };
        let settings = cx.global::<Settings>();
        let server = &mut self.servers[id];
        // The first candidate installed: a `[servers]` command as written, else the
        // project's npm install or `PATH`.
        let found = server.candidates.iter().find_map(|spec| {
            let (program, args) = match settings.servers.get(spec.name) {
                Some(command) => (
                    find_program(command.first()?, &path)?,
                    command.iter().skip(1).cloned().collect(),
                ),
                None => (
                    find_server(spec, &server.root, &path)?,
                    spec.args
                        .iter()
                        .map(|arg| (*arg).to_owned())
                        .collect::<Vec<_>>(),
                ),
            };
            Some((*spec, program, args))
        });
        let Some((spec, program, args)) = found else {
            let first = server.candidates[0];
            server.state =
                State::Failed(format!("not found; install with `{}`", first.install).into());
            cx.notify();
            return;
        };
        server.spec = spec;
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let (client, mut events) = match Client::start(&program, &args, &server.root, &path) {
            Ok(started) => started,
            Err(err) => {
                server.state = State::Failed(format!("{}: {err}", program.display()).into());
                cx.notify();
                return;
            }
        };
        let client = Arc::new(client);
        server.client = Some(Arc::clone(&client));
        server.state = State::Starting;
        server.events_task = Some(cx.spawn(async move |this, cx| {
            while let Some(event) = events.next().await {
                if this
                    .update(cx, |this, cx| this.on_event(id, event, cx))
                    .is_err()
                {
                    break;
                }
            }
        }));
        let initialize = client.request(
            "initialize",
            protocol::initialize(&server.root, initialization_options(spec.name)),
        );
        cx.spawn(async move |this, cx| {
            let result = initialize.await;
            let _ = this.update(cx, |this, cx| this.initialized(id, result, cx));
        })
        .detach();
        cx.notify();
    }

    fn initialized(&mut self, id: ServerId, result: Result<Value, String>, cx: &mut Context<Self>) {
        let server = &mut self.servers[id];
        let Some(client) = server.client.clone() else {
            return;
        };
        match result {
            Ok(value) => {
                server.caps = protocol::capabilities(&value);
                client.notify("initialized", json!({}));
                server.state = State::Running;
                for (editor, _) in &server.docs {
                    let Some(editor) = editor.upgrade() else {
                        continue;
                    };
                    if let Some((uri, language, version, text)) = editor.read(cx).lsp_document() {
                        client.notify(
                            "textDocument/didOpen",
                            protocol::did_open(&uri, language, version, &text),
                        );
                    }
                }
            }
            Err(err) => server.state = State::Failed(err.into()),
        }
        self.pull_all(id, cx);
        cx.notify();
    }

    /// Has every document a server serves ask it for problems, once this update is over:
    /// asking reads the server list, which is in use until then.
    fn pull_all(&self, id: ServerId, cx: &mut Context<Self>) {
        let editors: Vec<WeakEntity<Editor>> = self.servers[id]
            .docs
            .iter()
            .map(|(editor, _)| editor.clone())
            .collect();
        cx.spawn(async move |_, cx| {
            for editor in editors {
                let _ = editor.update(cx, Editor::pull_diagnostics);
            }
        })
        .detach();
    }

    fn on_event(&mut self, id: ServerId, event: Event, cx: &mut Context<Self>) {
        let server = &mut self.servers[id];
        match event {
            Event::Notification { method, params } => match method.as_str() {
                "textDocument/publishDiagnostics" => {
                    let Some((uri, list)) = protocol::diagnostics(&params) else {
                        return;
                    };
                    let version = params["version"].as_i64();
                    for (editor, open) in &server.docs {
                        if *open == uri {
                            let list = list.clone();
                            let _ = editor.update(cx, |editor, cx| {
                                editor.set_diagnostics(list, version, cx);
                            });
                        }
                    }
                }
                "$/progress" => {
                    let token = match &params["token"] {
                        Value::String(token) => token.clone(),
                        token => token.to_string(),
                    };
                    let value = &params["value"];
                    if value["kind"] == "end" {
                        server.progress.remove(&token);
                    } else {
                        let title = value["title"]
                            .as_str()
                            .map(str::to_owned)
                            .or_else(|| server.progress.get(&token).cloned())
                            .unwrap_or_default();
                        let title = title.split(" (").next().unwrap_or_default().to_owned();
                        let detail = match value["percentage"].as_u64() {
                            Some(percent) => format!("{title} {percent}%"),
                            None => title,
                        };
                        server.progress.insert(token, detail);
                    }
                    cx.notify();
                }
                "window/showMessage" if params["type"].as_u64().is_some_and(|kind| kind <= 2) => {
                    if let Some(message) = params["message"].as_str() {
                        cx.emit(LspEvent::Message(
                            format!("{}: {message}", server.spec.name).into(),
                        ));
                    }
                }
                "workspace/diagnostic/refresh" => self.pull_all(id, cx),
                "window/logMessage" => {
                    log::debug!("{}: {}", server.spec.name, params["message"]);
                }
                _ => {}
            },
            Event::Exited { status, stderr } => {
                server.client = None;
                server.progress.clear();
                if server.state != State::Stopped {
                    let last = stderr.lines().last().unwrap_or_default().trim();
                    let mut why = match status {
                        Some(code) => format!("exited ({code})"),
                        None => "was killed".to_owned(),
                    };
                    if !last.is_empty() {
                        why.push_str(": ");
                        why.push_str(last);
                    }
                    // One that never got going is most likely not installed (a rustup proxy
                    // without its component, a broken npm link). The hint goes first: the
                    // status bar cuts long text off at its end.
                    if server.state == State::Starting {
                        why = format!(
                            "didn't start, install with `{}` ({why})",
                            server.spec.install
                        );
                    }
                    server.state = State::Failed(why.into());
                }
                for (editor, _) in &server.docs {
                    let _ = editor.update(cx, |editor, cx| {
                        editor.set_diagnostics(Vec::new(), None, cx);
                    });
                }
                cx.notify();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_should_be_the_folder_else_the_nearest_marker() {
        let dir = std::env::temp_dir().join(format!("kiled-root-{}", std::process::id()));
        let crate_dir = dir.join("tools/crate");
        std::fs::create_dir_all(crate_dir.join("src")).unwrap();
        std::fs::write(crate_dir.join("Cargo.toml"), "").unwrap();
        let file = crate_dir.join("src/main.rs");
        assert_eq!(
            (
                root_for(&file, Some(&dir), &["Cargo.toml"]),
                root_for(&file, None, &["Cargo.toml"]),
                root_for(&file, None, &[]),
            ),
            (dir.clone(), crate_dir.clone(), crate_dir.join("src"))
        );
    }
}
