//! stdio MCP transport (architecture.md Section 5.2, P3.1).
//!
//! Spawns the MCP server as a child process via [`tokio::process::Command`] and
//! speaks JSON-RPC 2.0 over its stdin/stdout using the common MCP stdio framing:
//! one JSON object per line (newline-delimited). A background task reads the
//! child's stdout line by line, decodes each line as a [`JsonRpcMessage`], and
//! either resolves the matching pending request (by `id`) or forwards a
//! server-initiated notification onto a channel.
//!
//! CROSS-PLATFORM: uses `tokio::process` exclusively (no Unix-only shell, no
//! `sh -c`), so it spawns identically on ubuntu-latest and windows-latest.
//! `tokio::process::Command` resolves the program name using the OS's normal
//! rules, honoring `.exe` on Windows. Child exit / broken pipe surface as a
//! [`TransportError::Closed`] on the affected request.
//!
//! CONTROLLED ENVIRONMENT (architecture.md Section 9.4): the child is spawned
//! from a CLEARED environment (`env_clear`), not the parent's full environment,
//! so no inherited secrets leak into an untrusted server. Only a minimal,
//! documented allowlist ([`MINIMAL_ENV_ALLOWLIST`]) needed for a child to launch
//! is re-introduced from the parent, and the caller-configured env is layered on
//! top.

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{mpsc, oneshot, Mutex};

use super::jsonrpc::{JsonRpcMessage, JsonRpcNotification, JsonRpcRequest};
use super::{Transport, NOTIFICATION_BUFFER};
use crate::error::{JsonRpcErrorPayload, TransportError};

/// The table of in-flight requests awaiting a correlated response, keyed by
/// JSON-RPC `id`.
type PendingMap = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, TransportError>>>>>;

/// Minimal allowlist of PARENT environment variables re-introduced into the
/// otherwise-cleared child environment (architecture.md Section 9.4). These are
/// the variables a child process commonly needs merely to LAUNCH; they are NOT
/// secrets. Everything else (provider API keys, tokens, etc.) is deliberately
/// withheld. Each entry is documented with WHY it is required:
///
/// - `PATH`: without it, a server whose `command` relies on `PATH` resolution
///   (or a helper it shells out to, e.g. `node`/`python`) cannot be found and
///   the spawn fails. Cross-platform (both Unix and Windows use `PATH`).
/// - `SYSTEMROOT` (Windows): many Windows binaries and the CRT/winsock startup
///   fail to initialize without `SystemRoot`; omitting it can break the child
///   before it runs a line of user code.
/// - `SYSTEMDRIVE` (Windows): some Windows tooling resolves paths relative to
///   `SystemDrive`; included alongside `SYSTEMROOT` for robust child startup.
///
/// The Windows-only names are harmless on Unix (they are simply absent from the
/// parent, so `var_os` returns `None` and nothing is added).
const MINIMAL_ENV_ALLOWLIST: &[&str] = &[
    "PATH",
    // Windows child-startup essentials (absent on Unix -> skipped).
    "SYSTEMROOT",
    "SYSTEMDRIVE",
];

/// A JSON-RPC transport over a spawned child process's stdin/stdout.
pub struct StdioTransport {
    /// Monotonic request-id source.
    next_id: AtomicU64,
    /// The child's stdin, guarded for exclusive line-writes.
    stdin: Mutex<ChildStdin>,
    /// In-flight requests awaiting responses.
    pending: PendingMap,
    /// Server -> client notifications, drained by [`next_notification`]. This
    /// is a BOUNDED channel with a drop-newest-on-full policy (see
    /// [`NOTIFICATION_BUFFER`]): a chatty long-lived server that emits progress/
    /// log notifications faster than they are drained (or with no consumer at
    /// all) cannot grow memory without bound - excess notifications are dropped.
    notifications: Mutex<mpsc::Receiver<JsonRpcNotification>>,
    /// The child handle, kept so [`shutdown`](Transport::shutdown) can kill it.
    child: Mutex<Child>,
}

impl StdioTransport {
    /// Spawn `command` with `args` and `env` and start speaking JSON-RPC over
    /// its stdio. Returns once the child is spawned and the reader task is
    /// running; it does NOT perform the MCP handshake (that is the session
    /// layer's job).
    pub async fn spawn(
        command: &str,
        args: &[String],
        env: &[(String, String)],
    ) -> Result<Self, TransportError> {
        let mut cmd = Command::new(command);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());

        // CONTROLLED ENVIRONMENT (architecture.md Section 9.4): stdio MCP servers
        // must run with "a controlled environment (explicit env, no inherited
        // secrets)". We therefore START FROM A CLEARED ENVIRONMENT rather than
        // inheriting the parent process's full environment, so provider API keys
        // and other secrets present in this process cannot leak into an untrusted
        // child. The ONLY parent values re-introduced are a minimal, explicitly
        // documented allowlist required for a child to launch at all on each
        // platform; the caller-configured `env` is then layered on top.
        cmd.env_clear();
        for key in MINIMAL_ENV_ALLOWLIST {
            if let Some(value) = std::env::var_os(key) {
                cmd.env(key, value);
            }
        }
        // Layer the caller-configured env on top of the controlled base. These
        // are explicit and intentional (they come from the MCP server config),
        // and they override an allowlisted value if the names collide.
        for (key, value) in env {
            cmd.env(key, value);
        }

