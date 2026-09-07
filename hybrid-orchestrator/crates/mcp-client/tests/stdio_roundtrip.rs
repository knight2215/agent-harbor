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
//!   6. Controlled child environment (architecture.md Section 9.4): a SECRET env
//!      var set in the PARENT is NOT visible to the spawned stdio child, while a
//!      CONFIGURED env var IS. Proves `env_clear` + minimal allowlist + layering.
//!   7. Shutdown terminates the stdio child (termination on disable/removal/exit):
//!      after `Transport::shutdown`, in-flight/new requests fail `Closed`.
//!
//! CROSS-PLATFORM: the mock server is spawned via `env!("CARGO_BIN_EXE_...")`,
//! which Cargo resolves to the built binary (with `.exe` on Windows). No shell,
//! no Unix-only syscalls.

use std::time::Duration;

use mcp_client::session::{HandleLimits, McpConnectionState};
use mcp_client::{resolve_namespaced, McpServerHandle, StdioTransport, Transport};

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
async fn runtime_add_path_is_config_only_handshake_and_discovery_automatic() {
    // P6.7 (architecture.md Section 10): prove the runtime "add an MCP server"
    // path needs NO code change - only a config row. This mirrors what the
    // Tool/MCP Server Manager does when a user adds a server at runtime: it
    // builds an `McpServerConfig` and drives the SAME `McpServerHandle`
    // lifecycle. Nothing here adds or edits a server in code beyond constructing
    // the config value.
    //
    // The assertions establish that, from a config value alone:
    //   - the handshake happens automatically on `connect()` (state -> Connected,
    //     server capabilities stored from `initialize`), and
    //   - tool discovery happens automatically (the config-added server's tools
    //     are enumerated), and
    //   - the discovered tools are EXPOSED to the request builder as namespaced
    //     `ToolSpec`s (visible_tools), i.e. they become usable with no code edit.
    let config = McpServerConfig {
        id: Uuid::new_v4(),
        name: "runtime-added".to_string(),
        transport: McpTransport::Stdio {
            command: MOCK_SERVER.to_string(),
            args: vec![],
            env: vec![],
        },
        permission_mode: PermissionMode::Allow,
        enabled: true,
    };

    // The ONLY step to "add" the server: hand the config to the same handle the
    // manager uses. No code path is added per-server.
    let handle = McpServerHandle::new(config);

    // Handshake happens automatically on connect.
    handle
        .connect()
        .await
        .expect("config-only connect succeeds");
    assert_eq!(handle.state().await, McpConnectionState::Connected);
    assert!(
        handle.server_capabilities().await.get("tools").is_some(),
        "initialize handshake stored server capabilities automatically"
    );

    // Tool discovery happened automatically as part of connect.
    let discovered: Vec<String> = handle
        .tools()
        .await
        .iter()
        .map(|t| t.name.clone())
        .collect();
    assert!(
        discovered.contains(&"echo".to_string()) && discovered.contains(&"slow".to_string()),
        "tools were discovered automatically from the config-added server, got {discovered:?}"
    );

    // The discovered tools are exposed to the request builder (namespaced per
    // server), i.e. immediately usable - with no code change to add the server.
    let visible = handle.visible_tools().await;
    assert!(
        visible.iter().any(|s| s.function.name.ends_with("__echo")),
        "discovered tools must be exposed as namespaced ToolSpecs"
    );

    handle.teardown().await;
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

#[tokio::test]
async fn controlled_env_hides_parent_secret_but_keeps_configured_var() {
    // A SENTINEL secret is placed in THIS (parent) process's environment and is
    // deliberately NOT part of the child's configured env. Per architecture.md
    // Section 9.4 the child must run in a controlled environment (env_clear +
    // minimal allowlist + configured vars), so it MUST NOT inherit this secret.
    //
    // SAFETY / race avoidance: `set_var` mutates process-global state shared by
    // every test in this binary. To avoid racing a sibling test that reads the
    // same key, the sentinel key is made UNIQUE PER TEST RUN (a fresh UUID
    // suffix) so no other test can ever observe or clobber it, and the key is
    // removed via a RAII guard so it is cleared even if an assertion below
    // panics (leaving no residue for other tests). The child's env is a snapshot
    // taken at spawn time (inside `connect()` below), so once spawned the
    // assertions do not depend on any concurrent mutation of the parent env.
    let secret_key = format!("AH_TEST_PARENT_SECRET_SENTINEL_{}", Uuid::new_v4().simple());
    const CONFIGURED_KEY: &str = "AH_TEST_CONFIGURED_VAR";
    std::env::set_var(&secret_key, "super-secret-value");
    // RAII guard: remove the unique sentinel on every exit path (incl. panics).
    struct EnvGuard(String);
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            std::env::remove_var(&self.0);
        }
    }
    let _secret_guard = EnvGuard(secret_key.clone());

    let id = Uuid::new_v4();
    let config = McpServerConfig {
        id,
        name: "env-sandbox".to_string(),
        transport: McpTransport::Stdio {
            command: MOCK_SERVER.to_string(),
            args: vec![],
            // Only this configured var is passed explicitly; the parent secret
            // is NOT here.
            env: vec![(CONFIGURED_KEY.to_string(), "configured-value".to_string())],
        },
        permission_mode: PermissionMode::Allow,
        enabled: true,
    };
    let handle = McpServerHandle::new(config);
    handle.connect().await.expect("connect");

    // The parent secret is NOT visible to the child (reportEnv -> null).
    let secret = handle
        .invoke("call-secret", "reportEnv", json!({ "name": secret_key }))
        .await
        .expect("reportEnv result");
    assert!(!secret.is_error, "reportEnv is not an error");
    assert_eq!(
        secret.content["value"],
        json!(null),
        "parent secret must NOT be inherited by the child"
    );

    // The explicitly configured var IS visible (layering still works).
    let configured = handle
        .invoke(
            "call-configured",
            "reportEnv",
            json!({ "name": CONFIGURED_KEY }),
        )
        .await
        .expect("reportEnv result");
    assert_eq!(
        configured.content["value"],
        json!("configured-value"),
        "configured env var must be visible to the child"
    );

    handle.teardown().await;
    // Do not assert PATH is absent: it is intentionally allowlisted so the child
    // (a cargo-built binary invoked by absolute path) can launch on both OSes.
    // The sentinel is cleared by `_secret_guard` (RAII) on scope exit.
}

