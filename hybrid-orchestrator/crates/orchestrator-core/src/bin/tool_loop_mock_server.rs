//! A minimal, CROSS-PLATFORM mock MCP server for the FEAT-002 orchestrator-core
//! integration test (`tests/tool_loop.rs`).
//!
//! WHY A DEDICATED `[[bin]]` (mirrors the mcp-client `mcp-mock-server`): the
//! bundled mcp-client fixture is a `[[bin]]` of the `mcp-client` crate, and
//! `env!("CARGO_BIN_EXE_mcp-mock-server")` is only set for THAT crate's own
//! tests, not for `orchestrator-core`'s. Cargo does not build a path
//! dependency's binaries into a dependent crate's target dir, so the
//! orchestrator-core test cannot reach the mcp-client fixture binary. We
//! therefore ship an equivalent fixture as an orchestrator-core `[[bin]]` so
//! the test can spawn it via `env!("CARGO_BIN_EXE_tool-loop-mock-server")`
//! (Cargo resolves `.exe` on Windows automatically). It uses ONLY the standard
//! library (blocking stdin/stdout), so it behaves identically on ubuntu-latest
//! and windows-latest.
//!
//! It speaks JSON-RPC 2.0 over stdin/stdout, one JSON object per line, and
//! implements just enough of MCP for the loop test:
//!
//! - `initialize`  -> fixed protocol version + `tools` capability.
//! - `tools/list`  -> one tool `echo` whose input schema REQUIRES a string
//!   `text` property, so the bridge's argument-schema validation has something
//!   to enforce (P3.6 schema-validation-failure coverage).
//! - `tools/call`  -> `echo` returns its `arguments` back in the result.
//! - `notifications/initialized` (no `id`) -> ignored.

use std::io::{BufRead, Write};

use serde_json::{json, Value};

fn main() {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(_) => continue,
        };

        // Notifications carry no `id` and expect no reply.
        let id = match msg.get("id") {
            Some(id) => id.clone(),
            None => continue,
        };
        let method = msg.get("method").and_then(Value::as_str).unwrap_or("");

        let response = match method {
            "initialize" => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "tool-loop-mock-server", "version": "0.1.0" }
                }
            }),
            "tools/list" => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "tools": [
                        {
                            "name": "echo",
                            "description": "echo the text back",
                            "inputSchema": {
                                "type": "object",
                                "properties": { "text": { "type": "string" } },
                                "required": ["text"]
                            }
                        }
                    ]
                }
            }),
            "tools/call" => handle_tools_call(&id, &msg),
            _ => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32601, "message": format!("method not found: {method}") }
            }),
        };

        let mut serialized = response.to_string();
        serialized.push('\n');
        if out.write_all(serialized.as_bytes()).is_err() {
            break;
        }
        let _ = out.flush();
    }
}

fn handle_tools_call(id: &Value, msg: &Value) -> Value {
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let arguments = params.get("arguments").cloned().unwrap_or(json!({}));

    match name {
        "echo" => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": { "echoed": arguments, "isError": false }
        }),
        other => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": -32602, "message": format!("unknown tool: {other}") }
        }),
    }
}
