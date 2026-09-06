//! A minimal, CROSS-PLATFORM mock MCP server used by the stdio transport tests.
//!
//! CHOSEN APPROACH (per the FEAT-001 plan): a dedicated `[[bin]]` target rather
//! than re-invoking the test binary in a "mock mode". A separate binary keeps
//! the child's stdout free of the test harness's own chatter, so the newline-
//! delimited JSON-RPC framing is clean and identical on ubuntu-latest and
//! windows-latest. Tests spawn it via `env!("CARGO_BIN_EXE_mcp-mock-server")`,
//! which Cargo sets to the built binary path (resolving `.exe` on Windows).
//!
//! It speaks JSON-RPC 2.0 over stdin/stdout, one JSON object per line, and
//! implements just enough of MCP for the tests:
//!
//! - `initialize`      -> returns a fixed protocol version + `tools` capability.
//! - `tools/list`      -> returns two tools: `echo` and `slow`.
//! - `tools/call`
//!     - `echo`  -> echoes its `arguments` straight back in the result.
//!     - `slow`  -> sleeps for a configurable number of milliseconds before
//!       replying (used to exercise the per-tool timeout path).
//! - `notifications/initialized` (a notification, no `id`) -> ignored.
//!
//! It uses ONLY the standard library (blocking stdin/stdout + `thread::sleep`),
//! so it has no async runtime and behaves identically across platforms.

use std::io::{BufRead, Write};
use std::thread;
use std::time::Duration;

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
                    "serverInfo": { "name": "mock-mcp-server", "version": "0.1.0" }
                }
            }),
            "tools/list" => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "tools": [
                        {
                            "name": "echo",
                            "description": "echo the arguments back",
                            "inputSchema": { "type": "object" }
                        },
                        {
                            "name": "slow",
                            "description": "sleep then reply (for timeout tests)",
                            "inputSchema": {
                                "type": "object",
                                "properties": { "delayMs": { "type": "integer" } }
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
        "slow" => {
            let delay_ms = arguments
                .get("delayMs")
                .and_then(Value::as_u64)
                .unwrap_or(1000);
            thread::sleep(Duration::from_millis(delay_ms));
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": { "slept": delay_ms, "isError": false }
            })
        }
        other => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": -32602, "message": format!("unknown tool: {other}") }
        }),
    }
}
