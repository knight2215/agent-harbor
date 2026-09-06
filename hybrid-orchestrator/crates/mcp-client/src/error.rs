//! Error types for the MCP client (architecture.md Section 5.6).
//!
//! Two layers of error are distinguished:
//!
//! - [`TransportError`]: a low-level failure of a [`crate::transport::Transport`]
//!   (spawn failed, broken pipe, child exited, HTTP/SSE failure, malformed
//!   JSON-RPC framing, or a JSON-RPC error object returned by the server). These
//!   abort the operation that hit them.
//! - [`McpError`]: the crate-level error surfaced by [`crate::McpServerHandle`]
//!   lifecycle operations (connect / list-tools / invoke). It wraps a
//!   [`TransportError`] plus lifecycle-specific cases (handshake rejected,
//!   invoke timed out, output too large).
//!
//! Per Section 5.6, *invocation* failures (timeouts, oversized output,
//! server-returned tool errors) are NOT propagated as hard `Err` up the turn -
//! [`crate::McpServerHandle::invoke`] converts them into a structured
//! [`domain::ToolResult`] with `is_error = true` so the model can recover rather
//! than crashing the conversation. `McpError` is reserved for lifecycle/plumbing
//! failures where there is no tool result to return.

use thiserror::Error;

/// A JSON-RPC 2.0 error object returned by the server (`{code, message, data}`).
///
/// Derives `Debug` so tests can `{:?}`-format it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonRpcErrorPayload {
    pub code: i64,
    pub message: String,
}

/// A low-level transport failure.
#[derive(Debug, Error)]
pub enum TransportError {
    /// Spawning the child process (stdio) failed.
    #[error("failed to spawn MCP server process: {0}")]
    Spawn(String),
    /// The transport's read/write channel is closed (broken pipe, child exited,
    /// SSE stream ended, or the background reader task stopped).
    #[error("MCP transport closed: {0}")]
    Closed(String),
    /// An underlying I/O error.
    #[error("MCP transport I/O error: {0}")]
    Io(String),
    /// The HTTP/SSE transport failed at the HTTP layer.
    #[error("MCP HTTP transport error: {0}")]
    Http(String),
    /// A frame could not be encoded/decoded as JSON-RPC.
    #[error("malformed JSON-RPC message: {0}")]
    Protocol(String),
    /// The server returned a JSON-RPC error object for a request.
    #[error("JSON-RPC error {}: {}", .0.code, .0.message)]
    Rpc(JsonRpcErrorPayload),
}

/// The crate-level error for lifecycle operations on an [`crate::McpServerHandle`].
#[derive(Debug, Error)]
pub enum McpError {
    /// A transport-level failure occurred.
    #[error(transparent)]
    Transport(#[from] TransportError),
    /// The `initialize` handshake was rejected or returned an unusable result.
    #[error("MCP initialize handshake failed: {0}")]
    Handshake(String),
    /// A tool invocation exceeded its configured timeout.
    #[error("tool invocation timed out after {0:?}")]
    Timeout(std::time::Duration),
    /// A tool result exceeded the configured output-size cap.
    #[error("tool output exceeded size cap of {cap} bytes (was {actual} bytes)")]
    OutputTooLarge { cap: usize, actual: usize },
    /// The operation was attempted while the server was not connected.
    #[error("MCP server is not connected")]
    NotConnected,
    /// A namespaced tool name could not be resolved to this server's tool.
    #[error("unknown tool: {0}")]
    UnknownTool(String),
}
