//! Google Gemini `generateContent` translation shim (architecture.md Section
//! 4.3, tasks.md P2.8).
//!
//! Gemini does NOT speak the OpenAI Chat Completions wire format, so this is a
//! *translation shim*: it presents the internal OpenAI-compatible
//! [`ChatProvider`] contract while converting to and from Gemini's
//! `generateContent` / `streamGenerateContent` API at the edges.
//!
//! Outbound translation (`ChatRequest` -> Gemini body):
//!   - Non-streaming: `POST {base_url}/v1beta/models/{model}:generateContent`.
//!   - Streaming:
//!     `POST {base_url}/v1beta/models/{model}:streamGenerateContent?alt=sse`.
//!   - The API key is placed on the URL query string (`?key=<key>`), the Gemini
//!     convention.
//!   - user/assistant messages map to `contents` entries with `parts`; the
//!     Gemini role for assistant is `model`. Tool results map to a `function`
//!     part; assistant tool calls map to a `functionCall` part.
//!   - system-role message(s) map to the top-level `systemInstruction`.
//!   - internal [`ToolSpec`] function schemas map to
//!     `tools[0].functionDeclarations`.
//!   - `extra.project` is read when present (surfaced for downstream/OAuth use).
//!
//! Inbound translation (Gemini -> internal):
//!   - Non-streaming: `candidates[0].content.parts[]` (text + functionCall) plus
//!     `usageMetadata` normalize into a single-choice [`ChatResponse`].
//!   - Streaming: each streamed chunk (`candidates[].content.parts[]`) is
//!     normalized into an internal [`ChatDelta`] via the shared SSE decoder.

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

/// Default Gemini API root when `ProviderConfig::base_url` is unset.
pub const DEFAULT_BASE_URL: &str = "https://generativelanguage.googleapis.com";

/// The Gemini `generateContent` translation shim.
pub struct GeminiAdapter {
    id: String,
    client: HttpSseClient,
    api_key: String,
    /// Optional GCP project id read from `extra.project`; retained for
    /// downstream OAuth / quota-project use.
    #[allow(dead_code)] // surfaced to callers; broader OAuth wiring lands in Phase 3.
    project: Option<String>,
}

impl GeminiAdapter {
    /// Construct the adapter with a resolved key. Key material lives only inside
    /// this value, never on the config.
    pub fn new(
        id: impl Into<String>,
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        project: Option<String>,
    ) -> Self {
        GeminiAdapter {
            id: id.into(),
            client: HttpSseClient::new(base_url),
            api_key: api_key.into(),
            project,
        }
    }

    fn request_headers(&self) -> Vec<(String, String)> {
        vec![("Content-Type".to_string(), "application/json".to_string())]
    }

    /// The path for the non-streaming or streaming endpoint, with the API key
    /// on the query string (Gemini convention).
    fn path(&self, model: &str, stream: bool) -> String {
        if stream {
            format!(
                "/v1beta/models/{model}:streamGenerateContent?alt=sse&key={key}",
                key = self.api_key
            )
        } else {
            format!(
                "/v1beta/models/{model}:generateContent?key={key}",
                key = self.api_key
            )
        }
    }

