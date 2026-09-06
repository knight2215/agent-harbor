//! `mcp-client` crate: the built-in MCP client (architecture.md Section 5).
//!
//! Implements the two MCP transports (Section 5.2, [`transport`]), the per-server
//! lifecycle and resource handling (Sections 5.3 / 5.6, [`session`]), and tool
//! discovery + schema mapping into `providers::ToolSpec` namespaced by server
//! (Section 5.4, [`tools`]).
//!
//! Phase 3 (FEAT-001) implements this crate: the stdio + HTTP/SSE transports,
//! the [`session::McpServerHandle`] lifecycle (handshake / list-tools /
//! on-demand invoke / bounded reconnect / teardown), tool namespacing, per-tool
//! timeouts + output caps, and hiding tools from disconnected servers. The
//! `orchestrator-core` function-calling bridge, the permission model, and the
//! Tauri wiring build on this in FEAT-002 (P3.4 / P3.5 and the core-side of
//! P3.6).
//!
//! This crate carries crates.io deps (tokio process/io/sync/time, serde_json for
//! the in-crate JSON-RPC layer, reqwest + rustls-`ring` + futures/bytes for the
//! HTTP/SSE transport), so it is under `[workspace] exclude` in the root
//! Cargo.toml and built/tested/clippied in CI via
//! `--manifest-path crates/mcp-client/Cargo.toml` (the offline sandbox cannot
//! resolve those deps). The stdio transport is exercised end to end against a
//! bundled cross-platform mock server (the `mcp-mock-server` `[[bin]]`); the
//! HTTP/SSE transport is validated against a mocked HTTP server in CI, never
//! live network.
//!
//! NOTE for FEAT-002: this crate no longer exposes `placeholder()`. The
//! `orchestrator-core` smoke test still calls `mcp_client::placeholder()` and
//! MUST be updated in FEAT-002 (e.g. to reference `McpServerHandle`) when the
//! bridge is wired; that call site is intentionally left for FEAT-002 to avoid
//! touching `orchestrator-core` here.

pub mod crypto;
pub mod error;
pub mod session;
pub mod tools;
pub mod transport;

// Ergonomic re-exports of the public API the orchestrator-core bridge and
// tauri-app will use. Kept ALPHABETICALLY ORDERED (rustfmt / review convention).
pub use crypto::ensure_crypto_provider;
pub use error::{JsonRpcErrorPayload, McpError, TransportError};
pub use session::{
    HandleLimits, McpConnectionState, McpServerHandle, DEFAULT_OUTPUT_CAP_BYTES,
    DEFAULT_TOOL_TIMEOUT, MCP_PROTOCOL_VERSION,
};
pub use tools::{namespace_tool, resolve_namespaced, ToolDescriptor, NAMESPACE_SEPARATOR};
pub use transport::http_sse::HttpSseTransport;
pub use transport::stdio::StdioTransport;
pub use transport::Transport;
