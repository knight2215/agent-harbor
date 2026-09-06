//! Domain models (architecture.md Section 7.1, plus ProviderConfig 4.2 and
//! ManualRoute/PrivacyTag/RouteSource 6.1).
//!
//! These are the authoritative Rust DTOs and they live in the leaf `domain`
//! crate so BOTH `orchestrator-core` and `persistence` can depend on them
//! DOWNWARD (architecture.md Section 3.3) without forming a dependency cycle.
//! `orchestrator-core` re-exports every type here at its crate root, so
//! `orchestrator_core::Conversation` (etc.) keeps resolving.
//!
//! Every type derives `serde::Serialize` + `serde::Deserialize` with
//! `rename_all = "camelCase"` so the JSON that crosses the Tauri IPC boundary
//! matches the hand-mirrored TypeScript types in
//! `frontend/src/types/index.ts`. When you change a field here, update the TS
//! mirror in the same change (there is no codegen; the two are kept in sync by
//! hand, per the Phase 0 convention).
//!
//! `SecretRef` note: architecture.md Section 3.3 places the canonical
//! `SecretRef` in the `secrets` crate, and permits crates above it to depend on
//! `secrets`. The canonical, serde-serializable `SecretRef` lives in the
//! `secrets` crate and is RE-EXPORTED here (see [`SecretRef`]), so there is a
//! single source of truth. The dependency direction stays correct
//! (`domain -> secrets`, never the reverse). The invariant is unchanged:
//! `SecretRef` is only an opaque keystore handle, never the secret.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// Re-export of the canonical [`secrets::SecretRef`] (architecture.md Section
/// 3.3). Never contains secret material; the raw key is resolved against the
/// keystore inside the core at call time and never crosses IPC.
pub use secrets::SecretRef;

/// A conversation and its per-conversation settings (Section 7.1).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Conversation {
    pub id: Uuid,
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub persona_id: Option<Uuid>,
    /// Per-conversation model pin.
    pub conversation_pref: Option<ManualRoute>,
    pub privacy_tags: Vec<PrivacyTag>,
    /// Which MCP servers are active in this conversation.
    pub enabled_tool_servers: Vec<Uuid>,
}

/// A single message within a conversation (Section 7.1).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub id: Uuid,
    pub conversation_id: Uuid,
    pub role: Role,
    pub content: MessageContent,
    pub created_at: DateTime<Utc>,
    /// Which provider/model answered plus the routing rationale.
    pub route: Option<RouteMetadata>,
    pub usage: Option<TokenUsage>,
    pub status: MessageStatus,
}

/// Message author role (Section 7.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

/// Lifecycle status of a message (Section 7.1 / 7.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MessageStatus {
    Pending,
    Streaming,
    Complete,
    Error,
}

/// The body of a message: plain text, tool calls, tool results, or attachments
/// (Section 7.1: "text, tool calls, tool results, attachments").
///
/// Serialized as an internally tagged enum so the TypeScript mirror is a
/// discriminated union on a `type` field.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum MessageContent {
    /// Plain text content.
    Text { text: String },
    /// One or more tool/function calls requested by the assistant.
    ToolCalls { calls: Vec<ToolCall> },
    /// Results returned from invoking tools.
    ToolResults { results: Vec<ToolResult> },
    /// Non-text attachments (images, files) carried with the message.
    Attachments { attachments: Vec<Attachment> },
}

/// A single tool/function call requested by the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCall {
    /// Provider-assigned call id, used to correlate the matching result.
    pub id: String,
    /// Fully-qualified tool name (typically `serverId/toolName`).
    pub name: String,
    /// Arguments as provider-supplied JSON.
    pub arguments: Value,
}

/// The result of invoking a tool, correlated back to a [`ToolCall`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResult {
    /// Matches [`ToolCall::id`].
    pub call_id: String,
    /// Result payload as JSON.
    pub content: Value,
    /// Whether the invocation failed.
    pub is_error: bool,
}

/// A non-text attachment carried with a message.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    /// MIME type, e.g. `image/png`.
    pub mime_type: String,
    /// Optional original filename.
    pub name: Option<String>,
    /// Location handle or inline data URI, depending on transport.
    pub uri: String,
}

/// Which provider/model answered a message and why (Section 7.1 / 6.1).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteMetadata {
    pub provider_id: String,
    pub model: String,
    /// Human-readable explanation shown in the UI ("why this model").
    pub rationale: String,
    pub source: RouteSource,
}

/// How a route was decided (Section 6.1 / 6.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RouteSource {
    /// Per-message manual override.
    Manual,
    /// Per-conversation pin.
    ConversationPin,
    /// Automatic routing policy.
    Automatic,
}

/// Token accounting for a completed message (Section 7.1 / 7.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

/// A saved agent persona (Section 7.1).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPersona {
    pub id: Uuid,
    pub name: String,
    pub system_prompt: String,
    /// Preferred provider/model.
    pub default_route: Option<ManualRoute>,
    /// Coarse routing preference, e.g. prefer-local / prefer-quality.
    pub routing_hint: Option<RoutingHint>,
    pub allowed_tool_servers: Vec<Uuid>,
    pub parameters: ModelParameters,
}

