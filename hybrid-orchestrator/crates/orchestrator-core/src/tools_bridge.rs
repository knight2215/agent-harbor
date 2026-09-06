//! Function-calling bridge between the model and the MCP client (architecture.md
//! Sections 5.4 / 5.5, P3.4; plus the orchestrator side of the Section 5.6
//! error handling, P3.6).
//!
//! The [`ToolBridge`] owns the set of connected [`McpServerHandle`]s for a turn
//! and drives the Section 5.5 sequence:
//!
//! ```text
//! attach_tools()  -> populate ChatRequest.tools from visible_tools()
//! model emits tool_call { name: "<serverId>__<tool>", arguments: "<json string>" }
//! handle_tool_call():
//!   1. resolve the namespaced name -> owning server + bare tool
//!   2. validate the decoded arguments against the tool input JSON Schema
//!   3. permission check (Ask/Allow/Deny) via the PermissionGate
//!   4. invoke via the handle, apply timeout / output cap (in mcp-client)
//!   5. normalize the result into a `tool` role ChatMessage
//! ```
//!
//! FAILURE HANDLING (Section 5.6, P3.6): an unknown server, a schema-validation
//! failure, a denied permission, an invocation/transport error, a timeout, or a
//! server-returned error are ALL captured as a structured `tool` message whose
//! JSON content carries `isError: true`. They NEVER surface as a returned `Err`
//! that would crash the turn. Server-level plumbing errors are additionally
//! surfaced on the core event channel as [`CoreEvent::McpError`] for the MCP
//! manager surface (Section 8.3).
//!
//! Tools from `Disconnected` servers are already excluded by
//! [`McpServerHandle::visible_tools`], so [`ToolBridge::attach_tools`] naturally
//! hides them (Section 5.6 "tools from a disconnected server are hidden").

use std::collections::HashMap;
use std::sync::Arc;

use mcp_client::{resolve_namespaced, McpServerHandle};
use providers::{ChatMessage, ChatRequest, MessageRole, ToolCall, ToolSpec};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::events::CoreEvent;
use crate::models::ToolResult;
use crate::permission::{PermissionGate, PermissionOutcome, PermissionRegistry};

use tokio::sync::mpsc::UnboundedSender;

/// A tool call bridged between the model and the MCP client for one turn.
///
/// Holds the connected server handles (keyed by their server-`Uuid` namespace),
/// the [`PermissionGate`], and the core event sender for `McpError` surfacing.
pub struct ToolBridge {
    /// Connected server handles keyed by server id (the namespace used by
    /// `mcp-client`; see [`McpServerHandle::namespace`]).
    servers: HashMap<Uuid, Arc<McpServerHandle>>,
    gate: PermissionGate,
    events: UnboundedSender<CoreEvent>,
}

impl ToolBridge {
    /// Build a bridge over the given connected server handles, a permission
    /// gate, and the core event channel.
    pub fn new(
        servers: impl IntoIterator<Item = Arc<McpServerHandle>>,
        gate: PermissionGate,
        events: UnboundedSender<CoreEvent>,
    ) -> Self {
        let servers = servers
            .into_iter()
            .map(|h| (h.config().id, h))
            .collect::<HashMap<_, _>>();
        ToolBridge {
            servers,
            gate,
            events,
        }
    }

    /// The permission registry the gate resolves against (so the shell can wire
    /// `resolve_permission` to the same instance).
    pub fn permission_registry(&self) -> &PermissionRegistry {
        self.gate.registry()
    }

    /// Collect the currently-visible tools across all connected servers as
    /// [`providers::ToolSpec`]s and set them on `request.tools` (P3.4a).
    ///
    /// Only tools from `Connected` servers appear (Section 5.6): a
    /// `Disconnected` server's [`McpServerHandle::visible_tools`] returns empty.
    /// Capability negotiation (whether the target model supports tools at all)
    /// is handled elsewhere; this only populates the field.
    pub async fn attach_tools(&self, request: &mut ChatRequest) {
        let mut specs: Vec<ToolSpec> = Vec::new();
        for handle in self.servers.values() {
            specs.extend(handle.visible_tools().await);
        }
        request.tools = specs;
    }

