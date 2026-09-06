//! Tool discovery and schema mapping (architecture.md Section 5.4, P3.3).
//!
//! MCP servers expose tools via `tools/list`, each described by a name, an
//! optional human description, and a JSON Schema for its input. This module:
//!
//! - deserializes those descriptors into [`ToolDescriptor`];
//! - namespaces tool names by server as `{server}__{tool}` (double underscore,
//!   per the Section 5.4 example `filesystem__read_file`) so two servers can
//!   expose the same bare tool name without colliding;
//! - converts a [`ToolDescriptor`] into a [`providers::ToolSpec`] for
//!   `ChatRequest.tools` via [`providers::ToolSpec::function`]; and
//! - reverse-resolves a namespaced name back into `(server, tool)`.

use providers::ToolSpec;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The double-underscore separator between a server namespace and a bare tool
/// name (Section 5.4: `filesystem__read_file`).
pub const NAMESPACE_SEPARATOR: &str = "__";

/// An MCP tool descriptor as returned by `tools/list`.
///
/// `inputSchema` is the wire field name in the MCP spec; the camelCase rename
/// keeps deserialization matching the server payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDescriptor {
    /// The server-local (bare) tool name, e.g. `read_file`.
    pub name: String,
    /// Optional human-readable description shown to the model.
    #[serde(default)]
    pub description: String,
    /// JSON Schema describing the tool's arguments. Defaults to an empty object
    /// schema when the server omits it.
    #[serde(default = "empty_object_schema")]
    pub input_schema: Value,
}

fn empty_object_schema() -> Value {
    serde_json::json!({ "type": "object" })
}

impl ToolDescriptor {
    /// Build the namespaced tool name for this descriptor under `server`
    /// (`{server}__{name}`).
    pub fn namespaced_name(&self, server: &str) -> String {
        namespace_tool(server, &self.name)
    }

    /// Convert this descriptor into a [`providers::ToolSpec`] for
    /// `ChatRequest.tools`, using the namespaced name so tool calls resolve back
    /// to the owning server.
    pub fn to_tool_spec(&self, server: &str) -> ToolSpec {
        ToolSpec::function(
            self.namespaced_name(server),
            self.description.clone(),
            self.input_schema.clone(),
        )
    }
}

/// Compose a namespaced tool name from a server namespace and a bare tool name.
pub fn namespace_tool(server: &str, tool: &str) -> String {
    format!("{server}{NAMESPACE_SEPARATOR}{tool}")
}

/// Reverse-resolve a namespaced tool name into `(server, tool)`.
///
/// COLLISION / SEPARATOR HANDLING: a server *name* may itself contain the `__`
/// separator, which would make a naive split ambiguous. Rather than guess, the
/// namespace is always the substring up to the FIRST `__` and the tool is
/// everything after it. Callers that need exact round-tripping should namespace
/// by the server's opaque id (which never contains `__`) rather than its
/// display name; [`crate::McpServerHandle`] namespaces by id for this reason.
/// Returns `None` when the name contains no separator (not a namespaced name).
pub fn resolve_namespaced(name: &str) -> Option<(&str, &str)> {
    name.split_once(NAMESPACE_SEPARATOR)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn descriptor(name: &str) -> ToolDescriptor {
        ToolDescriptor {
            name: name.to_string(),
            description: format!("does {name}"),
            input_schema: json!({"type": "object", "properties": {"path": {"type": "string"}}}),
        }
    }

    #[test]
    fn descriptor_deserializes_camel_case_input_schema() {
        let value = json!({
            "name": "read_file",
            "description": "read a file",
            "inputSchema": {"type": "object"}
        });
        let d: ToolDescriptor = serde_json::from_value(value).unwrap();
        assert_eq!(d.name, "read_file");
        assert_eq!(d.input_schema, json!({"type": "object"}));
    }

    #[test]
    fn descriptor_defaults_missing_schema_to_object() {
        let value = json!({"name": "noop"});
        let d: ToolDescriptor = serde_json::from_value(value).unwrap();
        assert_eq!(d.description, "");
        assert_eq!(d.input_schema, json!({"type": "object"}));
    }

    #[test]
    fn namespacing_uses_double_underscore() {
        let d = descriptor("read_file");
        assert_eq!(d.namespaced_name("filesystem"), "filesystem__read_file");
    }

    #[test]
    fn to_tool_spec_uses_namespaced_name() {
        let d = descriptor("read_file");
        let spec = d.to_tool_spec("filesystem");
        assert_eq!(spec.function.name, "filesystem__read_file");
        assert_eq!(spec.function.description, "does read_file");
    }

    #[test]
    fn reverse_resolution_splits_on_first_separator() {
        assert_eq!(
            resolve_namespaced("filesystem__read_file"),
            Some(("filesystem", "read_file"))
        );
        // A bare (un-namespaced) name has no separator.
        assert_eq!(resolve_namespaced("read_file"), None);
        // Only the FIRST separator delimits the namespace; tool names keep the
        // rest verbatim.
        assert_eq!(resolve_namespaced("srv__a__b"), Some(("srv", "a__b")));
    }
}