    /// Translate the internal [`ChatRequest`] into the Gemini request body.
    fn build_body(req: &ChatRequest) -> Value {
        let mut system_parts: Vec<String> = Vec::new();
        let mut contents: Vec<Value> = Vec::new();

        for msg in &req.messages {
            match msg.role {
                MessageRole::System => {
                    if let Some(text) = &msg.content {
                        system_parts.push(text.clone());
                    }
                }
                MessageRole::User => {
                    contents.push(json!({
                        "role": "user",
                        "parts": [{"text": msg.content.clone().unwrap_or_default()}],
                    }));
                }
                MessageRole::Assistant => {
                    let mut parts: Vec<Value> = Vec::new();
                    if let Some(text) = &msg.content {
                        if !text.is_empty() {
                            parts.push(json!({"text": text}));
                        }
                    }
                    for tc in &msg.tool_calls {
                        let args: Value = serde_json::from_str(&tc.function.arguments)
                            .unwrap_or_else(|_| json!({}));
                        parts.push(json!({
                            "functionCall": {"name": tc.function.name, "args": args}
                        }));
                    }
                    contents.push(json!({"role": "model", "parts": parts}));
                }
                MessageRole::Tool => {
                    // A tool result maps to a `functionResponse` part in a user
                    // turn, keyed by the participant name (function name).
                    let response: Value = msg
                        .content
                        .as_ref()
                        .and_then(|c| serde_json::from_str(c).ok())
                        .unwrap_or_else(
                            || json!({"result": msg.content.clone().unwrap_or_default()}),
                        );
                    contents.push(json!({
                        "role": "user",
                        "parts": [{
                            "functionResponse": {
                                "name": msg.name.clone().unwrap_or_default(),
                                "response": response,
                            }
                        }],
                    }));
                }
            }
        }

        let mut body = json!({ "contents": contents });
        let obj = body.as_object_mut().expect("body is an object");

        if !system_parts.is_empty() {
            obj.insert(
                "systemInstruction".to_string(),
                json!({"parts": [{"text": system_parts.join("\n\n")}]}),
            );
        }

        if !req.tools.is_empty() {
            let decls: Vec<Value> = req
                .tools
                .iter()
                .map(|t| {
                    json!({
                        "name": t.function.name,
                        "description": t.function.description,
                        "parameters": t.function.parameters,
                    })
                })
                .collect();
            obj.insert(
                "tools".to_string(),
                json!([{"functionDeclarations": decls}]),
            );
        }

        let mut gen_config = serde_json::Map::new();
        if let Some(temp) = req.temperature {
            gen_config.insert("temperature".to_string(), json!(temp));
        }
        if let Some(max) = req.max_tokens {
            gen_config.insert("maxOutputTokens".to_string(), json!(max));
        }
        if !gen_config.is_empty() {
            obj.insert("generationConfig".to_string(), Value::Object(gen_config));
        }

        body
    }

    fn caps_for(model: &str) -> Capabilities {
        Capabilities {
            streaming: true,
            tools: true,
            vision: model.contains("gemini-1.5")
                || model.contains("gemini-2")
                || model.contains("pro")
                || model.contains("flash"),
            json_mode: true,
            max_context: None,
        }
    }
}

/// Map a Gemini `finishReason` onto the internal [`FinishReason`].
fn map_finish_reason(reason: Option<&str>) -> Option<FinishReason> {
    match reason {
        Some("STOP") => Some(FinishReason::Stop),
        Some("MAX_TOKENS") => Some(FinishReason::Length),
        Some("SAFETY") | Some("RECITATION") => Some(FinishReason::ContentFilter),
        _ => None,
    }
}

