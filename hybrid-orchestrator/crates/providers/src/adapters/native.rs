//! The shared native OpenAI-compatible adapter (architecture.md Section 4.3).
//!
//! OpenAI, LM Studio, the generic user-supplied endpoint, and Azure OpenAI all
//! speak the OpenAI Chat Completions wire format. Rather than duplicate the
//! request-building, non-streaming decode, and SSE-normalization logic four
//! times, they all share [`NativeAdapter`], parameterized by:
//!
//! - a [`crate::HttpSseClient`] rooted at the provider's `base_url`,
//! - an [`AuthStrategy`] (Bearer for OpenAI/Generic, none for LM Studio,
//!   `api-key` header for Azure),
//! - a [`Routing`] describing how to form the chat/models paths (plain
//!   `/chat/completions` + `/models`, or the Azure deployment path-rewrite with
//!   an `api-version` query parameter).
//!
//! The four adapter modules ([`super::openai`], [`super::lmstudio`],
//! [`super::generic_openai`], [`super::azure_openai`]) are thin: each builds a
//! `NativeAdapter` with the right base_url + auth + routing from a
//! [`domain::ProviderConfig`] and delegates every [`ChatProvider`] method to it.

use async_trait::async_trait;
use futures_util::stream::BoxStream;
use serde::Deserialize;

use crate::capability::HttpSseClient;
use crate::contract::{
    Capabilities, ChatDelta, ChatProvider, ChatRequest, ChatResponse, ModelInfo, ProviderError,
};

/// How the adapter authenticates outbound requests (architecture.md Section
/// 4.3). Resolved from the provider config + secret store at build time; the
/// key material lives only inside this value, never on the config.
#[derive(Debug, Clone)]
pub enum AuthStrategy {
    /// Send no auth header (LM Studio's default local mode).
    None,
    /// `Authorization: Bearer <key>` (OpenAI and generic OpenAI-compatible).
    Bearer(String),
    /// `api-key: <key>` (Azure OpenAI, which does NOT use Bearer).
    ApiKeyHeader(String),
}

impl AuthStrategy {
    /// Produce the auth header(s) for a request, if any.
    fn headers(&self) -> Vec<(String, String)> {
        match self {
            AuthStrategy::None => Vec::new(),
            AuthStrategy::Bearer(key) => {
                vec![("Authorization".to_string(), format!("Bearer {key}"))]
            }
            AuthStrategy::ApiKeyHeader(key) => vec![("api-key".to_string(), key.clone())],
        }
    }
}

/// How chat/models request paths are formed for this provider (architecture.md
/// Section 4.3).
#[derive(Debug, Clone)]
pub enum Routing {
    /// Plain OpenAI layout: `POST {base}/chat/completions`, `GET {base}/models`.
    Standard,
    /// Azure deployment routing: `POST
    /// {base}/openai/deployments/{deployment}/chat/completions?api-version=...`
    /// and `GET {base}/openai/models?api-version=...`.
    Azure {
        deployment: String,
        api_version: String,
    },
}

impl Routing {
    /// The path (including any query string) for the chat completions endpoint.
    fn chat_path(&self) -> String {
        match self {
            Routing::Standard => "chat/completions".to_string(),
            Routing::Azure {
                deployment,
                api_version,
            } => {
                format!(
                    "openai/deployments/{deployment}/chat/completions?api-version={api_version}"
                )
            }
        }
    }

    /// The path (including any query string) for the model-listing endpoint.
    fn models_path(&self) -> String {
        match self {
            Routing::Standard => "models".to_string(),
            Routing::Azure { api_version, .. } => {
                format!("openai/models?api-version={api_version}")
            }
        }
    }
}

/// The shared native OpenAI-compatible adapter. Built by each provider module
/// with its own base_url + auth + routing and default capabilities.
#[derive(Debug)]
pub struct NativeAdapter {
    id: String,
    client: HttpSseClient,
    auth: AuthStrategy,
    routing: Routing,
    /// Per-provider default capabilities (per-model refinement happens in
    /// [`ChatProvider::capabilities`]).
    default_caps: Capabilities,
}

/// Sensible defaults for OpenAI-family chat models: streaming + tools on,
/// vision/json_mode/max_context left best-effort/unknown (architecture.md
/// Section 4.1). Individual adapters may override.
pub fn default_openai_caps() -> Capabilities {
    Capabilities {
        streaming: true,
        tools: true,
        vision: false,
        json_mode: true,
        max_context: None,
    }
}

