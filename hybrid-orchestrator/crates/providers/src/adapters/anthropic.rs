//! Anthropic Messages API translation shim (architecture.md Section 4.3,
//! tasks.md P2.7).
//!
//! Anthropic does NOT speak the OpenAI Chat Completions wire format, so this is
//! a *translation shim* (contrast with [`super::native`]): it presents the
//! internal OpenAI-compatible [`ChatProvider`] contract to the rest of the core
//! while converting to and from Anthropic's Messages API at the edges.
//!
//! Outbound translation (`ChatRequest` -> Messages API body):
//!   - `POST {base_url}/v1/messages` (base_url default
//!     `https://api.anthropic.com`).
//!   - Headers `x-api-key: <key>` and `anthropic-version: <version>` (Anthropic
//!     does NOT use `Authorization: Bearer`).
//!   - System-role message(s) are split OUT of the messages array into the
//!     top-level `system` field (Anthropic keeps system separate).
//!   - user/assistant/tool messages map into the `messages` array; a `tool`
//!     role becomes a user message carrying a `tool_result` content block, and
//!     assistant `tool_calls` become `tool_use` content blocks.
//!   - internal [`ToolSpec`] function schemas map to Anthropic `tools`
//!     (`{name, description, input_schema}`).
//!   - `max_tokens` is required by Anthropic; a default is supplied when unset.
//!
//! Inbound translation (Anthropic -> internal):
//!   - Non-streaming: the `content` blocks + `usage` + `stop_reason` normalize
//!     into a single-choice [`ChatResponse`].
//!   - Streaming: the Anthropic SSE event stream (`message_start`,
//!     `content_block_start`, `content_block_delta`, `message_delta`, ...) is
//!     normalized into ordered internal [`ChatDelta`] values via the shared SSE
//!     decoder in [`crate::capability`].

use std::sync::Arc;

use async_trait::async_trait;
use futures_util::stream::BoxStream;
use serde::Deserialize;
use serde_json::{json, Value};

use domain::{ProviderConfig, ProviderKind};
use secrets::SecretStore;

use crate::capability::HttpSseClient;
use crate::contract::{
    Capabilities, ChatChoice, ChatDelta, ChatMessage, ChatProvider, ChatRequest, ChatResponse,
    FinishReason, FunctionCall, MessageRole, ModelInfo, ProviderError, ToolCall, ToolCallDelta,
    Usage,
};
use crate::registry::ProviderFactory;

/// Default Anthropic API root when `ProviderConfig::base_url` is unset.
pub const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";

/// Default `anthropic-version` header sent when the config does not override it
/// via `extra.anthropic_version`.
pub const DEFAULT_ANTHROPIC_VERSION: &str = "2023-06-01";

/// Anthropic requires `max_tokens`; use this when the request leaves it unset.
const DEFAULT_MAX_TOKENS: u32 = 4096;

/// The Anthropic Messages API translation shim.
#[derive(Debug)]
pub struct AnthropicAdapter {
    id: String,
    client: HttpSseClient,
    api_key: String,
    anthropic_version: String,
}

impl AnthropicAdapter {
    /// Construct the adapter with a resolved key + version. Key material lives
    /// only inside this value, never on the config.
    pub fn new(
        id: impl Into<String>,
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        anthropic_version: impl Into<String>,
    ) -> Self {
        AnthropicAdapter {
            id: id.into(),
            client: HttpSseClient::new(base_url),
            api_key: api_key.into(),
            anthropic_version: anthropic_version.into(),
        }
    }

    /// The auth + version headers for every request.
    ///
    /// `Content-Type: application/json` is intentionally NOT set here: the
    /// shared [`HttpSseClient`] POST helpers use reqwest's `.json()`, which
    /// already sets it once. Setting it again through `apply_headers` would make
    /// reqwest APPEND a duplicate value (`application/json,application/json`),
    /// which breaks exact-match `content-type` assertions.
    fn request_headers(&self) -> Vec<(String, String)> {
        vec![
            ("x-api-key".to_string(), self.api_key.clone()),
            (
                "anthropic-version".to_string(),
                self.anthropic_version.clone(),
            ),
        ]
    }

