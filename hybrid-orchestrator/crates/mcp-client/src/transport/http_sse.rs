//! HTTP/SSE MCP transport (architecture.md Section 5.2, P3.1).
//!
//! Connects to an already-running MCP server over HTTP. Client -> server
//! JSON-RPC calls are sent as HTTP POSTs of the JSON-RPC request body; the
//! server replies either with a single JSON object (a JSON-RPC response) or
//! with a `text/event-stream` whose `data:` frames carry the JSON-RPC response
//! (and, optionally, interleaved notifications). Server -> client streaming uses
//! Server-Sent Events.
//!
//! This REUSES the `providers` HTTP/SSE approach: it installs the `ring` rustls
//! crypto provider before building any reqwest client (see [`crate::crypto`]),
//! builds a reqwest client with the rustls-tls stack, and decodes SSE frames
//! with a buffer-and-split decoder analogous to
//! `providers::capability::sse_event_stream` (see [`decode_sse_data_frames`]).
//! The configured headers are applied to every request.
//!
//! NOTE (Phase 3 scope): the session layer (P3.2) drives connect/handshake/
//! list/invoke over the [`Transport`] trait. The stdio transport is exercised
//! end to end by the bundled mock server; the HTTP/SSE transport is validated in
//! CI against a mocked HTTP server (mirroring how `providers` tests its client),
//! since the offline sandbox cannot reach a network.
//!
//! ## KNOWN LIMITATION: buffered, request-scoped SSE (not true server-push)
//!
//! [`HttpSseTransport::post`] awaits [`reqwest::Response::text`], which BUFFERS
//! the entire HTTP response body before [`decode_sse_data_frames`] splits it
//! into `data:` frames. As a result SSE is consumed per request/response rather
//! than as a persistent server -> client stream: a POST's response is decoded in
//! full once the body completes, and any notifications interleaved in that body
//! are surfaced then (not incrementally as they arrive). This satisfies the
//! request/response MCP methods (`initialize`, `tools/list`, `tools/call`) but
//! does NOT deliver the long-lived server-push streaming described in
//! architecture.md Section 5.2.
//!
//! This is a deliberate, scoped choice: a streamed decoder (holding a
//! [`reqwest::Response`] byte stream open and feeding an incremental SSE frame
//! parser) is a larger rewrite that cannot be exercised in the offline sandbox
//! (crates.io is unreachable and there is no live server), so it carries real
//! regression risk with no local verification. The stdio transport - which IS
//! exercised end to end - already provides a fully async notification path, and
//! nothing in the current lifecycle consumes server-push notifications. When a
//! future phase needs live HTTP server-push, replace `post()`'s buffering with a
//! streamed body reader and an incremental frame parser; the frame-splitting
//! logic in [`decode_sse_data_frames`] can be reused per chunk.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::{mpsc, Mutex};

use super::jsonrpc::{JsonRpcMessage, JsonRpcNotification, JsonRpcRequest, JsonRpcResponse};
use super::{Transport, NOTIFICATION_BUFFER};
use crate::error::{JsonRpcErrorPayload, TransportError};

/// A JSON-RPC transport over HTTP with SSE for server -> client streaming.
pub struct HttpSseTransport {
    url: String,
    headers: Vec<(String, String)>,
    http: reqwest::Client,
    next_id: AtomicU64,
    /// Server -> client notifications observed on POST SSE responses. BOUNDED
    /// with a drop-on-full policy (see [`NOTIFICATION_BUFFER`]) so a chatty
    /// server whose notifications are never drained cannot grow memory without
    /// bound.
    notif_rx: Mutex<mpsc::Receiver<JsonRpcNotification>>,
    notif_tx: mpsc::Sender<JsonRpcNotification>,
}

impl HttpSseTransport {
    /// Build a transport targeting `url` with `headers` applied to every
    /// request. Installs the `ring` rustls crypto provider first (idempotent),
    /// mirroring `providers`.
    pub fn connect(url: impl Into<String>, headers: Vec<(String, String)>) -> Self {
        // rustls 0.23 (via reqwest's `rustls-tls`) needs an unambiguous default
        // crypto provider installed before the client is used; install `ring`
        // once here so the reqwest connector's TLS init succeeds.
        crate::crypto::ensure_crypto_provider();
        let (notif_tx, notif_rx) = mpsc::channel(NOTIFICATION_BUFFER);
        HttpSseTransport {
            url: url.into(),
            headers,
            http: reqwest::Client::new(),
            next_id: AtomicU64::new(1),
            notif_rx: Mutex::new(notif_rx),
            notif_tx,
        }
    }

    fn apply_headers(&self, mut builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        for (name, value) in &self.headers {
            builder = builder.header(name, value);
        }
        builder
    }

    /// POST a JSON-RPC body and return the response text plus whether the reply
    /// was an SSE stream (`text/event-stream`) or a single JSON object.
    async fn post(&self, body: &Value) -> Result<(bool, String), TransportError> {
        let builder = self
            .http
            .post(&self.url)
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .json(body);
        let builder = self.apply_headers(builder);

        let resp = builder
            .send()
            .await
            .map_err(|e| TransportError::Http(e.to_string()))?;

        let status = resp.status();
        let is_sse = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|ct| ct.contains("text/event-stream"))
            .unwrap_or(false);

