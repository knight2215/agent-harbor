//! The internal `ChatProvider` contract (architecture.md Section 4.1).
//!
//! Every provider adapter implements [`ChatProvider`], an async trait modeled on
//! the OpenAI Chat Completions shape because it is the widest common denominator
//! across providers and exactly what LM Studio serves natively. The request /
//! response / delta / tool types below are all OpenAI-compatible internally;
//! translation-shim adapters (Anthropic, Gemini, Bedrock) convert to and from
//! their native formats at the edges (Section 4.3).

use async_trait::async_trait;
use futures_util::stream::BoxStream;
use serde::{Deserialize, Serialize};

/// Per-model capability descriptor (architecture.md Section 4.1 / 4.4).
///
/// The pipeline consults this before building a request: gating tools, falling
/// back from streaming to non-streaming, gating vision / json_mode, and using
/// `max_context` for truncation decisions. It is also a hard routing constraint
/// (a request needing tools cannot be routed to a tool-incapable model).
///
/// This is the single canonical definition; `capability.rs` re-exports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    /// Server-sent streaming of deltas is supported.
    pub streaming: bool,
    /// Function / tool calling is supported.
    pub tools: bool,
    /// Image inputs are supported.
    pub vision: bool,
    /// Structured JSON output mode is supported.
    pub json_mode: bool,
    /// Maximum context window in tokens, if known.
    pub max_context: Option<u32>,
}

impl Default for Capabilities {
    /// A conservative default: text-only, non-streaming, no tools. Adapters
    /// override per model.
    fn default() -> Self {
        Capabilities {
            streaming: false,
            tools: false,
            vision: false,
            json_mode: false,
            max_context: None,
        }
    }
}

/// Author role of a chat message (OpenAI-shaped).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    System,
    User,
    Assistant,
    Tool,
}

/// A tool/function call the assistant requested (OpenAI-shaped `tool_calls`
/// entry).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    /// Provider-assigned call id, correlates with the matching tool result.
    pub id: String,
    /// Always `"function"` for OpenAI-compatible tool calls.
    #[serde(rename = "type", default = "default_tool_call_type")]
    pub kind: String,
    /// The called function's name and JSON-encoded arguments.
    pub function: FunctionCall,
}

fn default_tool_call_type() -> String {
    "function".to_string()
}

/// The `function` payload of a [`ToolCall`] (OpenAI-shaped).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    /// Arguments as a JSON string (OpenAI encodes these as a string, not an
    /// object, so partial streamed fragments can be concatenated).
    pub arguments: String,
}

/// A single chat message (OpenAI-shaped: role + content + optional tool fields).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: MessageRole,
    /// Message text. `None` for assistant messages that only carry tool calls.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// Tool/function calls requested by the assistant.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    /// For `tool` role messages: the id of the tool call this message answers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// Optional participant name (OpenAI `name` field).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl ChatMessage {
    /// Convenience constructor for a plain text message.
    pub fn text(role: MessageRole, content: impl Into<String>) -> Self {
        ChatMessage {
            role,
            content: Some(content.into()),
            tool_calls: Vec::new(),
            tool_call_id: None,
            name: None,
        }
    }
}

/// A function-calling tool schema exposed to the model (OpenAI-shaped, sourced
/// from MCP tool descriptors).
// Not `Eq`: `FunctionSpec::parameters` is a `serde_json::Value`, which is only
// `PartialEq`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolSpec {
    /// Always `"function"` for OpenAI-compatible tools.
    #[serde(rename = "type", default = "default_tool_call_type")]
    pub kind: String,
    pub function: FunctionSpec,
}

impl ToolSpec {
    /// Build a function tool spec from a name, description, and JSON-schema
    /// parameters object.
    pub fn function(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: serde_json::Value,
    ) -> Self {
        ToolSpec {
            kind: "function".to_string(),
            function: FunctionSpec {
                name: name.into(),
                description: description.into(),
                parameters,
            },
        }
    }
}

/// The `function` schema of a [`ToolSpec`].
// Not `Eq`: `parameters` is a `serde_json::Value`, which is only `PartialEq`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FunctionSpec {
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// JSON Schema describing the function's parameters.
    pub parameters: serde_json::Value,
}

/// A completion request in the internal OpenAI-compatible shape
/// (architecture.md Section 4.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatRequest {
    pub model: String,
    /// system/user/assistant/tool messages.
    pub messages: Vec<ChatMessage>,
    /// Function-calling tool schemas (from MCP). Empty when tools are unused or
    /// gated off by capability negotiation.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    /// Whether the caller wants a streamed response.
    #[serde(default)]
    pub stream: bool,
    /// Provider-specific passthrough (e.g. `response_format`, vendor extras).
    #[serde(default)]
    pub extra: serde_json::Value,
}