    /// Resolve, permission-check, invoke, and normalize a single model
    /// [`ToolCall`] into a `tool` role [`ChatMessage`] (P3.4b-d, Section 5.5).
    ///
    /// This NEVER returns an `Err`: every failure mode is captured as a
    /// structured tool-error message (P3.6) so the model can recover and the
    /// turn continues. `call.function.name` is the namespaced tool name and
    /// `call.function.arguments` is a JSON STRING (OpenAI encoding).
    pub async fn handle_tool_call(&self, call: &ToolCall) -> ChatMessage {
        let namespaced = call.function.name.as_str();

        // 1. Resolve the namespaced name back to (server, tool).
        let (server_id, handle, bare_tool) = match self.resolve(namespaced) {
            Ok(triple) => triple,
            Err(message) => return tool_error_message(&call.id, &message),
        };

        // 2. Decode the JSON-string arguments and validate them against the
        //    tool's cached input schema (P3.4c).
        let arguments = match decode_arguments(&call.function.arguments) {
            Ok(args) => args,
            Err(message) => return tool_error_message(&call.id, &message),
        };
        if let Some(schema) = self.input_schema(&handle, bare_tool).await {
            if let Err(message) = validate_arguments(&schema, &arguments) {
                return tool_error_message(&call.id, &message);
            }
        }

        // 3. Permission check (Ask/Allow/Deny) before touching the server.
        let rationale = format!("invoke `{bare_tool}` on MCP server {server_id}");
        let outcome = self
            .gate
            .check(
                server_id,
                bare_tool,
                handle.config().permission_mode,
                // Per-tool overrides are not yet modeled on McpServerConfig;
                // pass an empty map so only the server default applies. Wired to
                // a real overrides map when the config surface gains it.
                &HashMap::new(),
                &rationale,
            )
            .await;
        if let PermissionOutcome::Deny { reason } = outcome {
            return tool_error_message(&call.id, &reason);
        }

        // 4. Invoke. mcp-client applies the per-tool timeout + output cap and
        //    returns a structured ToolResult (is_error set) for tool-level
        //    failures; a lifecycle Err (e.g. NotConnected) is turned into a
        //    structured tool-error message here and surfaced as McpError.
        let result = match handle.invoke(call.id.as_str(), bare_tool, arguments).await {
            Ok(result) => result,
            Err(err) => {
                self.emit_mcp_error(server_id, &err.to_string());
                return tool_error_message(&call.id, &err.to_string());
            }
        };

        // 5. Normalize the ToolResult into a `tool` role message.
        tool_result_message(result)
    }

    /// Resolve a namespaced tool name to its owning server id, handle, and bare
    /// tool name (P3.4b). Returns a display-safe error string on failure.
    fn resolve<'a>(
        &self,
        namespaced: &'a str,
    ) -> Result<(Uuid, Arc<McpServerHandle>, &'a str), String> {
        let (ns, bare_tool) = resolve_namespaced(namespaced)
            .ok_or_else(|| format!("tool name `{namespaced}` is not namespaced (missing `__`)"))?;
        let server_id = Uuid::parse_str(ns)
            .map_err(|_| format!("tool namespace `{ns}` is not a known server id"))?;
        let handle = self
            .servers
            .get(&server_id)
            .cloned()
            .ok_or_else(|| format!("no connected MCP server for tool `{namespaced}`"))?;
        Ok((server_id, handle, bare_tool))
    }

    /// The cached input JSON Schema for `bare_tool` on `handle`, if the server
    /// advertised the tool via `tools/list`. `None` when the tool is unknown to
    /// the cache (validation is then skipped and the invoke path reports any
    /// unknown-tool error structurally).
    async fn input_schema(&self, handle: &McpServerHandle, bare_tool: &str) -> Option<Value> {
        handle
            .tools()
            .await
            .into_iter()
            .find(|d| d.name == bare_tool)
            .map(|d| d.input_schema)
    }

    /// Surface a server-level plumbing error on the core event channel
    /// (Section 8.3). Best-effort: a closed channel is ignored.
    fn emit_mcp_error(&self, server_id: Uuid, message: &str) {
        let _ = self.events.send(CoreEvent::McpError {
            server_id,
            message: message.to_string(),
        });
    }
}

/// Decode the OpenAI-encoded JSON-STRING tool arguments into a [`Value`]. An
/// empty/whitespace string means "no arguments" and decodes to an empty object.
/// Returns a display-safe error string when the string is not valid JSON.
fn decode_arguments(arguments: &str) -> Result<Value, String> {
    if arguments.trim().is_empty() {
        Ok(json!({}))
    } else {
        serde_json::from_str(arguments)
            .map_err(|e| format!("tool arguments are not valid JSON: {e}"))
    }
}

