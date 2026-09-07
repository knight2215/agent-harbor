//! Core events emitted to the shell (architecture.md Sections 7.3 / 7.4 / 8,
//! with event hygiene per 9.1 / 9.2).
//!
//! [`CoreEvent`] is the single, serde-tagged event enum forwarded across the
//! Tauri IPC bridge to the frontend. It is serialized as an internally tagged
//! union (`{ "type": "messageDelta", ... }`) with `rename_all = "camelCase"`,
//! so the TypeScript mirror is a discriminated union on `type`.
//!
//! EVENT HYGIENE (Section 9.1 / 9.2): payloads carry ONLY display-safe data -
//! ids, text deltas, statuses, and human-readable rationales. They NEVER carry
//! secret material, `SecretRef` handles, raw provider responses, or credentials.
//!
//! The variants map to the surfaces in Section 8:
//!   - chat:        messageStarted, messageDelta, messageComplete, messageError, permissionRequested
//!   - history:     conversationUpdated, conversationCreated, conversationDeleted
//!   - MCP manager: mcpStateChanged, mcpError
//!   - selectors:   providersChanged, personasChanged
//!
//! Most variants are now constructed by the pipeline and the tauri-app event
//! bridge (message/conversation-update events by the streaming pipeline, MCP
//! and permission events by the tauri-app commands). The four still-unemitted
//! lifecycle/selector variants (`ConversationCreated`, `ConversationDeleted`,
//! `ProvidersChanged`, `PersonasChanged`) carry a per-variant
//! `#[allow(dead_code)]` to keep clippy `-D warnings` clean until the surfaces
//! that emit them land; drop each one as its emitter is wired.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::models::{MessageStatus, PermissionMode, Role, RouteMetadata, TokenUsage};

/// Connection state of an MCP server, reported to the Tool/MCP manager surface
/// (Section 8.3). Display-safe; carries no secrets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum McpConnectionState {
    Connecting,
    Connected,
    Disconnected,
}

/// Events emitted by the core to the frontend (architecture.md Section 8).
///
/// Exactly twelve variants, serialized as a `type`-tagged camelCase union.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum CoreEvent {
    /// A message was created and is about to stream (Section 7.4). Emitted BEFORE
    /// the first [`CoreEvent::MessageDelta`] for both the persisted user message
    /// (`role: User`, already `Complete`) and the assistant reply (`role:
    /// Assistant`, `streaming`), so the frontend can seed a placeholder keyed on
    /// `messageId` before any delta arrives. Without it the deltas reference an
    /// id the store has never seen and are dropped.
    #[serde(rename_all = "camelCase")]
    MessageStarted {
        conversation_id: Uuid,
        message_id: Uuid,
        role: Role,
    },
    /// A chunk of streamed assistant output (Section 7.4). `delta` is the new
    /// text appended to the message identified by `messageId`.
    #[serde(rename_all = "camelCase")]
    MessageDelta {
        conversation_id: Uuid,
        message_id: Uuid,
        delta: String,
    },
    /// A message finished streaming; final status, route, and usage are
    /// attached (Section 7.4).
    #[serde(rename_all = "camelCase")]
    MessageComplete {
        conversation_id: Uuid,
        message_id: Uuid,
        status: MessageStatus,
        route: Option<RouteMetadata>,
        usage: Option<TokenUsage>,
    },
    /// A message failed. `message` is a display-safe error string (never a raw
    /// provider response or credentials).
    #[serde(rename_all = "camelCase")]
    MessageError {
        conversation_id: Uuid,
        message_id: Uuid,
        message: String,
    },
    /// A conversation's metadata changed (title, tags, route pin, persona).
    #[serde(rename_all = "camelCase")]
    ConversationUpdated { conversation_id: Uuid },
    /// A new conversation was created.
    #[allow(dead_code)] // no emitter wired yet (conversation lifecycle surface)
    #[serde(rename_all = "camelCase")]
    ConversationCreated { conversation_id: Uuid },
    /// A conversation was deleted.
    #[allow(dead_code)] // no emitter wired yet (conversation lifecycle surface)
    #[serde(rename_all = "camelCase")]
    ConversationDeleted { conversation_id: Uuid },
    /// An MCP server's connection state or tool list changed (Section 8.3).
    #[serde(rename_all = "camelCase")]
    McpStateChanged {
        server_id: Uuid,
        state: McpConnectionState,
    },
    /// An MCP server reported an error. `message` is display-safe.
    #[serde(rename_all = "camelCase")]
    McpError { server_id: Uuid, message: String },
    /// A tool invocation requires user approval in `Ask` mode (Section 9.4).
    /// The frontend resolves it via `resolve_permission(requestId, decision)`.
    #[serde(rename_all = "camelCase")]
    PermissionRequested {
        request_id: Uuid,
        server_id: Uuid,
        tool_name: String,
        /// The server's configured permission mode, for display context.
        mode: PermissionMode,
        /// Human-readable rationale / summary of what the tool would do.
        rationale: String,
    },
    /// The set or availability of providers/models changed (Section 8.2).
    #[allow(dead_code)] // no emitter wired yet (provider/model selector surface)
    ProvidersChanged,
    /// The set of personas changed (Section 8.4).
    #[allow(dead_code)] // no emitter wired yet (persona selector surface)
    PersonasChanged,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_serialize_as_camel_case_tagged_union() {
        let ev = CoreEvent::MessageDelta {
            conversation_id: Uuid::nil(),
            message_id: Uuid::nil(),
            delta: "hi".to_string(),
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains("\"type\":\"messageDelta\""));
        assert!(json.contains("\"conversationId\""));
        assert!(json.contains("\"messageId\""));

        // Unit-like variant still tags on `type`.
        let json = serde_json::to_string(&CoreEvent::ProvidersChanged).unwrap();
        assert_eq!(json, "{\"type\":\"providersChanged\"}");
    }

    #[test]
    fn message_started_round_trips_camel_case() {
        let ev = CoreEvent::MessageStarted {
            conversation_id: Uuid::nil(),
            message_id: Uuid::nil(),
            role: Role::Assistant,
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains("\"type\":\"messageStarted\""));
        assert!(json.contains("\"conversationId\""));
        assert!(json.contains("\"messageId\""));
        assert!(json.contains("\"role\":\"assistant\""));
        let back: CoreEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, CoreEvent::MessageStarted { .. }));
    }

    #[test]
    fn permission_requested_round_trips() {
        let ev = CoreEvent::PermissionRequested {
            request_id: Uuid::nil(),
            server_id: Uuid::nil(),
            tool_name: "fs/read".to_string(),
            mode: PermissionMode::Ask,
            rationale: "read a file".to_string(),
        };
        let json = serde_json::to_string(&ev).unwrap();
        let back: CoreEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, CoreEvent::PermissionRequested { .. }));
    }
}