impl NativeAdapter {
    /// Build a native adapter. `base_url` roots the [`HttpSseClient`]; `auth`
    /// and `routing` capture the per-provider differences.
    pub fn new(
        id: impl Into<String>,
        base_url: impl Into<String>,
        auth: AuthStrategy,
        routing: Routing,
        default_caps: Capabilities,
    ) -> Self {
        NativeAdapter {
            id: id.into(),
            client: HttpSseClient::new(base_url),
            auth,
            routing,
            default_caps,
        }
    }

    /// The `Content-Type` + auth headers for a request.
    fn request_headers(&self) -> Vec<(String, String)> {
        let mut headers = vec![("Content-Type".to_string(), "application/json".to_string())];
        headers.extend(self.auth.headers());
        headers
    }

    /// Serialize a [`ChatRequest`] to the outbound JSON body, forcing the
    /// `stream` flag to `stream`.
    fn body(req: &ChatRequest, stream: bool) -> Result<serde_json::Value, ProviderError> {
        let mut value =
            serde_json::to_value(req).map_err(|e| ProviderError::Decode(e.to_string()))?;
        if let Some(obj) = value.as_object_mut() {
            obj.insert("stream".to_string(), serde_json::Value::Bool(stream));
            // `extra` is an internal passthrough container, not an OpenAI field;
            // splice its members up to the top level and drop the wrapper.
            if let Some(serde_json::Value::Object(extra_obj)) = obj.remove("extra") {
                for (k, v) in extra_obj {
                    obj.insert(k, v);
                }
            }
        }
        Ok(value)
    }
}

/// OpenAI `/models` list response shape (`{ "data": [ { "id": ... } ] }`).
#[derive(Debug, Deserialize)]
struct ModelsResponse {
    #[serde(default)]
    data: Vec<ModelEntry>,
}

#[derive(Debug, Deserialize)]
struct ModelEntry {
    id: String,
}

#[async_trait]
impl ChatProvider for NativeAdapter {
    fn id(&self) -> &str {
        &self.id
    }

    fn capabilities(&self, _model: &str) -> Capabilities {
        self.default_caps
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        // `/models` is a GET; use the shared client's `get_json` helper.
        let headers = self.request_headers();
        let models: ModelsResponse = self
            .client
            .get_json(&self.routing.models_path(), &headers)
            .await?;
        Ok(models
            .data
            .into_iter()
            .map(|m| ModelInfo::new(m.id))
            .collect())
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, ProviderError> {
        let headers = self.request_headers();
        let body = Self::body(&req, false)?;
        self.client
            .post_json(&self.routing.chat_path(), &headers, &body)
            .await
    }

    async fn chat_stream(
        &self,
        req: ChatRequest,
    ) -> Result<BoxStream<'static, Result<ChatDelta, ProviderError>>, ProviderError> {
        let headers = self.request_headers();
        let body = Self::body(&req, true)?;
        self.client
            .post_sse(
                &self.routing.chat_path(),
                &headers,
                &body,
                parse_stream_chunk,
            )
            .await
    }
}

/// Parse one OpenAI streaming chunk (`chat.completion.chunk`) into a
/// normalized [`ChatDelta`]. The OpenAI wire shape nests the delta under
/// `choices[0].delta`; this flattens it to our transport-agnostic delta and
/// skips chunks that carry no usable content (returning `Ok(None)`).
fn parse_stream_chunk(data: &str) -> Result<Option<ChatDelta>, ProviderError> {
    let chunk: StreamChunk =
        serde_json::from_str(data).map_err(|e| ProviderError::Decode(e.to_string()))?;
    let Some(choice) = chunk.choices.into_iter().next() else {
        return Ok(None);
    };

    let delta = ChatDelta {
        content: choice.delta.content,
        tool_calls: choice
            .delta
            .tool_calls
            .into_iter()
            .map(|tc| crate::contract::ToolCallDelta {
                index: tc.index,
                id: tc.id,
                function_name: tc.function.as_ref().and_then(|f| f.name.clone()),
                arguments_fragment: tc.function.and_then(|f| f.arguments),
            })
            .collect(),
        finish_reason: choice.finish_reason,
    };

    // Skip fully-empty deltas (e.g. the opening role-only chunk) so callers see
    // only meaningful increments.
    if delta.content.is_none() && delta.tool_calls.is_empty() && delta.finish_reason.is_none() {
        return Ok(None);
    }
    Ok(Some(delta))
}

/// The OpenAI streaming chunk envelope (only the fields we consume).
#[derive(Debug, Deserialize)]
struct StreamChunk {
    #[serde(default)]
    choices: Vec<StreamChoice>,
}

