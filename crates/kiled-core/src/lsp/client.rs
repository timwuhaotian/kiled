//! A running language server: its process, the threads that carry its messages, and the
//! requests waiting for answers (DESIGN.md §6.3).
//!
//! The server runs in a process group of its own, so whatever it starts (tsserver, a
//! proc-macro server) ends with it. One thread owns the child: when asked to stop, or when the
//! server closes its output, it gives the server [`GRACE`] to exit, kills its group, and only
//! then reaps it. A pid isn't reused until it is reaped, so the group signalled is always ours.

use std::{
    collections::{HashMap, VecDeque},
    ffi::OsStr,
    io::{self, BufRead, BufReader},
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Child, ChildStderr, Command, Stdio},
    sync::{
        Arc, Mutex, MutexGuard, PoisonError, TryLockError,
        atomic::{AtomicBool, AtomicI64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use futures::channel::{mpsc as async_mpsc, oneshot};
use serde_json::{Value, json};

use super::{
    protocol,
    transport::{read_message, write_message},
};
use crate::sys;

/// How long a server has to exit on its own once told to (or once its output closed).
const GRACE: Duration = Duration::from_secs(1);
/// How long `shutdown` may go unanswered before `exit` is sent anyway.
const SHUTDOWN_WAIT: Duration = Duration::from_secs(1);
/// Lines of the server's stderr kept to explain an early exit.
const STDERR_TAIL: usize = 12;

/// What a server sends besides answers.
#[derive(Debug)]
pub enum Event {
    /// A notification: diagnostics, progress, a message for the user.
    Notification {
        /// Its method.
        method: String,
        /// Its params.
        params: Value,
    },
    /// The server is gone and reaped. `status` is its exit code if it exited by itself;
    /// `stderr` holds its last lines.
    Exited {
        /// Exit code.
        status: Option<i32>,
        /// The end of what it wrote to stderr.
        stderr: String,
    },
}

type Reply = Box<dyn FnOnce(Result<Value, String>) + Send>;

struct Shared {
    /// Requests waiting for answers; `None` once the server's output closed, when no more
    /// will come.
    pending: Mutex<Option<HashMap<i64, Reply>>>,
    /// Set, under the lock, once the server is reaped: from then on its pid isn't ours.
    reaped: Mutex<bool>,
    pid: u32,
    root: PathBuf,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A language server we started.
pub struct Client {
    outgoing: mpsc::Sender<String>,
    shared: Arc<Shared>,
    next_id: AtomicI64,
    stop: mpsc::Sender<()>,
    stopping: AtomicBool,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("pid", &self.shared.pid)
            .finish_non_exhaustive()
    }
}

impl Client {
    /// Starts `program` in `root` with `path` as its `PATH`. Returns the client and the
    /// stream of what the server sends besides answers; the stream ends after
    /// [`Event::Exited`].
    ///
    /// # Errors
    /// If the program can't be started.
    pub fn start(
        program: &Path,
        args: &[&str],
        root: &Path,
        path: &OsStr,
    ) -> io::Result<(Self, async_mpsc::UnboundedReceiver<Event>)> {
        let mut child = Command::new(program)
            .args(args)
            .current_dir(root)
            .env("PATH", path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()?;
        let missing = || io::Error::other("no pipe to the server");
        let stdin = child.stdin.take().ok_or_else(missing)?;
        let stdout = child.stdout.take().ok_or_else(missing)?;
        let stderr = child.stderr.take().ok_or_else(missing)?;
        let shared = Arc::new(Shared {
            pending: Mutex::new(Some(HashMap::new())),
            reaped: Mutex::new(false),
            pid: child.id(),
            root: root.to_owned(),
        });
        let (events, events_rx) = async_mpsc::unbounded();
        let (outgoing, outgoing_rx) = mpsc::channel::<String>();
        let (stop, stop_rx) = mpsc::channel::<()>();
        let tail = Arc::new(Mutex::new(VecDeque::new()));
        let name = program.file_name().map_or_else(
            || "server".into(),
            |name| name.to_string_lossy().into_owned(),
        );

        thread::Builder::new()
            .name(format!("lsp-write {name}"))
            .spawn(move || {
                let mut stdin = stdin;
                for body in outgoing_rx {
                    if write_message(&mut stdin, &body).is_err() {
                        break;
                    }
                }
            })?;

        keep_stderr(&name, stderr, Arc::clone(&tail))?;

        let reader_shared = Arc::clone(&shared);
        let reader_out = outgoing.clone();
        let reader_events = events.clone();
        let reader_stop = stop.clone();
        thread::Builder::new()
            .name(format!("lsp-read {name}"))
            .spawn(move || {
                let mut reader = BufReader::new(stdout);
                while let Ok(Some(body)) = read_message(&mut reader) {
                    dispatch(&body, &reader_shared, &reader_out, &reader_events);
                }
                // Whatever is still waiting will never be answered.
                let waiting: Vec<Reply> = lock(&reader_shared.pending)
                    .take()
                    .into_iter()
                    .flat_map(HashMap::into_values)
                    .collect();
                for reply in waiting {
                    reply(Err("the language server stopped".to_owned()));
                }
                let _ = reader_stop.send(());
            })?;

        own(&name, child, Arc::clone(&shared), stop_rx, tail, events)?;

        Ok((
            Self {
                outgoing,
                shared,
                next_id: AtomicI64::new(1),
                stop,
                stopping: AtomicBool::new(false),
            },
            events_rx,
        ))
    }

    /// The server's process id, which is also its process group's.
    pub fn pid(&self) -> u32 {
        self.shared.pid
    }

    /// The server's output has closed: nothing more will be answered.
    pub fn is_closed(&self) -> bool {
        lock(&self.shared.pending).is_none()
    }

    fn send(&self, message: &Value) {
        let _ = self.outgoing.send(message.to_string());
    }

    /// Sends a notification.
    pub fn notify(&self, method: &str, params: Value) {
        self.send(&message(None, method, params));
    }

    fn call(&self, method: &str, params: Value, reply: Reply) {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        if let Some(pending) = lock(&self.shared.pending).as_mut() {
            pending.insert(id, reply);
        } else {
            reply(Err("the language server stopped".to_owned()));
            return;
        }
        self.send(&message(Some(id), method, params));
    }

    /// Sends a request; the future resolves to its result.
    ///
    /// # Errors
    /// The future fails with the server's error message, or if the server stops first.
    pub fn request(
        &self,
        method: &str,
        params: Value,
    ) -> impl Future<Output = Result<Value, String>> + Send + 'static + use<> {
        let (tx, rx) = oneshot::channel();
        self.call(
            method,
            params,
            Box::new(move |result| {
                let _ = tx.send(result);
            }),
        );
        async move {
            rx.await
                .unwrap_or_else(|_| Err("the language server stopped".to_owned()))
        }
    }

    /// Ends the server politely: `shutdown`, then `exit`, then (after [`GRACE`]) its group is
    /// killed and it is reaped. Returns at once.
    pub fn shutdown(&self) {
        if self.stopping.swap(true, Ordering::SeqCst) {
            return;
        }
        let (answered, wait) = mpsc::channel();
        self.call(
            "shutdown",
            Value::Null,
            Box::new(move |_| {
                let _ = answered.send(());
            }),
        );
        let outgoing = self.outgoing.clone();
        let stop = self.stop.clone();
        let spawned = thread::Builder::new()
            .name("lsp-shutdown".into())
            .spawn(move || {
                let _ = wait.recv_timeout(SHUTDOWN_WAIT);
                let _ = outgoing.send(json!({"jsonrpc": "2.0", "method": "exit"}).to_string());
                let _ = stop.send(());
            });
        if spawned.is_err() {
            let _ = self.stop.send(());
        }
    }

    /// For quitting, when nothing can wait: the server and everything it started get
    /// SIGTERM now.
    pub fn terminate(&self) {
        // Held only while the server is being killed and reaped, which a server that won't
        // die can make last: then it is ending already, and quitting doesn't wait for it.
        let reaped = match self.shared.reaped.try_lock() {
            Ok(reaped) => reaped,
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(TryLockError::WouldBlock) => return,
        };
        if !*reaped {
            let _ = sys::kill_group(self.shared.pid, libc::SIGTERM);
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Logs what the server writes to stderr, and keeps its last lines to explain an exit.
fn keep_stderr(
    name: &str,
    stderr: ChildStderr,
    tail: Arc<Mutex<VecDeque<String>>>,
) -> io::Result<()> {
    let label = name.to_owned();
    thread::Builder::new()
        .name(format!("lsp-stderr {name}"))
        .spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                log::debug!("{label}: {line}");
                let mut tail = lock(&tail);
                if tail.len() == STDERR_TAIL {
                    tail.pop_front();
                }
                tail.push_back(line);
            }
        })
        .map(drop)
}

/// The thread that owns the child: once asked to stop (or the output closed, or the client
/// is gone) it gives the server [`GRACE`] to exit, kills its group, and reaps it.
fn own(
    name: &str,
    mut child: Child,
    shared: Arc<Shared>,
    stop: mpsc::Receiver<()>,
    tail: Arc<Mutex<VecDeque<String>>>,
    events: async_mpsc::UnboundedSender<Event>,
) -> io::Result<()> {
    thread::Builder::new()
        .name(format!("lsp-process {name}"))
        .spawn(move || {
            let _ = stop.recv();
            let deadline = Instant::now() + GRACE;
            while !sys::exited(shared.pid) && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(10));
            }
            let status = {
                let mut reaped = lock(&shared.reaped);
                // The leader isn't reaped yet, so this group is still the server's, and its
                // pid too: killed on its own as well, in case it left the group.
                let _ = sys::kill_group(shared.pid, libc::SIGKILL);
                let _ = child.kill();
                let status = child.wait().ok().and_then(|status| status.code());
                *reaped = true;
                status
            };
            let stderr = lock(&tail).iter().cloned().collect::<Vec<_>>().join("\n");
            let _ = events.unbounded_send(Event::Exited { status, stderr });
        })
        .map(drop)
}