impl ChatRequest {
    /// Construct a minimal request for `model` with `messages` and default
    /// (unset) options.
    pub fn new(model: impl Into<String>, messages: Vec<ChatMessage>) -> Self {
        ChatRequest {
            model: model.into(),
            messages,
            tools: Vec::new(),
            temperature: None,
            max_tokens: None,
            stream: false,
            extra: serde_json::Value::Null,
        }
    }
}

/// Token accounting for a completion (OpenAI-shaped `usage`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

/// Why a completion stopped (OpenAI-shaped `finish_reason`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    Stop,
    Length,
    ToolCalls,
    ContentFilter,
}

/// One choice in a non-streaming completion (OpenAI-shaped).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatChoice {
    #[serde(default)]
    pub index: u32,
    pub message: ChatMessage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<FinishReason>,
}

/// A non-streaming completion response (OpenAI-shaped).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatResponse {
    pub choices: Vec<ChatChoice>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// An incremental tool-call fragment in a streamed delta (OpenAI-shaped
/// `delta.tool_calls` entry; fields arrive piecemeal).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ToolCallDelta {
    /// Index of the tool call this fragment belongs to.
    #[serde(default)]
    pub index: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub function_name: Option<String>,
    /// A fragment of the JSON-encoded arguments string, to be concatenated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments_fragment: Option<String>,
}

/// One incremental chunk of a streamed completion (architecture.md Section 4.1).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ChatDelta {
    /// Incremental assistant text, if any in this chunk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// Incremental reasoning / "thinking" text, if any in this chunk. Thinking
    /// models (via Ollama's OpenAI-compat `/v1` path) surface their chain of
    /// thought on a separate `reasoning_content` delta key, distinct from the
    /// final answer `content`. Mirrors the `content` field's attributes so a
    /// chunk carrying no thinking is byte-identical on the wire to today.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    /// Incremental tool-call fragments, if any in this chunk.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCallDelta>,
    /// Set on the terminal chunk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<FinishReason>,
}

/// Metadata about a model a provider offers (architecture.md Section 4.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelInfo {
    /// Provider-scoped model id, e.g. `gpt-4o`.
    pub id: String,
    /// Optional human-friendly display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// Context window in tokens, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_length: Option<u32>,
}

impl ModelInfo {
    /// Construct a minimal `ModelInfo` from just an id.
    pub fn new(id: impl Into<String>) -> Self {
        ModelInfo {
            id: id.into(),
            display_name: None,
            context_length: None,
        }
    }
}

/// Errors an adapter can return (architecture.md Section 4.1).
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    /// Network / connection-level failure before an HTTP response.
    #[error("transport error: {0}")]
    Transport(String),
    /// The server returned a non-success HTTP status.
    #[error("http status {status}: {body}")]
    HttpStatus { status: u16, body: String },
    /// A response body could not be decoded / parsed into the expected shape.
    #[error("decode error: {0}")]
    Decode(String),
    /// The request needs a capability the model does not support.
    #[error("unsupported capability: {0}")]
    UnsupportedCapability(String),
    /// Authentication / credential resolution failure.
    #[error("auth error: {0}")]
    Auth(String),
    /// Any other adapter-specific failure.
    #[error("provider error: {0}")]
    Other(String),
}

/// The internal contract every provider adapter implements (architecture.md
/// Section 4.1).
#[async_trait]
pub trait ChatProvider: Send + Sync {
    /// Stable provider id, e.g. `"openai"`, `"lmstudio"`, `"anthropic"`.
    fn id(&self) -> &str;

    /// What this provider/model can do (streaming, tools, vision, etc.).
    fn capabilities(&self, model: &str) -> Capabilities;

    /// List models the provider currently offers (may hit the network).
    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError>;

    /// Non-streaming completion.
    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, ProviderError>;