#[derive(Debug, Deserialize)]
struct StreamChoice {
    delta: StreamDelta,
    #[serde(default)]
    finish_reason: Option<crate::contract::FinishReason>,
}

#[derive(Debug, Default, Deserialize)]
struct StreamDelta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<StreamToolCall>,
}

#[derive(Debug, Deserialize)]
struct StreamToolCall {
    #[serde(default)]
    index: u32,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<StreamToolFn>,
}

#[derive(Debug, Deserialize)]
struct StreamToolFn {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{ChatMessage, FinishReason, MessageRole, ToolSpec};
    use serde_json::json;

    fn sample_request() -> ChatRequest {
        let mut r = ChatRequest::new("gpt-4o", vec![ChatMessage::text(MessageRole::User, "hi")]);
        r.tools = vec![ToolSpec::function("f", "d", json!({"type": "object"}))];
        r.extra = json!({"response_format": {"type": "json_object"}});
        r
    }

    #[test]
    fn body_forces_stream_flag_and_splices_extra() {
        let streamed = NativeAdapter::body(&sample_request(), true).unwrap();
        assert_eq!(streamed["stream"], json!(true));
        // `extra.response_format` is spliced to the top level; no `extra` key.
        assert!(streamed.get("extra").is_none());
        assert_eq!(streamed["response_format"], json!({"type": "json_object"}));
        assert_eq!(streamed["model"], json!("gpt-4o"));

        let non_streamed = NativeAdapter::body(&sample_request(), false).unwrap();
        assert_eq!(non_streamed["stream"], json!(false));
    }

    #[test]
    fn bearer_auth_headers() {
        let auth = AuthStrategy::Bearer("sk-123".to_string());
        assert_eq!(
            auth.headers(),
            vec![("Authorization".to_string(), "Bearer sk-123".to_string())]
        );
    }

    #[test]
    fn none_auth_sends_no_headers() {
        assert!(AuthStrategy::None.headers().is_empty());
    }

    #[test]
    fn api_key_header_auth() {
        let auth = AuthStrategy::ApiKeyHeader("azkey".to_string());
        assert_eq!(
            auth.headers(),
            vec![("api-key".to_string(), "azkey".to_string())]
        );
    }

    #[test]
    fn azure_routing_rewrites_path_and_adds_api_version() {
        let routing = Routing::Azure {
            deployment: "gpt4o-deploy".to_string(),
            api_version: "2024-02-01".to_string(),
        };
        assert_eq!(
            routing.chat_path(),
            "openai/deployments/gpt4o-deploy/chat/completions?api-version=2024-02-01"
        );
        assert_eq!(
            routing.models_path(),
            "openai/models?api-version=2024-02-01"
        );
    }

    #[test]
    fn standard_routing_paths() {
        assert_eq!(Routing::Standard.chat_path(), "chat/completions");
        assert_eq!(Routing::Standard.models_path(), "models");
    }

    #[test]
    fn parse_stream_chunk_flattens_delta() {
        let data = json!({
            "choices": [{"delta": {"content": "Hel"}}]
        })
        .to_string();
        let delta = parse_stream_chunk(&data).unwrap().unwrap();
        assert_eq!(delta.content.as_deref(), Some("Hel"));
        assert!(delta.tool_calls.is_empty());
    }

    #[test]
    fn parse_stream_chunk_skips_role_only_opener() {
        let data = json!({"choices": [{"delta": {"role": "assistant"}}]}).to_string();
        assert!(parse_stream_chunk(&data).unwrap().is_none());
    }

    #[test]
    fn parse_stream_chunk_extracts_finish_reason_and_tool_calls() {
        let data = json!({
            "choices": [{
                "delta": {"tool_calls": [{
                    "index": 0,
                    "id": "call_1",
                    "function": {"name": "get_weather", "arguments": "{\"c"}
                }]},
                "finish_reason": "tool_calls"
            }]
        })
        .to_string();
        let delta = parse_stream_chunk(&data).unwrap().unwrap();
        assert_eq!(delta.finish_reason, Some(FinishReason::ToolCalls));
        assert_eq!(delta.tool_calls.len(), 1);
        assert_eq!(delta.tool_calls[0].id.as_deref(), Some("call_1"));
        assert_eq!(
            delta.tool_calls[0].function_name.as_deref(),
            Some("get_weather")
        );
        assert_eq!(
            delta.tool_calls[0].arguments_fragment.as_deref(),
            Some("{\"c")
        );
    }
}