/// A request (with `id`) or a notification.
fn message(id: Option<i64>, method: &str, params: Value) -> Value {
    let mut message = serde_json::Map::new();
    message.insert("jsonrpc".into(), "2.0".into());
    if let Some(id) = id {
        message.insert("id".into(), id.into());
    }
    message.insert("method".into(), method.into());
    message.insert("params".into(), params);
    Value::Object(message)
}

/// Routes one message: answers to their requests, notifications to the events, and the
/// server's own requests answered here.
fn dispatch(
    body: &[u8],
    shared: &Shared,
    outgoing: &mpsc::Sender<String>,
    events: &async_mpsc::UnboundedSender<Event>,
) {
    let message: Value = match serde_json::from_slice(body) {
        Ok(message) => message,
        Err(err) => {
            log::warn!("unreadable message from a language server: {err}");
            return;
        }
    };
    let method = message.get("method").and_then(Value::as_str);
    match (method, message.get("id")) {
        (Some(method), Some(id)) => {
            // Answered here, and passed on: the app asks for its documents' problems again.
            if method == "workspace/diagnostic/refresh" {
                let _ = events.unbounded_send(Event::Notification {
                    method: method.to_owned(),
                    params: Value::Null,
                });
            }
            let response = match answer(method, message.get("params"), &shared.root) {
                Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
                Err(err) => json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": {"code": -32601, "message": err},
                }),
            };
            let _ = outgoing.send(response.to_string());
        }
        (Some(method), None) => {
            let _ = events.unbounded_send(Event::Notification {
                method: method.to_owned(),
                params: message.get("params").cloned().unwrap_or(Value::Null),
            });
        }
        (None, Some(id)) => {
            let reply = id
                .as_i64()
                .and_then(|id| lock(&shared.pending).as_mut()?.remove(&id));
            if let Some(reply) = reply {
                let result = match message.get("error") {
                    Some(error) => Err(error
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("the server failed")
                        .to_owned()),
                    None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
                };
                reply(result);
            }
        }
        (None, None) => {}
    }
}