        let mut child = cmd
            .spawn()
            .map_err(|e| TransportError::Spawn(e.to_string()))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| TransportError::Spawn("child stdin was not captured".to_string()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| TransportError::Spawn("child stdout was not captured".to_string()))?;

        let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));
        let (notif_tx, notif_rx) = mpsc::channel(NOTIFICATION_BUFFER);

        // Background reader: parse newline-delimited JSON-RPC frames and route
        // them to pending requests or the notification channel.
        let reader_pending = Arc::clone(&pending);
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            loop {
                match lines.next_line().await {
                    Ok(Some(line)) => {
                        let trimmed = line.trim();
                        if trimmed.is_empty() {
                            continue;
                        }
                        match serde_json::from_str::<JsonRpcMessage>(trimmed) {
                            Ok(JsonRpcMessage::Response(resp)) => {
                                if let Some(tx) = reader_pending.lock().await.remove(&resp.id) {
                                    let result = match (resp.result, resp.error) {
                                        (_, Some(err)) => {
                                            Err(TransportError::Rpc(JsonRpcErrorPayload {
                                                code: err.code,
                                                message: err.message,
                                            }))
                                        }
                                        (Some(value), None) => Ok(value),
                                        (None, None) => Ok(Value::Null),
                                    };
                                    // Receiver may have been dropped; ignore.
                                    let _ = tx.send(result);
                                }
                            }
                            Ok(JsonRpcMessage::Notification(note)) => {
                                // Bounded channel with a drop policy: never block
                                // the reader (that would stall correlated
                                // responses) and never grow memory without bound.
                                // On `Full`, drop this notification; on `Closed`
                                // (receiver gone => nobody is listening), stop.
                                match notif_tx.try_send(note) {
                                    Ok(()) => {}
                                    Err(mpsc::error::TrySendError::Full(_)) => {}
                                    Err(mpsc::error::TrySendError::Closed(_)) => break,
                                }
                            }
                            Err(_) => {
                                // Ignore unparseable lines (e.g. stray server
                                // logging that leaked onto stdout) rather than
                                // tearing down the whole transport.
                                continue;
                            }
                        }
                    }
                    // EOF: child closed stdout (exited or broke the pipe). Fail
                    // every pending request so awaiting callers do not hang.
                    Ok(None) => {
                        let mut guard = reader_pending.lock().await;
                        for (_, tx) in guard.drain() {
                            let _ = tx.send(Err(TransportError::Closed(
                                "child stdout closed".to_string(),
                            )));
                        }
                        break;
                    }
                    Err(e) => {
                        let mut guard = reader_pending.lock().await;
                        for (_, tx) in guard.drain() {
                            let _ = tx.send(Err(TransportError::Io(e.to_string())));
                        }
                        break;
                    }
                }
            }
        });

        Ok(StdioTransport {
            next_id: AtomicU64::new(1),
            stdin: Mutex::new(stdin),
            pending,
            notifications: Mutex::new(notif_rx),
            child: Mutex::new(child),
        })
    }

    /// Write one JSON value as a single `\n`-terminated line to the child.
    async fn write_line(&self, value: &Value) -> Result<(), TransportError> {
        let mut line =
            serde_json::to_string(value).map_err(|e| TransportError::Protocol(e.to_string()))?;
        line.push('\n');
        let mut stdin = self.stdin.lock().await;
        stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|e| TransportError::Closed(e.to_string()))?;
        stdin
            .flush()
            .await
            .map_err(|e| TransportError::Closed(e.to_string()))?;
        Ok(())
    }
}

#[async_trait]
impl Transport for StdioTransport {
    async fn request(&self, method: &str, params: Option<Value>) -> Result<Value, TransportError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let req = JsonRpcRequest::new(id, method, params);
        let value =
            serde_json::to_value(&req).map_err(|e| TransportError::Protocol(e.to_string()))?;

        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);

        if let Err(e) = self.write_line(&value).await {
            // Writing failed: drop the pending entry so it does not leak.
            self.pending.lock().await.remove(&id);
            return Err(e);
        }

        match rx.await {
            Ok(result) => result,
            // The reader task dropped the sender without responding (task ended).
            Err(_) => Err(TransportError::Closed(
                "transport reader stopped before responding".to_string(),
            )),
        }
    }

    async fn notify(&self, method: &str, params: Option<Value>) -> Result<(), TransportError> {
        let note = JsonRpcNotification::new(method, params);
        let value =
            serde_json::to_value(&note).map_err(|e| TransportError::Protocol(e.to_string()))?;
        self.write_line(&value).await
    }

    async fn next_notification(&self) -> Option<JsonRpcNotification> {
        self.notifications.lock().await.recv().await
    }

    async fn shutdown(&self) {
        // Best-effort graceful kill of the child; ignore errors (already exited).
        let mut child = self.child.lock().await;
        let _ = child.start_kill();
        let _ = child.wait().await;
    }
}