    /// Translate the internal [`ChatRequest`] into the Anthropic Messages API
    /// body, forcing the `stream` flag to `stream`.
    fn build_body(req: &ChatRequest, stream: bool) -> Value {
        let mut system_parts: Vec<String> = Vec::new();
        let mut messages: Vec<Value> = Vec::new();

        for msg in &req.messages {
            match msg.role {
                MessageRole::System => {
                    if let Some(text) = &msg.content {
                        system_parts.push(text.clone());
                    }
                }
                MessageRole::User => {
                    // Phase 2: user messages are TEXT-ONLY. The internal
                    // `ChatMessage` has no image/attachment field yet, so a user
                    // turn always maps to a single text block. NOTE: `caps_for`
                    // may report `vision: true` for Claude 3/4, but that
                    // advertises the model's future capability, not working
                    // image input here; multimodal content plumbing lands in a
                    // later phase (review issue 7).
                    messages.push(json!({
                        "role": "user",
                        "content": [{"type": "text", "text": msg.content.clone().unwrap_or_default()}],
                    }));
                }
                MessageRole::Assistant => {
                    let mut blocks: Vec<Value> = Vec::new();
                    if let Some(text) = &msg.content {
                        if !text.is_empty() {
                            blocks.push(json!({"type": "text", "text": text}));
                        }
                    }
                    for tc in &msg.tool_calls {
                        // Anthropic wants the arguments as a JSON object; the
                        // internal shape carries them as a JSON string.
                        let input: Value = serde_json::from_str(&tc.function.arguments)
                            .unwrap_or_else(|_| json!({}));
                        blocks.push(json!({
                            "type": "tool_use",
                            "id": tc.id,
                            "name": tc.function.name,
                            "input": input,
                        }));
                    }
                    messages.push(json!({"role": "assistant", "content": blocks}));
                }
                MessageRole::Tool => {
                    // A tool result is delivered to Anthropic as a user message
                    // carrying a `tool_result` content block keyed by the call
                    // id it answers.
                    messages.push(json!({
                        "role": "user",
                        "content": [{
                            "type": "tool_result",
                            "tool_use_id": msg.tool_call_id.clone().unwrap_or_default(),
                            "content": msg.content.clone().unwrap_or_default(),
                        }],
                    }));
                }
            }
        }

        let mut body = json!({
            "model": req.model,
            "messages": messages,
            "max_tokens": req.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
            "stream": stream,
        });

        let obj = body.as_object_mut().expect("body is an object");
        if !system_parts.is_empty() {
            obj.insert("system".to_string(), json!(system_parts.join("\n\n")));
        }
        if let Some(temp) = req.temperature {
            obj.insert("temperature".to_string(), json!(temp));
        }
        if !req.tools.is_empty() {
            let tools: Vec<Value> = req
                .tools
                .iter()
                .map(|t| {
                    json!({
                        "name": t.function.name,
                        "description": t.function.description,
                        "input_schema": t.function.parameters,
                    })
                })
                .collect();
            obj.insert("tools".to_string(), json!(tools));
        }
        body
    }

    /// Default per-model capabilities for Anthropic (tools + streaming true;
    /// vision on for the Claude 3 family).
    fn caps_for(model: &str) -> Capabilities {
        Capabilities {
            streaming: true,
            tools: true,
            vision: model.contains("claude-3") || model.contains("claude-4"),
            json_mode: false,
            max_context: None,
        }
    }
}

/// Map an Anthropic `stop_reason` onto the internal [`FinishReason`].
fn map_stop_reason(reason: Option<&str>) -> Option<FinishReason> {
    match reason {
        Some("end_turn") | Some("stop_sequence") => Some(FinishReason::Stop),
        Some("max_tokens") => Some(FinishReason::Length),
        Some("tool_use") => Some(FinishReason::ToolCalls),
        _ => None,
    }
}

