//! MCP server lifecycle (architecture.md Section 5.3, P3.2) plus the mcp-client
//! side of resource handling (Section 5.6, P3.6).
//!
//! [`McpServerHandle`] owns one server: its [`McpServerConfig`], connection
//! [`McpConnectionState`], negotiated server capabilities, and cached
//! [`ToolDescriptor`]s. It drives the Section 5.3 lifecycle:
//!
//! ```text
//! spawn/connect -> initialize handshake -> tools/list (cache) -> [ready]
//!               -> invoke on demand -> (on loss) bounded reconnect w/ backoff
//!               -> teardown
//! ```
//!
//! Resource handling (Section 5.6): [`McpServerHandle::invoke`] applies a
//! per-tool timeout and an output-size cap and returns a STRUCTURED
//! [`ToolResult`] with `is_error = true` on failure (never a panic / turn
//! crash). [`McpServerHandle::visible_tools`] hides tools while the server is
//! `Disconnected`.
//!
//! ## Why an internal connection-state enum
//!
//! `orchestrator-core` already defines `McpConnectionState` and depends on this
//! crate (a path dep), so this crate CANNOT depend back on `orchestrator-core`
//! without forming a `cargo` cycle. We therefore define our own
//! [`McpConnectionState`] here (identical variants) and let the FEAT-002 bridge
//! map it onto the core's event enum. The two are intentionally kept in sync.

use std::sync::Arc;
use std::time::Duration;

use domain::{McpServerConfig, McpTransport, ToolResult};
use providers::ToolSpec;
use serde_json::{json, Value};
use tokio::sync::RwLock;

use crate::error::{McpError, TransportError};
use crate::tools::ToolDescriptor;
use crate::transport::http_sse::HttpSseTransport;
use crate::transport::stdio::StdioTransport;
use crate::transport::Transport;

/// The MCP protocol version this client advertises in the `initialize`
/// handshake. A concrete, widely-supported spec revision.
pub const MCP_PROTOCOL_VERSION: &str = "2024-11-05";

/// Default per-tool invocation timeout (Section 5.6 resource limit).
pub const DEFAULT_TOOL_TIMEOUT: Duration = Duration::from_secs(30);

/// Default cap on a single tool result's serialized size, in bytes (Section 5.6
/// resource limit). Results larger than this are returned as a structured error
/// rather than fed to the model.
pub const DEFAULT_OUTPUT_CAP_BYTES: usize = 1024 * 1024;

/// Default bounded-reconnect policy: attempts and base backoff.
pub const DEFAULT_MAX_RECONNECT_ATTEMPTS: u32 = 5;
pub const DEFAULT_RECONNECT_BASE_DELAY: Duration = Duration::from_millis(200);

/// Connection state of an MCP server (mirror of
/// `orchestrator_core::McpConnectionState`; see the module docs for why it is
/// redefined here). Display-safe; carries no secrets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpConnectionState {
    Connecting,
    Connected,
    Disconnected,
}

/// Tunable resource limits and reconnect policy for a handle (Section 5.6).
#[derive(Debug, Clone, Copy)]
pub struct HandleLimits {
    /// Per-tool invocation timeout.
    pub tool_timeout: Duration,
    /// Maximum serialized size of a single tool result, in bytes.
    pub output_cap_bytes: usize,
    /// Maximum bounded-reconnect attempts before giving up (-> `Disconnected`).
    pub max_reconnect_attempts: u32,
    /// Base delay for exponential reconnect backoff.
    pub reconnect_base_delay: Duration,
}

impl Default for HandleLimits {
    fn default() -> Self {
        HandleLimits {
            tool_timeout: DEFAULT_TOOL_TIMEOUT,
            output_cap_bytes: DEFAULT_OUTPUT_CAP_BYTES,
            max_reconnect_attempts: DEFAULT_MAX_RECONNECT_ATTEMPTS,
            reconnect_base_delay: DEFAULT_RECONNECT_BASE_DELAY,
        }
    }
}