    /// Streaming completion. Yields deltas until the stream ends.
    async fn chat_stream(
        &self,
        req: ChatRequest,
    ) -> Result<BoxStream<'static, Result<ChatDelta, ProviderError>>, ProviderError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn chat_request_round_trips() {
        let req = ChatRequest {
            model: "gpt-4o".to_string(),
            messages: vec![
                ChatMessage::text(MessageRole::System, "be terse"),
                ChatMessage::text(MessageRole::User, "hi"),
            ],
            tools: vec![ToolSpec::function(
                "get_weather",
                "get the weather",
                json!({"type": "object", "properties": {"city": {"type": "string"}}}),
            )],
            temperature: Some(0.2),
            max_tokens: Some(256),
            stream: true,
            extra: json!({"response_format": {"type": "json_object"}}),
        };
        let s = serde_json::to_string(&req).unwrap();
        let back: ChatRequest = serde_json::from_str(&s).unwrap();
        assert_eq!(back, req);
    }

    #[test]
    fn chat_message_omits_empty_optionals() {
        let msg = ChatMessage::text(MessageRole::User, "hello");
        let s = serde_json::to_string(&msg).unwrap();
        // No tool fields serialized for a plain text message.
        assert!(!s.contains("tool_calls"));
        assert!(!s.contains("tool_call_id"));
        assert!(s.contains("\"role\":\"user\""));
        assert!(s.contains("\"content\":\"hello\""));
    }

    #[test]
    fn provider_error_auth_display_carries_reason_not_key_material() {
        // The Auth variant's Display is `auth error: {0}`, where {0} is a REASON
        // string sourced from SecretError (NotFound(handle)/Backend(reason)) or an
        // adapter's own reason. It must surface the reason so a keyring resolve
        // failure is diagnosable, and it must NOT carry resolved key material. The
        // handle name (e.g. 'gemini-cloud') is a config id, not a secret.
        let err = ProviderError::Auth("no secret found for handle 'gemini-cloud'".to_string());
        let display = err.to_string();
        assert_eq!(
            display,
            "auth error: no secret found for handle 'gemini-cloud'"
        );
        // The reason is present so the UI can explain the failure.
        assert!(display.contains("no secret found"));
        assert!(display.contains("gemini-cloud"));
        // No key-shaped material leaks: the Display is exactly the reason, so a
        // hypothetical resolved key like an "sk-"/"AIza" token is never present.
        assert!(!display.contains("sk-"));
        assert!(!display.contains("AIza"));
    }

    #[test]
    fn tool_call_defaults_type_to_function() {
        // A tool_call missing the "type" field decodes with kind = "function".
        let value = json!({
            "id": "call_1",
            "function": {"name": "f", "arguments": "{}"}
        });
        let call: ToolCall = serde_json::from_value(value).unwrap();
        assert_eq!(call.kind, "function");
        assert_eq!(call.function.name, "f");
    }

    #[test]
    fn chat_response_round_trips() {
        let resp = ChatResponse {
            choices: vec![ChatChoice {
                index: 0,
                message: ChatMessage {
                    role: MessageRole::Assistant,
                    content: Some("hi there".to_string()),
                    tool_calls: vec![ToolCall {
                        id: "call_1".to_string(),
                        kind: "function".to_string(),
                        function: FunctionCall {
                            name: "get_weather".to_string(),
                            arguments: "{\"city\":\"NYC\"}".to_string(),
                        },
                    }],
                    tool_call_id: None,
                    name: None,
                },
                finish_reason: Some(FinishReason::ToolCalls),
            }],
            usage: Some(Usage {
                prompt_tokens: 10,
                completion_tokens: 5,
                total_tokens: 15,
            }),
            model: Some("gpt-4o".to_string()),
        };
        let s = serde_json::to_string(&resp).unwrap();
        assert!(s.contains("\"finish_reason\":\"tool_calls\""));
        let back: ChatResponse = serde_json::from_str(&s).unwrap();
        assert_eq!(back, resp);
    }

    #[test]
    fn chat_delta_round_trips_and_defaults() {
        let delta = ChatDelta {
            content: Some("partial".to_string()),
            thinking: Some("let me think".to_string()),
            tool_calls: vec![ToolCallDelta {
                index: 0,
                id: Some("call_1".to_string()),
                function_name: Some("f".to_string()),
                arguments_fragment: Some("{\"c".to_string()),
            }],
            finish_reason: None,
        };
        let s = serde_json::to_string(&delta).unwrap();
        assert!(s.contains("\"thinking\":\"let me think\""));
        let back: ChatDelta = serde_json::from_str(&s).unwrap();
        assert_eq!(back, delta);

        // A delta with no thinking omits the field entirely, keeping the wire
        // shape byte-identical to before the field existed (no regression).
        let no_thinking = ChatDelta {
            content: Some("answer".to_string()),
            ..ChatDelta::default()
        };
        let s = serde_json::to_string(&no_thinking).unwrap();
        assert!(!s.contains("thinking"));

        // An empty delta decodes from `{}` with all-empty fields (thinking None).
        let empty: ChatDelta = serde_json::from_str("{}").unwrap();
        assert_eq!(empty, ChatDelta::default());
        assert_eq!(empty.thinking, None);
    }

    #[test]
    fn capabilities_default_is_conservative() {
        let caps = Capabilities::default();
        assert!(!caps.streaming);
        assert!(!caps.tools);
        assert!(!caps.vision);
        assert!(!caps.json_mode);
        assert_eq!(caps.max_context, None);
    }
}