/// Model sampling / generation parameters attached to a persona (Section 7.1).
///
/// All fields are optional so older configs keep working and the provider's own
/// defaults apply when unset (Section 10.4 additive-change bias).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelParameters {
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    pub top_p: Option<f32>,
    pub frequency_penalty: Option<f32>,
    pub presence_penalty: Option<f32>,
    pub stop: Option<Vec<String>>,
}

/// Coarse routing preference expressed by a persona (Section 7.1: "prefer-local,
/// prefer-quality"). Extensible.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RoutingHint {
    PreferLocal,
    PreferQuality,
    PreferCheap,
    PreferSpeed,
}

/// A data-handling constraint tag on a conversation or message (Section 6.1 /
/// 6.2). `LocalOnly` and `Confidential` are hard constraints that force local
/// routing. Extensible.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PrivacyTag {
    /// MUST be routed to a local provider (LM Studio); never to the cloud.
    LocalOnly,
    /// Sensitive data; treated as a hard local-only constraint.
    Confidential,
    /// Escape hatch for user-defined tags without a schema change.
    Custom(String),
}

/// A manual provider/model selection (Section 6.1 / 6.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManualRoute {
    pub provider_id: String,
    pub model: String,
}

/// Configuration for a single MCP tool server (Section 7.1).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerConfig {
    pub id: Uuid,
    pub name: String,
    pub transport: McpTransport,
    pub permission_mode: PermissionMode,
    pub enabled: bool,
}

/// How the core connects to an MCP server (Section 7.1: stdio / HTTP-SSE).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum McpTransport {
    /// Spawn a local process and speak MCP over stdio.
    Stdio {
        command: String,
        args: Vec<String>,
        env: Vec<(String, String)>,
    },
    /// Connect to a remote MCP server over HTTP + SSE.
    HttpSse {
        url: String,
        headers: Vec<(String, String)>,
    },
}

/// Permission gate applied to an MCP server's tool invocations (Section 7.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PermissionMode {
    /// Prompt the user for each invocation.
    Ask,
    /// Auto-approve invocations.
    Allow,
    /// Block invocations.
    Deny,
}

/// Provider adapter construction config (Section 4.2). `api_key_ref` is a
/// keystore handle, not the key; the raw key never lives here and never crosses
/// IPC.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConfig {
    pub id: String,
    pub kind: ProviderKind,
    /// Swappable endpoint; defaults per kind when `None`.
    pub base_url: Option<String>,
    /// Reference into the keystore, not the key itself.
    pub api_key_ref: Option<SecretRef>,
    /// Provider-specific extras (Azure deployment, Bedrock region, etc.).
    #[serde(default)]
    pub extra: Value,
}

/// The supported provider families (Section 4.2 / 4.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProviderKind {
    OpenAI,
    Anthropic,
    Bedrock,
    Gemini,
    Azure,
    LmStudio,
    GenericOpenAI,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_kind_serializes_camel_case() {
        // Guards the camelCase contract the TS mirror relies on.
        let json = serde_json::to_string(&ProviderKind::LmStudio).unwrap();
        assert_eq!(json, "\"lmStudio\"");
        let json = serde_json::to_string(&ProviderKind::GenericOpenAI).unwrap();
        assert_eq!(json, "\"genericOpenAI\"");
    }

    #[test]
    fn conversation_round_trips() {
        let now = Utc::now();
        let conv = Conversation {
            id: Uuid::new_v4(),
            title: "Test".to_string(),
            created_at: now,
            updated_at: now,
            persona_id: None,
            conversation_pref: Some(ManualRoute {
                provider_id: "openai".to_string(),
                model: "gpt-4o".to_string(),
            }),
            privacy_tags: vec![PrivacyTag::LocalOnly],
            enabled_tool_servers: vec![Uuid::new_v4()],
        };
        let json = serde_json::to_string(&conv).unwrap();
        // Field names must be camelCase for the TS mirror.
        assert!(json.contains("\"createdAt\""));
        assert!(json.contains("\"conversationPref\""));
        assert!(json.contains("\"enabledToolServers\""));
        let back: Conversation = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, conv.id);
        assert_eq!(back.title, conv.title);
    }

    #[test]
    fn message_content_is_tagged_union() {
        let content = MessageContent::Text {
            text: "hi".to_string(),
        };
        let json = serde_json::to_string(&content).unwrap();
        assert_eq!(json, "{\"type\":\"text\",\"text\":\"hi\"}");
    }

    #[test]
    fn provider_config_hides_key_material() {
        let cfg = ProviderConfig {
            id: "openai".to_string(),
            kind: ProviderKind::OpenAI,
            base_url: None,
            api_key_ref: Some(SecretRef("keychain://openai".to_string())),
            extra: Value::Null,
        };
        let json = serde_json::to_string(&cfg).unwrap();
        assert!(json.contains("\"apiKeyRef\""));
        // Only the opaque handle is present, never raw key material.
        assert!(json.contains("keychain://openai"));
    }
}