// ---- Non-streaming response shapes ----------------------------------------

#[derive(Debug, Deserialize)]
struct MessagesResponse {
    #[serde(default)]
    content: Vec<ContentBlock>,
    #[serde(default)]
    stop_reason: Option<String>,
    #[serde(default)]
    usage: Option<AnthropicUsage>,
    #[serde(default)]
    model: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        #[serde(default)]
        input: Value,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
struct AnthropicUsage {
    #[serde(default)]
    input_tokens: u32,
    #[serde(default)]
    output_tokens: u32,
}

/// Normalize a non-streaming Anthropic Messages response into the internal
/// single-choice [`ChatResponse`].
fn normalize_response(resp: MessagesResponse) -> ChatResponse {
    let mut text = String::new();
    let mut tool_calls: Vec<ToolCall> = Vec::new();
    for block in resp.content {
        match block {
            ContentBlock::Text { text: t } => text.push_str(&t),
            ContentBlock::ToolUse { id, name, input } => {
                tool_calls.push(ToolCall {
                    id,
                    kind: "function".to_string(),
                    function: FunctionCall {
                        name,
                        arguments: input.to_string(),
                    },
                });
            }
            ContentBlock::Other => {}
        }
    }

    let message = ChatMessage {
        role: MessageRole::Assistant,
        content: if text.is_empty() { None } else { Some(text) },
        tool_calls,
        tool_call_id: None,
        name: None,
    };

    let usage = resp.usage.map(|u| Usage {
        prompt_tokens: u.input_tokens,
        completion_tokens: u.output_tokens,
        total_tokens: u.input_tokens + u.output_tokens,
    });

    ChatResponse {
        choices: vec![ChatChoice {
            index: 0,
            message,
            finish_reason: map_stop_reason(resp.stop_reason.as_deref()),
        }],
        usage,
        model: resp.model,
    }
}

// ---- Streaming event shapes ------------------------------------------------

/// The Anthropic SSE event envelope (only the fields we consume). The stream is
/// a sequence of typed events; we normalize the ones that carry incremental
/// text, tool-call fragments, or the terminal stop reason.
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum StreamEvent {
    #[serde(rename = "content_block_start")]
    ContentBlockStart {
        index: u32,
        content_block: StreamContentBlock,
    },
    #[serde(rename = "content_block_delta")]
    ContentBlockDelta { index: u32, delta: StreamDelta },
    #[serde(rename = "message_delta")]
    MessageDelta { delta: MessageDeltaBody },
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum StreamContentBlock {
    #[serde(rename = "tool_use")]
    ToolUse { id: String, name: String },
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum StreamDelta {
    #[serde(rename = "text_delta")]
    TextDelta { text: String },
    #[serde(rename = "input_json_delta")]
    InputJsonDelta { partial_json: String },
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
struct MessageDeltaBody {
    #[serde(default)]
    stop_reason: Option<String>,
}

/// Normalize one Anthropic SSE event payload into an internal [`ChatDelta`].
/// Returns `Ok(None)` for events that carry nothing the core consumes
/// (`message_start`, `ping`, `content_block_stop`, ...).
fn parse_stream_event(data: &str) -> Result<Option<ChatDelta>, ProviderError> {
    let event: StreamEvent =
        serde_json::from_str(data).map_err(|e| ProviderError::Decode(e.to_string()))?;
    match event {
        StreamEvent::ContentBlockStart {
            index,
            content_block: StreamContentBlock::ToolUse { id, name },
        } => Ok(Some(ChatDelta {
            content: None,
            thinking: None,
            tool_calls: vec![ToolCallDelta {
                index,
                id: Some(id),
                function_name: Some(name),
                arguments_fragment: None,
            }],
            finish_reason: None,
        })),
        StreamEvent::ContentBlockDelta {
            delta: StreamDelta::TextDelta { text },
            ..
        } => Ok(Some(ChatDelta {
            content: Some(text),
            ..ChatDelta::default()
        })),
        StreamEvent::ContentBlockDelta {
            index,
            delta: StreamDelta::InputJsonDelta { partial_json },
        } => Ok(Some(ChatDelta {
            content: None,
            thinking: None,
            tool_calls: vec![ToolCallDelta {
                index,
                id: None,
                function_name: None,
                arguments_fragment: Some(partial_json),
            }],
            finish_reason: None,
        })),
        StreamEvent::MessageDelta {
            delta: MessageDeltaBody { stop_reason },
        } => match map_stop_reason(stop_reason.as_deref()) {
            Some(reason) => Ok(Some(ChatDelta {
                finish_reason: Some(reason),
                ..ChatDelta::default()
            })),
            None => Ok(None),
        },
        _ => Ok(None),
    }
}

#[async_trait]
impl ChatProvider for AnthropicAdapter {
    fn id(&self) -> &str {
        &self.id
    }

    fn capabilities(&self, model: &str) -> Capabilities {
        Self::caps_for(model)
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        // Anthropic has no public dynamic model-listing endpoint historically;
        // return the well-known ids so the UI can populate a picker without a
        // network round-trip.
        Ok(vec![
            ModelInfo::new("claude-3-5-sonnet-latest"),
            ModelInfo::new("claude-3-5-haiku-latest"),
            ModelInfo::new("claude-3-opus-latest"),
        ])
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, ProviderError> {
        let headers = self.request_headers();
        let body = Self::build_body(&req, false);
        let raw: MessagesResponse = self
            .client
            .post_json("/v1/messages", &headers, &body)
            .await?;
        Ok(normalize_response(raw))
    }

    async fn chat_stream(
        &self,
        req: ChatRequest,
    ) -> Result<BoxStream<'static, Result<ChatDelta, ProviderError>>, ProviderError> {
        let headers = self.request_headers();
        let body = Self::build_body(&req, true);
        self.client
            .post_sse("/v1/messages", &headers, &body, parse_stream_event)
            .await
    }
}

/// Build the Anthropic adapter from a [`ProviderConfig`], resolving the key
/// through `secrets` at build time. Anthropic requires a key: a missing
/// `api_key_ref` is an [`Auth`](ProviderError::Auth) error.
pub fn build_anthropic(
    cfg: &ProviderConfig,
    secrets: &dyn SecretStore,
) -> Result<AnthropicAdapter, ProviderError> {
    let key_ref = cfg.api_key_ref.as_ref().ok_or_else(|| {
        ProviderError::Auth("Anthropic provider requires an api_key_ref".to_string())
    })?;
    let key = secrets
        .resolve(key_ref)
        .map_err(|e| ProviderError::Auth(e.to_string()))?;
    let base_url = cfg
        .base_url
        .clone()
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
    let version = cfg
        .extra
        .get("anthropic_version")
        .and_then(|v| v.as_str())
        .unwrap_or(DEFAULT_ANTHROPIC_VERSION)
        .to_string();
    Ok(AnthropicAdapter::new(
        cfg.id.clone(),
        base_url,
        key,
        version,
    ))
}

/// [`ProviderFactory`] for [`ProviderKind::Anthropic`].
pub struct AnthropicFactory;

impl ProviderFactory for AnthropicFactory {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Anthropic
    }

    fn build(
        &self,
        cfg: &ProviderConfig,
        secrets: &dyn SecretStore,
    ) -> Result<Arc<dyn ChatProvider>, ProviderError> {
        Ok(Arc::new(build_anthropic(cfg, secrets)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::ToolSpec;
    use secrets::InMemorySecretStore;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, Request, ResponseTemplate};

    fn sample_request() -> ChatRequest {
        let mut r = ChatRequest::new(
            "claude-3-5-sonnet-latest",
            vec![
                ChatMessage::text(MessageRole::System, "be terse"),
                ChatMessage::text(MessageRole::User, "hi"),
            ],
        );
        r.max_tokens = Some(128);
        r.tools = vec![ToolSpec::function(
            "get_weather",
            "get the weather",
            json!({"type": "object", "properties": {"city": {"type": "string"}}}),
        )];
        r
    }

    #[test]
    fn build_body_splits_system_and_maps_tools() {
        let body = AnthropicAdapter::build_body(&sample_request(), false);
        // System prompt is split OUT to the top-level `system` field.
        assert_eq!(body["system"], json!("be terse"));
        // The messages array carries only non-system messages.
        assert_eq!(body["messages"].as_array().unwrap().len(), 1);
        assert_eq!(body["messages"][0]["role"], json!("user"));
        // Tools map to Anthropic {name, description, input_schema}.
        assert_eq!(body["tools"][0]["name"], json!("get_weather"));
        assert!(body["tools"][0]["input_schema"].is_object());
        assert_eq!(body["max_tokens"], json!(128));
        assert_eq!(body["stream"], json!(false));
    }

    #[test]
    fn build_body_maps_assistant_tool_calls_and_tool_results() {
        let mut r = ChatRequest::new("claude-3-5-sonnet-latest", vec![]);
        r.messages = vec![
            ChatMessage {
                role: MessageRole::Assistant,
                content: None,
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
            ChatMessage {
                role: MessageRole::Tool,
                content: Some("sunny".to_string()),
                tool_calls: vec![],
                tool_call_id: Some("call_1".to_string()),
                name: None,
            },
        ];
        let body = AnthropicAdapter::build_body(&r, false);
        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs[0]["role"], json!("assistant"));
        assert_eq!(msgs[0]["content"][0]["type"], json!("tool_use"));
        assert_eq!(msgs[0]["content"][0]["id"], json!("call_1"));
        assert_eq!(msgs[0]["content"][0]["input"], json!({"city": "NYC"}));
        // Tool result becomes a user message carrying a tool_result block.
        assert_eq!(msgs[1]["role"], json!("user"));
        assert_eq!(msgs[1]["content"][0]["type"], json!("tool_result"));
        assert_eq!(msgs[1]["content"][0]["tool_use_id"], json!("call_1"));
    }

    #[tokio::test]
    async fn chat_sends_anthropic_headers_and_normalizes_response() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .and(header("x-api-key", "sk-ant-test"))
            .and(header("anthropic-version", DEFAULT_ANTHROPIC_VERSION))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "content": [
                    {"type": "text", "text": "Hello there"},
                    {"type": "tool_use", "id": "tu_1", "name": "get_weather", "input": {"city": "NYC"}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 10, "output_tokens": 5},
                "model": "claude-3-5-sonnet-latest"
            })))
            .mount(&server)
            .await;

        let adapter = AnthropicAdapter::new(
            "anthropic",
            server.uri(),
            "sk-ant-test",
            DEFAULT_ANTHROPIC_VERSION,
        );
        let resp = adapter.chat(sample_request()).await.unwrap();

        assert_eq!(resp.choices.len(), 1);
        let msg = &resp.choices[0].message;
        assert_eq!(msg.content.as_deref(), Some("Hello there"));
        assert_eq!(msg.tool_calls.len(), 1);
        assert_eq!(msg.tool_calls[0].function.name, "get_weather");
        assert_eq!(msg.tool_calls[0].function.arguments, "{\"city\":\"NYC\"}");
        assert_eq!(resp.choices[0].finish_reason, Some(FinishReason::ToolCalls));
        assert_eq!(resp.usage.unwrap().total_tokens, 15);
    }

    #[tokio::test]
    async fn chat_translation_sends_split_system_in_body() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(|req: &Request| {
                let body: Value = serde_json::from_slice(&req.body).unwrap();
                // Assert the outbound native body split the system prompt out.
                assert_eq!(body["system"], json!("be terse"));
                assert_eq!(body["messages"].as_array().unwrap().len(), 1);
                ResponseTemplate::new(200).set_body_json(json!({
                    "content": [{"type": "text", "text": "ok"}],
                    "stop_reason": "end_turn"
                }))
            })
            .mount(&server)
            .await;

        let adapter =
            AnthropicAdapter::new("anthropic", server.uri(), "k", DEFAULT_ANTHROPIC_VERSION);
        let resp = adapter.chat(sample_request()).await.unwrap();
        assert_eq!(resp.choices[0].message.content.as_deref(), Some("ok"));
        assert_eq!(resp.choices[0].finish_reason, Some(FinishReason::Stop));
    }

