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
use super::Transport;
use crate::error::{JsonRpcErrorPayload, TransportError};

/// The table of in-flight requests awaiting a correlated response, keyed by
/// JSON-RPC `id`.
type PendingMap = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, TransportError>>>>>;

/// A JSON-RPC transport over a spawned child process's stdin/stdout.
pub struct StdioTransport {
    /// Monotonic request-id source.
    next_id: AtomicU64,
    /// The child's stdin, guarded for exclusive line-writes.
    stdin: Mutex<ChildStdin>,
    /// In-flight requests awaiting responses.
    pending: PendingMap,
    /// Server -> client notifications drained by the lifecycle layer.
    notifications: Mutex<mpsc::UnboundedReceiver<JsonRpcNotification>>,
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
        for (key, value) in env {
            cmd.env(key, value);
        }
        // Do not leak the parent's own env selectively; we keep the inherited
        // env and layer the configured vars on top, matching typical MCP hosts.

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
        let (notif_tx, notif_rx) = mpsc::unbounded_channel();

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
                                // Receiver dropped => nobody is listening; stop.
                                if notif_tx.send(note).is_err() {
                                    break;
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