/// Lightweight structural validation of decoded `arguments` against a tool input
/// `schema` (P3.4c / Section 9.4 "argument validation"):
///
/// - if the schema declares `"type": "object"` (or omits a type but has
///   `properties`), `arguments` MUST be a JSON object;
/// - every name in the schema's `required` array MUST be present;
/// - for each present property that the schema types, the value's JSON type
///   MUST match (`string`/`number`/`integer`/`boolean`/`object`/`array`).
///
/// Unknown/extra properties are permitted (open-world, matching typical MCP
/// tools). This is deliberately shallow: it is a guard against malformed or
/// injected payloads, not a full JSON Schema implementation, and adds no
/// dependency.
pub fn validate_arguments(schema: &Value, arguments: &Value) -> Result<(), String> {
    let declares_object = schema.get("type").and_then(Value::as_str) == Some("object")
        || (schema.get("type").is_none() && schema.get("properties").is_some());

    if declares_object {
        let obj = match arguments.as_object() {
            Some(obj) => obj,
            None => return Err("arguments must be a JSON object".to_string()),
        };

        // Required properties must all be present.
        if let Some(required) = schema.get("required").and_then(Value::as_array) {
            for req in required {
                if let Some(name) = req.as_str() {
                    if !obj.contains_key(name) {
                        return Err(format!("missing required argument `{name}`"));
                    }
                }
            }
        }

        // Declared property types must match for present values.
        if let Some(props) = schema.get("properties").and_then(Value::as_object) {
            for (name, value) in obj {
                if let Some(prop_schema) = props.get(name) {
                    if let Some(expected) = prop_schema.get("type").and_then(Value::as_str) {
                        if !json_type_matches(expected, value) {
                            return Err(format!(
                                "argument `{name}` should be of type `{expected}`"
                            ));
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

/// Whether `value`'s JSON type matches the schema `expected` type name.
fn json_type_matches(expected: &str, value: &Value) -> bool {
    match expected {
        "string" => value.is_string(),
        // `integer` requires a whole number; `number` accepts any numeric.
        "integer" => value.is_i64() || value.is_u64(),
        "number" => value.is_number(),
        "boolean" => value.is_boolean(),
        "object" => value.is_object(),
        "array" => value.is_array(),
        "null" => value.is_null(),
        // Unknown/compound type keywords are not enforced.
        _ => true,
    }
}

/// Build a `tool` role [`ChatMessage`] from a structured [`ToolResult`]
/// (P3.4d): `tool_call_id` is set to the originating call id and `content` is
/// the serialized result payload so the model can consume it.
fn tool_result_message(result: ToolResult) -> ChatMessage {
    // Serialize the result payload as compact JSON text for the `tool` message
    // body. On the rare chance serialization fails, fall back to a marker.
    let content = serde_json::to_string(&result.content)
        .unwrap_or_else(|_| "{\"error\":\"failed to serialize tool result\"}".to_string());
    ChatMessage {
        role: MessageRole::Tool,
        content: Some(content),
        tool_calls: Vec::new(),
        tool_call_id: Some(result.call_id),
        name: None,
    }
}

/// Build a structured tool-error `tool` message (P3.6): the JSON content carries
/// `isError: true` and a display-safe message so the model can recover rather
/// than the turn crashing.
fn tool_error_message(call_id: &str, message: &str) -> ChatMessage {
    let content = json!({ "isError": true, "error": message }).to_string();
    ChatMessage {
        role: MessageRole::Tool,
        content: Some(content),
        tool_calls: Vec::new(),
        tool_call_id: Some(call_id.to_string()),
        name: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn validate_arguments_requires_object_and_required_props() {
        let schema = json!({
            "type": "object",
            "properties": { "path": { "type": "string" } },
            "required": ["path"],
        });
        // Missing required property -> error.
        let err = validate_arguments(&schema, &json!({})).unwrap_err();
        assert!(err.contains("path"));
        // Present + correct type -> ok.
        assert!(validate_arguments(&schema, &json!({ "path": "/etc" })).is_ok());
        // Non-object arguments -> error.
        assert!(validate_arguments(&schema, &json!("nope")).is_err());
    }

    #[test]
    fn validate_arguments_checks_declared_types() {
        let schema = json!({
            "type": "object",
            "properties": { "count": { "type": "integer" } },
        });
        assert!(validate_arguments(&schema, &json!({ "count": 3 })).is_ok());
        // Wrong type for a declared property -> error.
        let err = validate_arguments(&schema, &json!({ "count": "three" })).unwrap_err();
        assert!(err.contains("count"));
        // Extra/unknown properties are allowed (open-world).
        assert!(validate_arguments(&schema, &json!({ "count": 1, "extra": true })).is_ok());
    }

    #[test]
    fn validate_arguments_ignores_non_object_schema() {
        // A schema without object typing imposes no structural constraint.
        let schema = json!({});
        assert!(validate_arguments(&schema, &json!("anything")).is_ok());
    }

    #[test]
    fn tool_result_message_sets_tool_role_and_call_id() {
        let result = ToolResult {
            call_id: "call_1".to_string(),
            content: json!({ "ok": true }),
            is_error: false,
        };
        let msg = tool_result_message(result);
        assert_eq!(msg.role, MessageRole::Tool);
        assert_eq!(msg.tool_call_id.as_deref(), Some("call_1"));
        assert!(msg.content.as_deref().unwrap().contains("\"ok\":true"));
    }

    #[test]
    fn tool_error_message_marks_is_error() {
        let msg = tool_error_message("call_2", "boom");
        assert_eq!(msg.role, MessageRole::Tool);
        assert_eq!(msg.tool_call_id.as_deref(), Some("call_2"));
        let content = msg.content.unwrap();
        assert!(content.contains("\"isError\":true"));
        assert!(content.contains("boom"));
    }
}
