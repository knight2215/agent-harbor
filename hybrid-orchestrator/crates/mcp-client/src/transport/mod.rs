//! MCP transports (architecture.md Section 5.2) and the [`Transport`] trait both
//! implementations satisfy.
//!
//! Two transports are supported, matching the MCP standard:
//!
//! - [`stdio::StdioTransport`]: spawns the server as a child process and speaks
//!   newline-delimited JSON-RPC over its stdin/stdout.
//! - [`http_sse::HttpSseTransport`]: POSTs JSON-RPC requests over HTTP and
//!   consumes Server-Sent Events for server -> client streaming.
//!
//! Both expose the same async [`Transport`] surface so [`crate::session`] is
//! transport-agnostic: send a correlated JSON-RPC request and await its
//! response, or drain server-initiated notifications.

pub mod http_sse;
pub mod jsonrpc;
pub mod stdio;

use async_trait::async_trait;
use serde_json::Value;

use crate::error::TransportError;
use jsonrpc::JsonRpcNotification;

/// Capacity of the bounded server -> client notification channel used by both
/// transports.
///
/// Notifications are advisory (progress / log pushes). The lifecycle layer does
/// not currently consume them, and even once it does a burst can outpace the
/// consumer. A BOUNDED channel with a drop-newest-on-full policy (the transports
/// use `try_send`) guarantees a chatty long-lived server cannot grow client
/// memory without bound: once the buffer fills, further notifications are
/// dropped rather than queued forever. This is the intended behavior for
/// best-effort notifications; correlated request/response traffic is unaffected
/// because it uses a separate per-request `oneshot`.
pub const NOTIFICATION_BUFFER: usize = 256;

/// The async transport contract shared by the stdio and HTTP/SSE transports.
///
/// A transport owns the connection to a single MCP server. [`request`](Transport::request)
/// sends a JSON-RPC request and resolves to the correlated result value (or a
/// [`TransportError::Rpc`] if the server returned a JSON-RPC error object).
/// [`next_notification`](Transport::next_notification) yields the next
/// server-initiated notification, or `None` once the transport is closed.
#[async_trait]
pub trait Transport: Send + Sync {
    /// Send a JSON-RPC request for `method` with `params` and await the
    /// correlated response `result`. The transport assigns and correlates the
    /// request `id` internally.
    async fn request(&self, method: &str, params: Option<Value>) -> Result<Value, TransportError>;

    /// Send a fire-and-forget JSON-RPC notification (no reply expected), e.g.
    /// the `notifications/initialized` sent after the handshake.
    async fn notify(&self, method: &str, params: Option<Value>) -> Result<(), TransportError>;

    /// Await the next server -> client notification, or `None` when the transport
    /// is closed. Used by the lifecycle layer to observe server-pushed events.
    async fn next_notification(&self) -> Option<JsonRpcNotification>;

    /// Gracefully shut the transport down (kill the child for stdio; drop the
    /// HTTP connection for HTTP/SSE). Idempotent.
    async fn shutdown(&self);
}