#[tokio::test]
async fn shutdown_terminates_stdio_child() {
    // Spawn the transport directly so we can drive shutdown and observe that the
    // child is gone: after shutdown, a fresh request must fail `Closed` because
    // the reader task ended when the child's stdout closed on kill.
    let transport = StdioTransport::spawn(MOCK_SERVER, &[], &[])
        .await
        .expect("spawn mock server");

    // Sanity: the child answers before shutdown.
    let init = transport
        .request(
            "initialize",
            Some(json!({
                "protocolVersion": "2024-11-05",
                "capabilities": { "tools": {} },
                "clientInfo": { "name": "test", "version": "0.0.0" }
            })),
        )
        .await
        .expect("initialize round-trips before shutdown");
    assert!(init.get("capabilities").is_some());

    // Terminate the child.
    transport.shutdown().await;

    // With the child killed, a new request cannot be answered: writing to the
    // broken stdin pipe fails (and, either way, the reader task has drained
    // pending requests with `Closed` once the child's stdout closed). Wrap in a
    // timeout so a regression that reintroduced a hang FAILS the test rather
    // than blocking CI forever.
    let after = tokio::time::timeout(
        Duration::from_secs(5),
        transport.request("tools/list", None),
    )
    .await
    .expect("post-shutdown request must resolve, not hang")
    .expect_err("request after shutdown must fail");
    assert!(
        matches!(after, mcp_client::TransportError::Closed(_)),
        "post-shutdown request should be Closed, got: {after:?}"
    );
}
