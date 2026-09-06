//! Integration tests for the mcp-client crate (architecture.md Section 5),
//! driven against the bundled cross-platform `mcp-mock-server` `[[bin]]`.
//!
//! Coverage (FEAT-001 acceptance):
//!   1. Full stdio round-trip: `initialize` handshake succeeds + capabilities
//!      stored, `tools/list` discovery returns the expected descriptors, and a
//!      `tools/call` round-trips a result.
//!   2. Tool-name namespacing collision: two servers exposing a same-named bare
//!      tool produce DISTINCT namespaced `ToolSpec` names, each reverse-resolving
//!      to the correct server.
//!   3. Invoke error path captured as a structured tool error (a server-returned
//!      JSON-RPC error for an unknown tool becomes `ToolResult{is_error:true}`).
//!      NOTE: argument-schema VALIDATION lives bridge-side (P3.4) and is covered
//!      in FEAT-002; here we cover the transport/invoke error path.
//!   4. Per-tool timeout yields a structured error (not a hang/crash).
//!   5. Disconnected-server tools are hidden from `visible_tools()`.
//!
//! CROSS-PLATFORM: the mock server is spawned via `env!("CARGO_BIN_EXE_...")`,
//! which Cargo resolves to the built binary (with `.exe` on Windows). No shell,
//! no Unix-only syscalls.

use std::time::Duration;

use mcp_client::session::{HandleLimits, McpConnectionState};
use mcp_client::{resolve_namespaced, McpServerHandle};

use domain::{McpServerConfig, McpTransport, PermissionMode};
use serde_json::json;
use uuid::Uuid;

/// Path to the bundled mock MCP server binary (resolved by Cargo, `.exe` on
/// Windows).
const MOCK_SERVER: &str = env!("CARGO_BIN_EXE_mcp-mock-server");

fn stdio_config(id: Uuid, name: &str) -> McpServerConfig {
    McpServerConfig {
        id,
        name: name.to_string(),
        transport: McpTransport::Stdio {
            command: MOCK_SERVER.to_string(),
            args: vec![],
            env: vec![],
        },
        permission_mode: PermissionMode::Allow,
        enabled: true,
    }
}

#[tokio::test]
async fn stdio_handshake_discovery_and_invoke_round_trip() {
    let handle = McpServerHandle::new(stdio_config(Uuid::new_v4(), "mock"));

    // Handshake + tools/list.
    handle.connect().await.expect("connect should succeed");
    assert_eq!(handle.state().await, McpConnectionState::Connected);

    // Capabilities stored from initialize.
    let caps = handle.server_capabilities().await;
    assert!(
        caps.get("tools").is_some(),
        "server tools capability stored"
    );

    // Discovery returns the two mock tools.
    let tools = handle.tools().await;
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"echo"));
    assert!(names.contains(&"slow"));

    // tools/call round-trips a result.
    let result = handle
        .invoke("call-1", "echo", json!({ "hello": "world" }))
        .await
        .expect("invoke should return a result");
    assert!(!result.is_error, "echo is not an error");
    assert_eq!(result.call_id, "call-1");
    assert_eq!(result.content["echoed"], json!({ "hello": "world" }));

    handle.teardown().await;
    assert_eq!(handle.state().await, McpConnectionState::Disconnected);
}

#[tokio::test]
async fn namespacing_avoids_collisions_across_servers() {
    let id_a = Uuid::new_v4();
    let id_b = Uuid::new_v4();
    let handle_a = McpServerHandle::new(stdio_config(id_a, "server-a"));
    let handle_b = McpServerHandle::new(stdio_config(id_b, "server-b"));

    handle_a.connect().await.expect("connect a");
    handle_b.connect().await.expect("connect b");

    let specs_a = handle_a.visible_tools().await;
    let specs_b = handle_b.visible_tools().await;

    // Both servers expose a bare `echo`, but the namespaced names differ.
    let echo_a = specs_a
        .iter()
        .find(|s| s.function.name.ends_with("__echo"))
        .expect("server a exposes echo");
    let echo_b = specs_b
        .iter()
        .find(|s| s.function.name.ends_with("__echo"))
        .expect("server b exposes echo");
    assert_ne!(echo_a.function.name, echo_b.function.name);

    // Each reverse-resolves to its owning server id.
    let (ns_a, tool_a) = resolve_namespaced(&echo_a.function.name).unwrap();
    let (ns_b, tool_b) = resolve_namespaced(&echo_b.function.name).unwrap();
    assert_eq!(ns_a, id_a.to_string());
    assert_eq!(ns_b, id_b.to_string());
    assert_eq!(tool_a, "echo");
    assert_eq!(tool_b, "echo");

    handle_a.teardown().await;
    handle_b.teardown().await;
}

#[tokio::test]
async fn unknown_tool_error_is_captured_as_structured_result() {
    let handle = McpServerHandle::new(stdio_config(Uuid::new_v4(), "mock"));
    handle.connect().await.expect("connect");

    // The mock server returns a JSON-RPC error for an unknown tool; the handle
    // converts it into a structured tool-error result rather than an Err.
    let result = handle
        .invoke("call-x", "does_not_exist", json!({}))
        .await
        .expect("invoke returns a structured result, not Err");
    assert!(result.is_error, "unknown tool must be a structured error");
    assert_eq!(result.call_id, "call-x");

    handle.teardown().await;
}

#[tokio::test]
async fn per_tool_timeout_yields_structured_error_not_hang() {
    // A tiny timeout against the `slow` tool must elapse into a structured error.
    let limits = HandleLimits {
        tool_timeout: Duration::from_millis(50),
        ..HandleLimits::default()
    };
    let handle = McpServerHandle::with_limits(stdio_config(Uuid::new_v4(), "mock"), limits);
    handle.connect().await.expect("connect");

    let result = handle
        .invoke("call-slow", "slow", json!({ "delayMs": 5000 }))
        .await
        .expect("timeout yields a structured result, not Err");
    assert!(result.is_error, "timed-out invoke is a structured error");
    let message = result.content["error"].as_str().unwrap_or_default();
    assert!(message.contains("timed out"), "message: {message}");

    handle.teardown().await;
}

#[tokio::test]
async fn disconnected_server_tools_are_hidden() {
    let handle = McpServerHandle::new(stdio_config(Uuid::new_v4(), "mock"));
    handle.connect().await.expect("connect");

    // While connected, tools are visible.
    assert!(!handle.visible_tools().await.is_empty());

    // After teardown the server is disconnected and its tools are hidden from
    // new requests (Section 5.6), even though the cache still holds them.
    handle.teardown().await;
    assert_eq!(handle.state().await, McpConnectionState::Disconnected);
    assert!(
        handle.visible_tools().await.is_empty(),
        "disconnected server exposes no tools"
    );
    assert!(
        !handle.tools().await.is_empty(),
        "cache is retained for reconnect refresh"
    );
}