/// What Kiled answers the server's own requests with: no settings of its own, one
/// workspace folder, and yes to whatever it wants to register or report.
fn answer(method: &str, params: Option<&Value>, root: &Path) -> Result<Value, String> {
    match method {
        "workspace/configuration" => {
            let items = params
                .and_then(|params| params.get("items"))
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            Ok(Value::Array(vec![Value::Null; items]))
        }
        "workspace/workspaceFolders" => Ok(protocol::workspace_folders(root)),
        "client/registerCapability"
        | "client/unregisterCapability"
        | "window/workDoneProgress/create"
        | "window/showMessageRequest"
        | "workspace/diagnostic/refresh" => Ok(Value::Null),
        _ => Err(format!("{method} isn't supported")),
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use futures::{StreamExt, executor::block_on};

    use super::*;

    /// A shell script speaking just enough LSP: it answers `initialize`, publishes one
    /// notification, starts a child that would outlive it, and exits on `exit`.
    const FAKE_SERVER: &str = r#"
sleep 300 &
echo $! > "$1"
reply() { printf 'Content-Length: %d\r\n\r\n%s' "${#1}" "$1"; }
while IFS= read -r line; do
  case "$line" in
    Content-Length:*) len=${line#Content-Length: }; len=${len%$'\r'} ;;
    $'\r') body=$(dd bs=1 count="$len" 2>/dev/null)
      case "$body" in
        *'"initialize"'*) reply '{"jsonrpc":"2.0","id":1,"result":{"capabilities":{}}}'
                          reply '{"jsonrpc":"2.0","method":"hello","params":{}}' ;;
        *'"shutdown"'*) reply '{"jsonrpc":"2.0","id":2,"result":null}' ;;
        *'"exit"'*) exit 0 ;;
      esac ;;
  esac
