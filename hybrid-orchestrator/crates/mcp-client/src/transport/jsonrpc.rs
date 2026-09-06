//! A small in-crate JSON-RPC 2.0 layer (architecture.md Section 5.2/5.3).
//!
//! MCP speaks JSON-RPC 2.0 over whichever transport is chosen (stdio framing is
//! newline-delimited JSON; HTTP/SSE posts requests and receives responses/
//! notifications as SSE `data:` payloads). This module defines the wire types
//! shared by both transports:
//!
//! - [`JsonRpcRequest`]: a client -> server call carrying an `id`, `method`, and
//!   optional `params`.
//! - [`JsonRpcResponse`]: the correlated server -> client reply, carrying either
//!   a `result` or an `error`.
//! - [`JsonRpcNotification`]: a server -> client (or fire-and-forget client ->
//!   server) message with no `id` and therefore no reply.
//! - [`JsonRpcMessage`]: an incoming frame that is either a response (has `id`)
//!   or a notification (no `id`); used by transports to demultiplex reads.
//!
//! All types derive `Debug` so tests can `{:?}`-format them, and use
//! `skip_serializing_if` so optional fields (params/id) are omitted when empty
//! per the JSON-RPC spec.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The JSON-RPC protocol version literal every message carries.
pub const JSONRPC_VERSION: &str = "2.0";

/// A JSON-RPC 2.0 request (client -> server), correlated by `id`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: u64,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl JsonRpcRequest {
    /// Build a request for `method` with `id` and optional `params`.
    pub fn new(id: u64, method: impl Into<String>, params: Option<Value>) -> Self {
        JsonRpcRequest {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id,
            method: method.into(),
            params,
        }
    }
}

/// A JSON-RPC 2.0 notification (no `id`, no reply expected).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcNotification {
    pub jsonrpc: String,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl JsonRpcNotification {
    /// Build a notification for `method` with optional `params`.
    pub fn new(method: impl Into<String>, params: Option<Value>) -> Self {
        JsonRpcNotification {
            jsonrpc: JSONRPC_VERSION.to_string(),
            method: method.into(),
            params,
        }
    }
}

/// The `error` object of a failed JSON-RPC response.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// A JSON-RPC 2.0 response (server -> client), correlated back by `id`. Exactly
/// one of `result` / `error` is present on a well-formed response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

/// An incoming frame decoded from a transport: either a correlated
/// [`JsonRpcResponse`] (has an `id`) or a server-initiated
/// [`JsonRpcNotification`] (no `id`). Transports use this to route reads to the
/// pending-request table or the notification channel.
///
/// `untagged` so a raw incoming JSON object is classified by shape (presence of
/// `id`) without a discriminator field.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JsonRpcMessage {
    /// A response to a request we sent (has `id`). Ordered first so a frame with
    /// an `id` is decoded as a response, not a notification.
    Response(JsonRpcResponse),
    /// A server-initiated notification (no `id`).
    Notification(JsonRpcNotification),
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn request_omits_none_params() {
        let req = JsonRpcRequest::new(1, "tools/list", None);
        let s = serde_json::to_string(&req).unwrap();
        assert!(s.contains("\"jsonrpc\":\"2.0\""));
        assert!(s.contains("\"id\":1"));
        assert!(s.contains("\"method\":\"tools/list\""));
        assert!(!s.contains("params"));
    }

    #[test]
    fn incoming_response_is_classified_by_id() {
        let frame = json!({"jsonrpc": "2.0", "id": 7, "result": {"ok": true}});
        let msg: JsonRpcMessage = serde_json::from_value(frame).unwrap();
        match msg {
            JsonRpcMessage::Response(r) => {
                assert_eq!(r.id, 7);
                assert!(r.error.is_none());
                assert_eq!(r.result, Some(json!({"ok": true})));
            }
            JsonRpcMessage::Notification(_) => panic!("expected response"),
        }
    }

    #[test]
    fn incoming_notification_has_no_id() {
        let frame =
            json!({"jsonrpc": "2.0", "method": "notifications/message", "params": {"x": 1}});
        let msg: JsonRpcMessage = serde_json::from_value(frame).unwrap();
        match msg {
            JsonRpcMessage::Notification(n) => {
                assert_eq!(n.method, "notifications/message");
                assert_eq!(n.params, Some(json!({"x": 1})));
            }
            JsonRpcMessage::Response(_) => panic!("expected notification"),
        }
    }

    #[test]
    fn error_response_round_trips() {
        let resp = JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: 3,
            result: None,
            error: Some(JsonRpcError {
                code: -32601,
                message: "method not found".to_string(),
                data: None,
            }),
        };
        let s = serde_json::to_string(&resp).unwrap();
        let back: JsonRpcResponse = serde_json::from_str(&s).unwrap();
        assert_eq!(back.id, 3);
        assert_eq!(back.error.unwrap().code, -32601);
    }
}