/// Mutable per-connection state, guarded so the handle is `Send + Sync` and
/// cheap to share across tasks.
struct Inner {
    state: McpConnectionState,
    /// Server capabilities returned by `initialize` (opaque JSON, stored for
    /// later feature gating by the bridge).
    server_capabilities: Value,
    /// Cached tool descriptors from the last successful `tools/list`.
    tools: Vec<ToolDescriptor>,
    /// The live transport, present only while `Connected`/`Connecting`.
    transport: Option<Arc<dyn Transport>>,
}

/// A per-server MCP handle (architecture.md Section 5.3).
pub struct McpServerHandle {
    config: McpServerConfig,
    limits: HandleLimits,
    inner: RwLock<Inner>,
}

impl McpServerHandle {
    /// Create a handle for `config` in the `Disconnected` state with default
    /// limits. Call [`connect`](Self::connect) to spawn/connect and handshake.
    pub fn new(config: McpServerConfig) -> Self {
        Self::with_limits(config, HandleLimits::default())
    }

    /// Create a handle with explicit resource limits / reconnect policy.
    pub fn with_limits(config: McpServerConfig, limits: HandleLimits) -> Self {
        McpServerHandle {
            config,
            limits,
            inner: RwLock::new(Inner {
                state: McpConnectionState::Disconnected,
                server_capabilities: Value::Null,
                tools: Vec::new(),
                transport: None,
            }),
        }
    }

    /// The server's opaque id (used as the tool namespace so reverse-resolution
    /// is unambiguous even when the display name contains `__`).
    pub fn namespace(&self) -> String {
        self.config.id.to_string()
    }

    /// The server's configuration.
    pub fn config(&self) -> &McpServerConfig {
        &self.config
    }

    /// The current connection state.
    pub async fn state(&self) -> McpConnectionState {
        self.inner.read().await.state
    }

    /// The negotiated server capabilities from the last successful handshake.
    pub async fn server_capabilities(&self) -> Value {
        self.inner.read().await.server_capabilities.clone()
    }

    /// The cached tool descriptors (regardless of connection state; prefer
    /// [`visible_tools`](Self::visible_tools) when building a request).
    pub async fn tools(&self) -> Vec<ToolDescriptor> {
        self.inner.read().await.tools.clone()
    }

    /// The tools this server currently exposes to NEW requests, as
    /// [`providers::ToolSpec`]s namespaced by server id.
    ///
    /// Per Section 5.6, tools from a `Disconnected` server are HIDDEN: this
    /// returns an empty vec unless the server is `Connected`.
    pub async fn visible_tools(&self) -> Vec<ToolSpec> {
        let inner = self.inner.read().await;
        if inner.state != McpConnectionState::Connected {
            return Vec::new();
        }
        let ns = self.config.id.to_string();
        inner.tools.iter().map(|t| t.to_tool_spec(&ns)).collect()
    }

    /// Spawn/connect the transport, perform the `initialize` handshake, cache
    /// server capabilities, and run `tools/list`, transitioning
    /// `Connecting -> Connected`. On any failure the handle is left
    /// `Disconnected` and the error returned.
    pub async fn connect(&self) -> Result<(), McpError> {
        {
            let mut inner = self.inner.write().await;
            inner.state = McpConnectionState::Connecting;
        }

        let result = self.establish().await;
        if result.is_err() {
            let mut inner = self.inner.write().await;
            inner.state = McpConnectionState::Disconnected;
            inner.transport = None;
        }
        result
    }

    /// Build the transport, handshake, and list tools. On success, commit the
    /// transport + caps + tools and mark `Connected`.
    async fn establish(&self) -> Result<(), McpError> {
        let transport: Arc<dyn Transport> = match &self.config.transport {
            McpTransport::Stdio { command, args, env } => {
                Arc::new(StdioTransport::spawn(command, args, env).await?)
            }
            McpTransport::HttpSse { url, headers } => {
                Arc::new(HttpSseTransport::connect(url.clone(), headers.clone()))
            }
        };

        let capabilities = handshake(transport.as_ref()).await?;
        let descriptors = list_tools(transport.as_ref()).await?;

        let mut inner = self.inner.write().await;
        inner.server_capabilities = capabilities;
        inner.tools = descriptors;
        inner.transport = Some(transport);
        inner.state = McpConnectionState::Connected;
        Ok(())
    }