        // KNOWN LIMITATION (see the module-level docs): this BUFFERS the whole
        // body before SSE decoding, so streaming is request-scoped rather than a
        // persistent server-push stream. Acceptable for request/response MCP; a
        // streamed decoder is deferred (a risky rewrite unverifiable offline).
        let text = resp
            .text()
            .await
            .map_err(|e| TransportError::Http(e.to_string()))?;

        if !status.is_success() {
            return Err(TransportError::Http(format!("HTTP {status}: {text}")));
        }
        Ok((is_sse, text))
    }
}

#[async_trait]
impl Transport for HttpSseTransport {
    async fn request(&self, method: &str, params: Option<Value>) -> Result<Value, TransportError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let req = JsonRpcRequest::new(id, method, params);
        let body =
            serde_json::to_value(&req).map_err(|e| TransportError::Protocol(e.to_string()))?;

        let (is_sse, text) = self.post(&body).await?;

        // Collect JSON-RPC frames from either a single JSON body or the SSE
        // `data:` frames, routing notifications to the channel and returning the
        // response correlated to `id`.
        let frames = if is_sse {
            decode_sse_data_frames(&text)
        } else {
            vec![text]
        };

        let mut response: Option<JsonRpcResponse> = None;
        for frame in frames {
            let trimmed = frame.trim();
            if trimmed.is_empty() || trimmed == "[DONE]" {
                continue;
            }
            match serde_json::from_str::<JsonRpcMessage>(trimmed) {
                Ok(JsonRpcMessage::Response(resp)) if resp.id == id => response = Some(resp),
                Ok(JsonRpcMessage::Response(_)) => {
                    // A response to a different id; ignore (single-flight POST).
                }
                Ok(JsonRpcMessage::Notification(note)) => {
                    // Drop on full / closed; never block or grow unbounded.
                    let _ = self.notif_tx.try_send(note);
                }
                Err(e) => return Err(TransportError::Protocol(e.to_string())),
            }
        }

        match response {
            Some(resp) => match (resp.result, resp.error) {
                (_, Some(err)) => Err(TransportError::Rpc(JsonRpcErrorPayload {
                    code: err.code,
                    message: err.message,
                })),
                (Some(value), None) => Ok(value),
                (None, None) => Ok(Value::Null),
            },
            None => Err(TransportError::Protocol(format!(
                "no JSON-RPC response for request id {id}"
            ))),
        }
    }

    async fn notify(&self, method: &str, params: Option<Value>) -> Result<(), TransportError> {
        let note = JsonRpcNotification::new(method, params);
        let body =
            serde_json::to_value(&note).map_err(|e| TransportError::Protocol(e.to_string()))?;
        // A notification expects no correlated response; POST and discard the
        // body (which may carry interleaved server notifications).
        let (is_sse, text) = self.post(&body).await?;
        if is_sse {
            for frame in decode_sse_data_frames(&text) {
                let trimmed = frame.trim();
                if trimmed.is_empty() || trimmed == "[DONE]" {
                    continue;
                }
                if let Ok(JsonRpcMessage::Notification(note)) =
                    serde_json::from_str::<JsonRpcMessage>(trimmed)
                {
                    // Drop on full / closed; never block or grow unbounded.
                    let _ = self.notif_tx.try_send(note);
                }
            }
        }
        Ok(())
    }

    async fn next_notification(&self) -> Option<JsonRpcNotification> {
        self.notif_rx.lock().await.recv().await
    }

    async fn shutdown(&self) {
        // Nothing to kill for HTTP; dropping the client closes idle connections.
        // Provided so the trait is uniform across transports.
    }
}

/// Extract the concatenated `data:` payloads from an SSE body, one `String` per
/// frame (frames are separated by a blank line). This mirrors the frame/`data:`
/// handling in `providers::capability::sse_event_stream`, but operates on a
/// fully-buffered body string (the POST response), so it needs no async stream
/// machinery. Comment (`:`), `event:`, and `id:` lines are ignored.
fn decode_sse_data_frames(body: &str) -> Vec<String> {
    // Normalize CRLF to LF so `\r\n\r\n` and `\n\n` boundaries both split.
    let normalized = body.replace("\r\n", "\n");
    let mut frames = Vec::new();
    for raw_frame in normalized.split("\n\n") {
        let mut data = String::new();
        for line in raw_frame.lines() {
            if let Some(rest) = line.strip_prefix("data:") {
                let rest = rest.strip_prefix(' ').unwrap_or(rest);
                if !data.is_empty() {
                    data.push('\n');
                }
                data.push_str(rest);
            }
        }
        if !data.is_empty() {
            frames.push(data);
        }
    }
    frames
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_sse_extracts_data_payloads() {
        let body = concat!(
            ": keep-alive\n\n",
            "event: message\n",
            "data: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"ok\":true}}\n\n",
            "data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/x\"}\n\n",
        );
        let frames = decode_sse_data_frames(body);
        assert_eq!(frames.len(), 2);
        assert!(frames[0].contains("\"id\":1"));
        assert!(frames[1].contains("notifications/x"));
    }

    #[test]
    fn decode_sse_handles_crlf_and_multiline_data() {
        let body = "data: {\"a\":\r\ndata: 1}\r\n\r\n";
        let frames = decode_sse_data_frames(body);
        assert_eq!(frames.len(), 1);
        // The two data lines are joined with a newline (SSE multi-line data).
        assert_eq!(frames[0], "{\"a\":\n1}");
    }
}