// ---- Response shapes (shared by streaming + non-streaming) -----------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GenerateContentResponse {
    #[serde(default)]
    candidates: Vec<Candidate>,
    #[serde(default)]
    usage_metadata: Option<UsageMetadata>,
    #[serde(default)]
    model_version: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Candidate {
    #[serde(default)]
    content: Option<Content>,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Content {
    #[serde(default)]
    parts: Vec<Part>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Part {
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    function_call: Option<GeminiFunctionCall>,
}

#[derive(Debug, Deserialize)]
struct GeminiFunctionCall {
    name: String,
    #[serde(default)]
    args: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UsageMetadata {
    #[serde(default)]
    prompt_token_count: u32,
    #[serde(default)]
    candidates_token_count: u32,
    #[serde(default)]
    total_token_count: u32,
}

/// Normalize a non-streaming Gemini response into the internal single-choice
/// [`ChatResponse`].
fn normalize_response(resp: GenerateContentResponse) -> ChatResponse {
    let mut text = String::new();
    let mut tool_calls: Vec<ToolCall> = Vec::new();
    let mut finish_reason = None;

    if let Some(candidate) = resp.candidates.into_iter().next() {
        finish_reason = map_finish_reason(candidate.finish_reason.as_deref());
        if let Some(content) = candidate.content {
            for (i, part) in content.parts.into_iter().enumerate() {
                if let Some(t) = part.text {
                    text.push_str(&t);
                }
                if let Some(fc) = part.function_call {
                    tool_calls.push(ToolCall {
                        id: format!("call_{i}"),
                        kind: "function".to_string(),
                        function: FunctionCall {
                            name: fc.name,
                            arguments: fc.args.to_string(),
                        },
                    });
                }
            }
        }
    }

    let message = ChatMessage {
        role: MessageRole::Assistant,
        content: if text.is_empty() { None } else { Some(text) },
        tool_calls,
        tool_call_id: None,
        name: None,
    };

    let usage = resp.usage_metadata.map(|u| Usage {
        prompt_tokens: u.prompt_token_count,
        completion_tokens: u.candidates_token_count,
        total_tokens: u.total_token_count,
    });

    ChatResponse {
        choices: vec![ChatChoice {
            index: 0,
            message,
            finish_reason,
        }],
        usage,
        model: resp.model_version,
    }
}

/// Normalize one streamed Gemini chunk into an internal [`ChatDelta`]. Returns
/// `Ok(None)` for chunks that carry nothing the core consumes.
fn parse_stream_chunk(data: &str) -> Result<Option<ChatDelta>, ProviderError> {
    let chunk: GenerateContentResponse =
        serde_json::from_str(data).map_err(|e| ProviderError::Decode(e.to_string()))?;

    let Some(candidate) = chunk.candidates.into_iter().next() else {
        return Ok(None);
    };

    let mut content = String::new();
    let mut tool_calls: Vec<ToolCallDelta> = Vec::new();
    if let Some(c) = candidate.content {
        for (i, part) in c.parts.into_iter().enumerate() {
            if let Some(t) = part.text {
                content.push_str(&t);
            }
            if let Some(fc) = part.function_call {
                tool_calls.push(ToolCallDelta {
                    index: i as u32,
                    id: Some(format!("call_{i}")),
                    function_name: Some(fc.name),
                    arguments_fragment: Some(fc.args.to_string()),
                });
            }
        }
    }

    let finish_reason = map_finish_reason(candidate.finish_reason.as_deref());
    let delta = ChatDelta {
        content: if content.is_empty() {
            None
        } else {
            Some(content)
        },
        tool_calls,
        finish_reason,
    };

    if delta.content.is_none() && delta.tool_calls.is_empty() && delta.finish_reason.is_none() {
        return Ok(None);
    }
    Ok(Some(delta))
}

#[async_trait]
impl ChatProvider for GeminiAdapter {
    fn id(&self) -> &str {
        &self.id
    }

    fn capabilities(&self, model: &str) -> Capabilities {
        Self::caps_for(model)
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        // The Gemini `/v1beta/models` listing needs the key on the query string.
        #[derive(Deserialize)]
        struct ModelsResponse {
            #[serde(default)]
            models: Vec<ModelEntry>,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct ModelEntry {
            name: String,
            #[serde(default)]
            display_name: Option<String>,
        }
        let path = format!("/v1beta/models?key={key}", key = self.api_key);
        let resp: ModelsResponse = self.client.get_json(&path, &self.request_headers()).await?;
        Ok(resp
            .models
            .into_iter()
            .map(|m| {
                // Gemini returns fully-qualified names like `models/gemini-pro`;
                // strip the prefix for the internal id.
                let id = m
                    .name
                    .strip_prefix("models/")
                    .unwrap_or(&m.name)
                    .to_string();
                ModelInfo {
                    id,
                    display_name: m.display_name,
                    context_length: None,
                }
            })
            .collect())
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, ProviderError> {
        let headers = self.request_headers();
        let body = Self::build_body(&req);
        let path = self.path(&req.model, false);
        let raw: GenerateContentResponse = self.client.post_json(&path, &headers, &body).await?;
        Ok(normalize_response(raw))
    }

    async fn chat_stream(
        &self,
        req: ChatRequest,
    ) -> Result<BoxStream<'static, Result<ChatDelta, ProviderError>>, ProviderError> {
        let headers = self.request_headers();
        let body = Self::build_body(&req);
        let path = self.path(&req.model, true);
        self.client
            .post_sse(&path, &headers, &body, parse_stream_chunk)
            .await
    }
}

/// Build the Gemini adapter from a [`ProviderConfig`], resolving the key through
/// `secrets` at build time. Gemini requires a key: a missing `api_key_ref` is an
/// [`Auth`](ProviderError::Auth) error.
pub fn build_gemini(
    cfg: &ProviderConfig,
    secrets: &dyn SecretStore,
) -> Result<GeminiAdapter, ProviderError> {
    let key_ref = cfg.api_key_ref.as_ref().ok_or_else(|| {
        ProviderError::Auth("Gemini provider requires an api_key_ref".to_string())
    })?;
    let key = secrets
        .resolve(key_ref)
        .map_err(|e| ProviderError::Auth(e.to_string()))?;
    let base_url = cfg
        .base_url
        .clone()
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
    let project = cfg
        .extra
        .get("project")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    Ok(GeminiAdapter::new(cfg.id.clone(), base_url, key, project))
}

/// [`ProviderFactory`] for [`ProviderKind::Gemini`].
pub struct GeminiFactory;

impl ProviderFactory for GeminiFactory {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Gemini
    }

    fn build(
        &self,
        cfg: &ProviderConfig,
        secrets: &dyn SecretStore,
    ) -> Result<Arc<dyn ChatProvider>, ProviderError> {
        Ok(Arc::new(build_gemini(cfg, secrets)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::ToolSpec;
    use futures_util::StreamExt;
    use secrets::InMemorySecretStore;
    use wiremock::matchers::{method, query_param};
    use wiremock::{Mock, MockServer, Request, ResponseTemplate};

    fn sample_request() -> ChatRequest {
        let mut r = ChatRequest::new(
            "gemini-1.5-pro",
            vec![
                ChatMessage::text(MessageRole::System, "be terse"),
                ChatMessage::text(MessageRole::User, "hi"),
            ],
        );
        r.tools = vec![ToolSpec::function(
            "get_weather",
            "get the weather",
            json!({"type": "object", "properties": {"city": {"type": "string"}}}),
        )];
        r
    }

    #[test]
    fn build_body_maps_contents_system_and_tools() {
        let body = GeminiAdapter::build_body(&sample_request());
        // System prompt maps to systemInstruction.
        assert_eq!(
            body["systemInstruction"]["parts"][0]["text"],
            json!("be terse")
        );
        // User message maps to contents/parts with role user.
        assert_eq!(body["contents"][0]["role"], json!("user"));
        assert_eq!(body["contents"][0]["parts"][0]["text"], json!("hi"));
        // Tools map to functionDeclarations under tools[0].
        assert_eq!(
            body["tools"][0]["functionDeclarations"][0]["name"],
            json!("get_weather")
        );
        assert!(body["tools"][0]["functionDeclarations"][0]["parameters"].is_object());
    }

    #[test]
    fn build_body_maps_assistant_tool_calls() {
        let mut r = ChatRequest::new("gemini-1.5-pro", vec![]);
        r.messages = vec![ChatMessage {
            role: MessageRole::Assistant,
            content: Some("thinking".to_string()),
            tool_calls: vec![ToolCall {
                id: "call_0".to_string(),
                kind: "function".to_string(),
                function: FunctionCall {
                    name: "get_weather".to_string(),
                    arguments: "{\"city\":\"NYC\"}".to_string(),
                },
            }],
            tool_call_id: None,
            name: None,
        }];
        let body = GeminiAdapter::build_body(&r);
        assert_eq!(body["contents"][0]["role"], json!("model"));
        assert_eq!(body["contents"][0]["parts"][0]["text"], json!("thinking"));
        assert_eq!(
            body["contents"][0]["parts"][1]["functionCall"]["name"],
            json!("get_weather")
        );
        assert_eq!(
            body["contents"][0]["parts"][1]["functionCall"]["args"],
            json!({"city": "NYC"})
        );
    }

    #[test]
    fn path_places_key_on_query_string() {
        let adapter = GeminiAdapter::new("gemini", "http://x", "AIza-key", None);
        assert_eq!(
            adapter.path("gemini-1.5-pro", false),
            "/v1beta/models/gemini-1.5-pro:generateContent?key=AIza-key"
        );
        assert_eq!(
            adapter.path("gemini-1.5-pro", true),
            "/v1beta/models/gemini-1.5-pro:streamGenerateContent?alt=sse&key=AIza-key"
        );
    }

    #[tokio::test]
    async fn chat_sends_key_query_and_normalizes_response() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(query_param("key", "AIza-test"))
            .respond_with(|req: &Request| {
                let body: Value = serde_json::from_slice(&req.body).unwrap();
                // Assert outbound translation: contents/parts + tools.
                assert_eq!(body["contents"][0]["role"], json!("user"));
                assert_eq!(
                    body["tools"][0]["functionDeclarations"][0]["name"],
                    json!("get_weather")
                );
                ResponseTemplate::new(200).set_body_json(json!({
                    "candidates": [{
                        "content": {"parts": [
                            {"text": "Hello there"},
                            {"functionCall": {"name": "get_weather", "args": {"city": "NYC"}}}
                        ]},
                        "finishReason": "STOP"
                    }],
                    "usageMetadata": {
                        "promptTokenCount": 10,
                        "candidatesTokenCount": 5,
                        "totalTokenCount": 15
                    },
                    "modelVersion": "gemini-1.5-pro"
                }))
            })
            .mount(&server)
            .await;

        let adapter = GeminiAdapter::new("gemini", server.uri(), "AIza-test", None);
        let resp = adapter.chat(sample_request()).await.unwrap();

        let msg = &resp.choices[0].message;
        assert_eq!(msg.content.as_deref(), Some("Hello there"));
        assert_eq!(msg.tool_calls.len(), 1);
        assert_eq!(msg.tool_calls[0].function.name, "get_weather");
        assert_eq!(msg.tool_calls[0].function.arguments, "{\"city\":\"NYC\"}");
        assert_eq!(resp.choices[0].finish_reason, Some(FinishReason::Stop));
        assert_eq!(resp.usage.unwrap().total_tokens, 15);
    }

    #[tokio::test]
    async fn chat_stream_normalizes_chunks() {
        let server = MockServer::start().await;
        // Gemini streamed chunks arrive as SSE `data:` frames when alt=sse.
        let sse = concat!(
            "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"Hel\"}]}}]}\n\n",
            "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"lo\"}]}}]}\n\n",
            "data: {\"candidates\":[{\"content\":{\"parts\":[{\"functionCall\":{\"name\":\"get_weather\",\"args\":{\"city\":\"NYC\"}}}]},\"finishReason\":\"STOP\"}]}\n\n",
        );
        Mock::given(method("POST"))
            .and(query_param("alt", "sse"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(sse),
            )
            .mount(&server)
            .await;

        let adapter = GeminiAdapter::new("gemini", server.uri(), "AIza-test", None);
        let mut req = sample_request();
        req.stream = true;
        let stream = adapter.chat_stream(req).await.unwrap();
        let deltas: Vec<ChatDelta> = stream.map(|r| r.unwrap()).collect().await;

        assert_eq!(deltas[0].content.as_deref(), Some("Hel"));
        assert_eq!(deltas[1].content.as_deref(), Some("lo"));
        assert_eq!(
            deltas[2].tool_calls[0].function_name.as_deref(),
            Some("get_weather")
        );
        assert_eq!(deltas[2].finish_reason, Some(FinishReason::Stop));
    }

    #[test]
    fn factory_builds_and_reads_project() {
        let store = InMemorySecretStore::new();
        let key_ref = store.store("gemini-key", "AIza-xyz").unwrap();
        let cfg = ProviderConfig {
            id: "gemini".to_string(),
            kind: ProviderKind::Gemini,
            base_url: None,
            api_key_ref: Some(key_ref),
            extra: json!({"project": "my-gcp-project"}),
        };
        let adapter = build_gemini(&cfg, &store).unwrap();
        assert_eq!(adapter.id(), "gemini");
        assert_eq!(adapter.api_key, "AIza-xyz");
        assert_eq!(adapter.project.as_deref(), Some("my-gcp-project"));
        assert_eq!(GeminiFactory.kind(), ProviderKind::Gemini);
    }

    #[test]
    fn factory_requires_api_key() {
        let store = InMemorySecretStore::new();
        let cfg = ProviderConfig {
            id: "gemini".to_string(),
            kind: ProviderKind::Gemini,
            base_url: None,
            api_key_ref: None,
            extra: Value::Null,
        };
        match build_gemini(&cfg, &store) {
            Err(ProviderError::Auth(_)) => {}
            other => panic!("expected Auth error, got {other:?}"),
        }
    }
}