    /// Invoke a bare `tool` (server-local name) with `arguments`, applying the
    /// per-tool timeout and output-size cap.
    ///
    /// Returns a STRUCTURED [`ToolResult`] in all outcomes EXCEPT lifecycle
    /// failures where there is no result to return (not connected). A timeout,
    /// an oversized result, a transport error, or a server-returned JSON-RPC
    /// error all yield `Ok(ToolResult { is_error: true, .. })` so the model can
    /// recover rather than crashing the turn (Section 5.6). `call_id` correlates
    /// the result back to the originating tool call.
    pub async fn invoke(
        &self,
        call_id: impl Into<String>,
        tool: &str,
        arguments: Value,
    ) -> Result<ToolResult, McpError> {
        let call_id = call_id.into();
        let transport = {
            let inner = self.inner.read().await;
            if inner.state != McpConnectionState::Connected {
                return Err(McpError::NotConnected);
            }
            match &inner.transport {
                Some(t) => Arc::clone(t),
                None => return Err(McpError::NotConnected),
            }
        };

        let params = json!({ "name": tool, "arguments": arguments });
        let fut = transport.request("tools/call", Some(params));

        match tokio::time::timeout(self.limits.tool_timeout, fut).await {
            // The server answered in time.
            Ok(Ok(value)) => Ok(self.cap_result(call_id, value)),
            // A transport / JSON-RPC error: surface as a structured tool error.
            Ok(Err(err)) => Ok(error_result(call_id, &err.to_string())),
            // The invocation timed out: structured tool error, not a hang.
            Err(_) => Ok(error_result(
                call_id,
                &format!("tool timed out after {:?}", self.limits.tool_timeout),
            )),
        }
    }

    /// Apply the output-size cap to a successful tool result, converting an
    /// oversized payload into a structured error result.
    fn cap_result(&self, call_id: String, value: Value) -> ToolResult {
        let size = serde_json::to_string(&value).map(|s| s.len()).unwrap_or(0);
        if size > self.limits.output_cap_bytes {
            return error_result(
                call_id,
                &format!(
                    "tool output {} bytes exceeds cap of {} bytes",
                    size, self.limits.output_cap_bytes
                ),
            );
        }
        // MCP `tools/call` results may carry an `isError` flag; honor it.
        let is_error = value
            .get("isError")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        ToolResult {
            call_id,
            content: value,
            is_error,
        }
    }

    /// Attempt a bounded reconnect with exponential backoff (Section 5.3 / 5.6).
    ///
    /// Marks the handle `Connecting`, then retries [`connect`](Self::connect) up
    /// to `max_reconnect_attempts` times with a delay of
    /// `reconnect_base_delay * 2^(attempt-1)` between tries. On success the tools
    /// cache is refreshed (via the `tools/list` inside `connect`). On exhausting
    /// all attempts the handle is left `Disconnected` and the last error
    /// returned.
    ///
    /// The backoff multiplier and the resulting `Duration` are computed with
    /// SATURATING arithmetic so a large `max_reconnect_attempts` set via the
    /// public [`HandleLimits`] can never overflow-panic (`2u32.pow` overflows
    /// past ~32 attempts, and the `Duration` multiply can overflow earlier). The
    /// exponent is capped so the multiplier stays within `u32`, and the delay
    /// saturates at [`Duration::MAX`] rather than wrapping.
    pub async fn reconnect(&self) -> Result<(), McpError> {
        /// Max shift for the backoff multiplier: `1u32 << 31` is the largest
        /// power of two that fits in a `u32`, so the exponent is capped here.
        const MAX_BACKOFF_SHIFT: u32 = 31;

        let mut last_err: Option<McpError> = None;
        for attempt in 0..self.limits.max_reconnect_attempts {
            if attempt > 0 {
                let shift = (attempt - 1).min(MAX_BACKOFF_SHIFT);
                let multiplier = 1u32 << shift;
                let delay = self
                    .limits
                    .reconnect_base_delay
                    .checked_mul(multiplier)
                    .unwrap_or(Duration::MAX);
                tokio::time::sleep(delay).await;
            }
            match self.connect().await {
                Ok(()) => return Ok(()),
                Err(e) => last_err = Some(e),
            }
        }
        let mut inner = self.inner.write().await;
        inner.state = McpConnectionState::Disconnected;
        Err(last_err.unwrap_or_else(|| {
            McpError::Transport(TransportError::Closed(
                "reconnect exhausted with no attempts".to_string(),
            ))
        }))
    }