done
"#;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kiled-lsp-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn alive(pid: i32) -> bool {
        Command::new("/bin/kill")
            .args(["-0", &pid.to_string()])
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    fn start_fake(dir: &Path) -> (Client, async_mpsc::UnboundedReceiver<Event>, PathBuf) {
        let script = dir.join("server.sh");
        std::fs::write(&script, FAKE_SERVER).unwrap();
        let grandchild = dir.join("grandchild.pid");
        let (client, events) = Client::start(
            Path::new("/bin/bash"),
            &[script.to_str().unwrap(), grandchild.to_str().unwrap()],
            dir,
            &OsString::from("/bin:/usr/bin"),
        )
        .unwrap();
        (client, events, grandchild)
    }

    #[test]
    fn requests_should_be_answered_and_notifications_delivered() {
        let dir = scratch("roundtrip");
        let (client, mut events, _) = start_fake(&dir);
        let result = block_on(client.request("initialize", json!({})));
        let event = block_on(events.next());
        client.shutdown();
        assert_eq!(result, Ok(json!({"capabilities": {}})));
        assert!(matches!(event, Some(Event::Notification { method, .. }) if method == "hello"));
    }

    #[test]
    fn shutdown_should_reap_the_server_and_end_what_it_started() {
        let dir = scratch("shutdown");
        let (client, mut events, grandchild) = start_fake(&dir);
        let _ = block_on(client.request("initialize", json!({})));
        let leftover: i32 = std::fs::read_to_string(&grandchild)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        client.shutdown();
        let exited = block_on(async {
            while let Some(event) = events.next().await {
                if let Event::Exited { status, .. } = event {
                    return Some(status);
                }
            }
            None
        });
        // The server exited by itself on `exit`; its background child was killed with
        // the group.
        assert_eq!((exited, alive(leftover)), (Some(Some(0)), false));
    }

    #[test]
    fn a_server_that_left_its_group_should_still_be_killed_and_quitting_not_wait() {
        let dir = scratch("escaped");
        // Joins its parent's process group, out of reach of a kill of its own, and stays.
        let (client, mut events) = Client::start(
            Path::new("/usr/bin/perl"),
            &["-e", "setpgrp(0, getpgrp(getppid())) or die; sleep 30"],
            &dir,
            &OsString::from("/bin:/usr/bin"),
        )
        .unwrap();
        let pid = i32::try_from(client.pid()).unwrap();
        thread::sleep(Duration::from_millis(200));
        client.shutdown();
        let (done, exited) = mpsc::channel();
        thread::spawn(move || {
            let event = block_on(async {
                while let Some(event) = events.next().await {
                    if let Event::Exited { status, .. } = event {
                        return Some(status);
                    }
                }
                None
            });
            let _ = done.send(event);
        });
        // Once it is being killed and reaped, quitting must not wait for it.
        thread::sleep(SHUTDOWN_WAIT + GRACE + Duration::from_millis(500));
        let asked = Instant::now();
        client.terminate();
        let quit_in = asked.elapsed();
        let exited = exited.recv_timeout(Duration::from_secs(10));
        assert_eq!(
            (quit_in < Duration::from_millis(100), exited, alive(pid)),
            (true, Ok(Some(None)), false)
        );
    }

    #[test]
    fn a_server_that_dies_should_fail_what_is_waiting() {
        let dir = scratch("dies");
        let (client, _events) = Client::start(
            Path::new("/bin/sh"),
            &["-c", "exit 3"],
            &dir,
            &OsString::from("/bin:/usr/bin"),
        )
        .unwrap();
        let result = block_on(client.request("initialize", json!({})));
        assert!(result.is_err());
    }
}