    #[tokio::test]
    async fn chat_stream_normalizes_sse_events() {
        let server = MockServer::start().await;
        // A representative Anthropic SSE stream: text deltas, a tool_use block
        // with input_json_delta fragments, then a terminal message_delta.
        let sse = concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{\"id\":\"m1\"}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hel\"}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"lo\"}}\n\n",
            "event: content_block_start\n",
            "data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"tu_1\",\"name\":\"get_weather\"}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"c\"}}\n\n",
            "event: message_delta\n",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"}}\n\n",
        );
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(sse),
            )
            .mount(&server)
            .await;

        let adapter =
            AnthropicAdapter::new("anthropic", server.uri(), "k", DEFAULT_ANTHROPIC_VERSION);
        let mut req = sample_request();
        req.stream = true;
        let stream = adapter.chat_stream(req).await.unwrap();
        use futures_util::StreamExt;
        let deltas: Vec<ChatDelta> = stream.map(|r| r.unwrap()).collect().await;

        assert_eq!(deltas[0].content.as_deref(), Some("Hel"));
        assert_eq!(deltas[1].content.as_deref(), Some("lo"));
        // tool_use start carries id + name.
        assert_eq!(deltas[2].tool_calls[0].id.as_deref(), Some("tu_1"));
        assert_eq!(
            deltas[2].tool_calls[0].function_name.as_deref(),
            Some("get_weather")
        );
        // input_json_delta carries an arguments fragment.
        assert_eq!(
            deltas[3].tool_calls[0].arguments_fragment.as_deref(),
            Some("{\"c")
        );
        // Terminal message_delta carries the finish reason.
        assert_eq!(
            deltas.last().unwrap().finish_reason,
            Some(FinishReason::ToolCalls)
        );
    }

    #[test]
    fn factory_builds_and_resolves_key() {
        let store = InMemorySecretStore::new();
        let key_ref = store.store("anthropic-key", "sk-ant-xyz").unwrap();
        let cfg = ProviderConfig {
            id: "anthropic".to_string(),
            kind: ProviderKind::Anthropic,
            base_url: None,
            api_key_ref: Some(key_ref),
            extra: Value::Null,
        };
        let adapter = build_anthropic(&cfg, &store).unwrap();
        assert_eq!(adapter.id(), "anthropic");
        assert_eq!(adapter.api_key, "sk-ant-xyz");
        assert_eq!(adapter.anthropic_version, DEFAULT_ANTHROPIC_VERSION);
        assert_eq!(AnthropicFactory.kind(), ProviderKind::Anthropic);
    }

    #[test]
    fn factory_requires_api_key() {
        let store = InMemorySecretStore::new();
        let cfg = ProviderConfig {
            id: "anthropic".to_string(),
            kind: ProviderKind::Anthropic,
            base_url: None,
            api_key_ref: None,
            extra: Value::Null,
        };
        match build_anthropic(&cfg, &store) {
            Err(ProviderError::Auth(_)) => {}
            other => panic!("expected Auth error, got {other:?}"),
        }
    }
}