    /// Tear the server down: shut the transport (kill the child for stdio) and
    /// mark the handle `Disconnected`. Idempotent.
    pub async fn teardown(&self) {
        let transport = {
            let mut inner = self.inner.write().await;
            inner.state = McpConnectionState::Disconnected;
            inner.transport.take()
        };
        if let Some(t) = transport {
            t.shutdown().await;
        }
    }
}

/// Perform the MCP `initialize` handshake over `transport`: send the protocol
/// version + client capabilities, receive + return the server's capabilities,
/// then send the `notifications/initialized` acknowledgement.
async fn handshake(transport: &dyn Transport) -> Result<Value, McpError> {
    let params = json!({
        "protocolVersion": MCP_PROTOCOL_VERSION,
        "capabilities": { "tools": {} },
        "clientInfo": { "name": "agent-harbor", "version": env!("CARGO_PKG_VERSION") },
    });
    let result = transport
        .request("initialize", Some(params))
        .await
        .map_err(McpError::Transport)?;

    let capabilities = result.get("capabilities").cloned().unwrap_or(Value::Null);

    // Acknowledge per the MCP lifecycle; a failure here is non-fatal to the
    // handshake result but is surfaced so the caller can log it.
    transport
        .notify("notifications/initialized", None)
        .await
        .map_err(McpError::Transport)?;

    Ok(capabilities)
}

/// Run `tools/list` over `transport` and parse the descriptors. Tolerates a
/// missing/empty `tools` array (returns an empty vec).
async fn list_tools(transport: &dyn Transport) -> Result<Vec<ToolDescriptor>, McpError> {
    let result = transport
        .request("tools/list", None)
        .await
        .map_err(McpError::Transport)?;

    let tools_value = result.get("tools").cloned().unwrap_or(json!([]));
    let descriptors: Vec<ToolDescriptor> = serde_json::from_value(tools_value)
        .map_err(|e| McpError::Handshake(format!("failed to parse tools/list: {e}")))?;
    Ok(descriptors)
}

/// Build a structured error [`ToolResult`] (Section 5.6): `is_error = true` with
/// a display-safe message payload so the model can recover.
fn error_result(call_id: String, message: &str) -> ToolResult {
    ToolResult {
        call_id,
        content: json!({ "error": message }),
        is_error: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{McpTransport, PermissionMode};
    use uuid::Uuid;

    fn stdio_config() -> McpServerConfig {
        McpServerConfig {
            id: Uuid::nil(),
            name: "test".to_string(),
            transport: McpTransport::Stdio {
                command: "does-not-run".to_string(),
                args: vec![],
                env: vec![],
            },
            permission_mode: PermissionMode::Allow,
            enabled: true,
        }
    }

    #[tokio::test]
    async fn new_handle_starts_disconnected_and_hides_tools() {
        let handle = McpServerHandle::new(stdio_config());
        assert_eq!(handle.state().await, McpConnectionState::Disconnected);
        // Disconnected servers expose no tools to new requests (Section 5.6).
        assert!(handle.visible_tools().await.is_empty());
    }

    #[tokio::test]
    async fn invoke_while_disconnected_errors() {
        let handle = McpServerHandle::new(stdio_config());
        let err = handle.invoke("c1", "read", json!({})).await.unwrap_err();
        assert!(matches!(err, McpError::NotConnected));
    }

    #[test]
    fn error_result_is_structured() {
        let r = error_result("c1".to_string(), "boom");
        assert!(r.is_error);
        assert_eq!(r.call_id, "c1");
        assert_eq!(r.content, json!({ "error": "boom" }));
    }
}
