//! AWS Bedrock translation shim with SigV4 signing (architecture.md Section
//! 4.3, tasks.md P2.9).
//!
//! Bedrock exposes provider-specific model bodies over a SigV4-signed HTTP
//! endpoint (no OpenAI-compatible surface), so this is a *translation shim*: it
//! presents the internal OpenAI-compatible [`ChatProvider`] contract while
//!
//!   1. shaping the request body per model family (Anthropic-on-Bedrock vs
//!      Amazon Titan) selected by the model id,
//!   2. signing the request with AWS SigV4 (region via `extra.region`),
//!   3. normalizing the Bedrock response / event stream back into
//!      [`ChatResponse`] / [`ChatDelta`].
//!
//! ## Credentials (architecture.md Section 4.3 / 9.1)
//!
//! Primary path: AWS access keys entered in the app are stored as a
//! [`secrets::SecretRef`] in the OS keychain exactly like every other provider
//! key and resolved through the keystore at build time. The stored secret is a
//! JSON blob `{ "access_key_id": ..., "secret_access_key": ..., (optional)
//! "session_token": ... }`.
//!
//! Secondary path: when NO `api_key_ref` is configured, the standard AWS
//! credential provider chain (environment variables, shared config/profile,
//! instance/SSO roles) is used. When that path is taken, no secret enters the
//! app at all. In both cases raw credentials never cross the IPC boundary.

use std::sync::Arc;
use std::time::SystemTime;

use async_trait::async_trait;
use aws_credential_types::Credentials;
use aws_sigv4::http_request::{sign, SignableBody, SignableRequest, SigningSettings};
use aws_sigv4::sign::v4;
use futures_util::stream::{BoxStream, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};

use domain::{ProviderConfig, ProviderKind};
use secrets::SecretStore;

use crate::contract::{
    Capabilities, ChatChoice, ChatDelta, ChatMessage, ChatProvider, ChatRequest, ChatResponse,
    FinishReason, MessageRole, ModelInfo, ProviderError, Usage,
};
use crate::registry::ProviderFactory;

/// The AWS service name Bedrock Runtime signs under.
const SERVICE: &str = "bedrock";

/// AWS credentials resolved for signing. Held only inside the adapter; never
/// stored on the config and never crossing IPC.
#[derive(Debug, Clone)]
pub struct AwsCredentials {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: Option<String>,
}

/// How the adapter obtained its AWS credentials (architecture.md Section 4.3).
///
/// `SecretRef` is the primary path (keys entered in-app, stored in the OS
/// keychain); `ProviderChain` is the optional secondary path used when no
/// `api_key_ref` is configured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialSource {
    /// Resolved from the keystore via a [`secrets::SecretRef`] (primary).
    SecretRef,
    /// Resolved from the standard AWS provider chain (secondary).
    ProviderChain,
}

/// Which model family the Bedrock model id belongs to; selects body shaping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelFamily {
    /// Anthropic Claude on Bedrock (Messages-style body).
    Anthropic,
    /// Amazon Titan text (inputText / textGenerationConfig body).
    Titan,
}

impl ModelFamily {
    /// Classify a Bedrock model id (e.g. `anthropic.claude-3-5-sonnet-20240620-v1:0`,
    /// `amazon.titan-text-express-v1`).
    pub fn from_model_id(model: &str) -> ModelFamily {
        if model.starts_with("amazon.titan") || model.contains("titan") {
            ModelFamily::Titan
        } else {
            // Default to the Anthropic-on-Bedrock body shape, the most common.
            ModelFamily::Anthropic
        }
    }
}

/// The Bedrock translation shim.
#[derive(Debug)]
pub struct BedrockAdapter {
    id: String,
    base_url: String,
    region: String,
    credentials: AwsCredentials,
    credential_source: CredentialSource,
    http: reqwest::Client,
}

impl BedrockAdapter {
    /// Construct the adapter with resolved credentials + region. Credential
    /// material lives only inside this value.
    pub fn new(
        id: impl Into<String>,
        base_url: impl Into<String>,
        region: impl Into<String>,
        credentials: AwsCredentials,
        credential_source: CredentialSource,
    ) -> Self {
        // Ensure the rustls default crypto provider (`ring`) is installed before
        // building the reqwest client; see `crate::crypto`.
        crate::crypto::ensure_crypto_provider();
        BedrockAdapter {
            id: id.into(),
            base_url: base_url.into(),
            region: region.into(),
            credentials,
            credential_source,
            http: reqwest::Client::new(),
        }
    }

    /// Which credential path this adapter used (test/observability accessor).
    pub fn credential_source(&self) -> &CredentialSource {
        &self.credential_source
    }

    /// The Bedrock Runtime path for a model + streaming flag.
    fn path(model: &str, stream: bool) -> String {
        let action = if stream {
            "invoke-with-response-stream"
        } else {
            "invoke"
        };
        format!("/model/{model}/{action}")
    }

    /// Shape the internal [`ChatRequest`] into the per-model native Bedrock
    /// body (architecture.md Section 4.3).
    fn shape_body(req: &ChatRequest) -> Value {
        match ModelFamily::from_model_id(&req.model) {
            ModelFamily::Anthropic => shape_anthropic_body(req),
            ModelFamily::Titan => shape_titan_body(req),
        }
    }

    /// Build, SigV4-sign, and dispatch a request; return the raw response bytes.
    async fn signed_request(
        &self,
        model: &str,
        stream: bool,
        body: &Value,
    ) -> Result<reqwest::Response, ProviderError> {
        let url = format!(
            "{}{}",
            self.base_url.trim_end_matches('/'),
            Self::path(model, stream)
        );
        let body_bytes =
            serde_json::to_vec(body).map_err(|e| ProviderError::Decode(e.to_string()))?;

        let headers = sign_request(
            &self.credentials,
            &self.region,
            "POST",
            &url,
            &body_bytes,
            SystemTime::now(),
        )?;

        let mut builder = self
            .http
            .post(&url)
            .header("content-type", "application/json");
        // Bedrock streaming uses the AWS event-stream accept type.
        if stream {
            builder = builder.header("accept", "application/vnd.amazon.eventstream");
        } else {
            builder = builder.header("accept", "application/json");
        }
        for (name, value) in headers {
            builder = builder.header(name, value);
        }

        let resp = builder
            .body(body_bytes)
            .send()
            .await
            .map_err(|e| ProviderError::Transport(e.to_string()))?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(ProviderError::HttpStatus {
                status: status.as_u16(),
                body: text,
            });
        }
        Ok(resp)
    }

    fn caps_for(model: &str) -> Capabilities {
        let anthropic = matches!(ModelFamily::from_model_id(model), ModelFamily::Anthropic);
        Capabilities {
            streaming: true,
            // Tool calling is NOT wired for Bedrock in Phase 2: `shape_*_body`
            // never serializes a `tools` array and the normalizers discard any
            // `tool_use` block. Advertising `tools: true` would let capability
            // negotiation route a tool-requiring request here, and the tools
            // would silently vanish (review issue 2). Report `false` so
            // negotiation strips tools honestly until Bedrock tool mapping is
            // wired (a later phase). The direct Anthropic adapter DOES support
            // tools; only Bedrock-Anthropic is gated here.
            tools: false,
            vision: anthropic && model.contains("claude-3"),
            json_mode: false,
            max_context: None,
        }
    }
}

/// Shape an Anthropic-on-Bedrock body (Messages-style, with the required
/// `anthropic_version` field and `max_tokens`).
fn shape_anthropic_body(req: &ChatRequest) -> Value {
    let mut system_parts: Vec<String> = Vec::new();
    let mut messages: Vec<Value> = Vec::new();
    for msg in &req.messages {
        match msg.role {
            MessageRole::System => {
                if let Some(text) = &msg.content {
                    system_parts.push(text.clone());
                }
            }
            MessageRole::User | MessageRole::Tool => {
                messages.push(json!({
                    "role": "user",
                    "content": [{"type": "text", "text": msg.content.clone().unwrap_or_default()}],
                }));
            }
            MessageRole::Assistant => {
                messages.push(json!({
                    "role": "assistant",
                    "content": [{"type": "text", "text": msg.content.clone().unwrap_or_default()}],
                }));
            }
        }
    }
    let mut body = json!({
        "anthropic_version": "bedrock-2023-05-31",
        "messages": messages,
        "max_tokens": req.max_tokens.unwrap_or(4096),
    });
    let obj = body.as_object_mut().expect("object");
    if !system_parts.is_empty() {
        obj.insert("system".to_string(), json!(system_parts.join("\n\n")));
    }
    if let Some(temp) = req.temperature {
        obj.insert("temperature".to_string(), json!(temp));
    }
    body
}

/// Shape an Amazon Titan text body (`inputText` + `textGenerationConfig`).
fn shape_titan_body(req: &ChatRequest) -> Value {
    // Titan has no multi-turn message array; concatenate the conversation into
    // a single prompt string.
    let mut prompt = String::new();
    for msg in &req.messages {
        if let Some(text) = &msg.content {
            let tag = match msg.role {
                MessageRole::System => "System",
                MessageRole::User | MessageRole::Tool => "User",
                MessageRole::Assistant => "Bot",
            };
            prompt.push_str(&format!("{tag}: {text}\n"));
        }
    }
    let mut config = serde_json::Map::new();
    if let Some(max) = req.max_tokens {
        config.insert("maxTokenCount".to_string(), json!(max));
    }
    if let Some(temp) = req.temperature {
        config.insert("temperature".to_string(), json!(temp));
    }
    json!({
        "inputText": prompt,
        "textGenerationConfig": Value::Object(config),
    })
}

/// SigV4-sign a request and return the signing headers to attach (including the
/// `Authorization` header). Extracted as a pure function of its inputs so tests
/// can assert a DETERMINISTIC signature from a fixed credential + region + time.
///
/// `time` is the signing instant; passing a fixed value makes the produced
/// `Authorization` signature deterministic (SigV4 folds the timestamp into the
/// string-to-sign).
pub fn sign_request(
    creds: &AwsCredentials,
    region: &str,
    http_method: &str,
    url: &str,
    body: &[u8],
    time: SystemTime,
) -> Result<Vec<(String, String)>, ProviderError> {
    let identity = Credentials::new(
        creds.access_key_id.clone(),
        creds.secret_access_key.clone(),
        creds.session_token.clone(),
        None,
        "providers-bedrock",
    )
    .into();

    let signing_settings = SigningSettings::default();
    let signing_params = v4::SigningParams::builder()
        .identity(&identity)
        .region(region)
        .name(SERVICE)
        .time(time)
        .settings(signing_settings)
        .build()
        .map_err(|e| ProviderError::Other(format!("sigv4 params: {e}")))?
        .into();

    // Only the headers SigV4 needs to sign are supplied here; the host header is
    // required for a valid signature.
    let host = url_host(url)
        .ok_or_else(|| ProviderError::Other(format!("invalid url for signing: {url}")))?;
    let headers_to_sign = [("host", host.as_str())];

    let signable = SignableRequest::new(
        http_method,
        url,
        headers_to_sign.iter().copied(),
        SignableBody::Bytes(body),
    )
    .map_err(|e| ProviderError::Other(format!("sigv4 signable: {e}")))?;

    let signing_output = sign(signable, &signing_params)
        .map_err(|e| ProviderError::Other(format!("sigv4 sign: {e}")))?;
    let (instructions, _signature) = signing_output.into_parts();

    // Collect the headers SigV4 instructs us to add (Authorization,
    // X-Amz-Date, X-Amz-Security-Token when a session token is present).
    let mut out: Vec<(String, String)> = instructions
        .headers()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect();
    // Ensure the signed host header is present on the wire.
    out.push(("host".to_string(), host));
    Ok(out)
}

/// Extract the `host[:port]` authority from a URL for the SigV4 host header.
fn url_host(url: &str) -> Option<String> {
    let after_scheme = url.split("://").nth(1)?;
    let authority = after_scheme.split(['/', '?']).next()?;
    if authority.is_empty() {
        None
    } else {
        Some(authority.to_string())
    }
}

// ---- Non-streaming response normalization ----------------------------------

/// Normalize a raw Bedrock model response body into a [`ChatResponse`],
/// dispatching on the model family that produced it.
fn normalize_response(model: &str, raw: &Value) -> ChatResponse {
    match ModelFamily::from_model_id(model) {
        ModelFamily::Anthropic => normalize_anthropic(raw),
        ModelFamily::Titan => normalize_titan(raw),
    }
}

#[derive(Debug, Deserialize)]
struct AnthropicBody {
    #[serde(default)]
    content: Vec<AnthropicContentBlock>,
    #[serde(default)]
    stop_reason: Option<String>,
    #[serde(default)]
    usage: Option<AnthropicUsage>,
}

#[derive(Debug, Deserialize)]
struct AnthropicContentBlock {
    #[serde(default)]
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AnthropicUsage {
    #[serde(default)]
    input_tokens: u32,
    #[serde(default)]
    output_tokens: u32,
}

fn normalize_anthropic(raw: &Value) -> ChatResponse {
    let body: AnthropicBody = serde_json::from_value(raw.clone()).unwrap_or(AnthropicBody {
        content: vec![],
        stop_reason: None,
        usage: None,
    });
    let text: String = body.content.iter().filter_map(|b| b.text.clone()).collect();
    let finish_reason = match body.stop_reason.as_deref() {
        Some("end_turn") | Some("stop_sequence") => Some(FinishReason::Stop),
        Some("max_tokens") => Some(FinishReason::Length),
        Some("tool_use") => Some(FinishReason::ToolCalls),
        _ => None,
    };
    let usage = body.usage.map(|u| Usage {
        prompt_tokens: u.input_tokens,
        completion_tokens: u.output_tokens,
        total_tokens: u.input_tokens + u.output_tokens,
    });
    single_choice(text, finish_reason, usage)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TitanBody {
    /// Prompt token count Titan reports at the top level of an invoke response.
    #[serde(default)]
    input_text_token_count: u32,
    #[serde(default)]
    results: Vec<TitanResult>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TitanResult {
    #[serde(default)]
    output_text: String,
    #[serde(default)]
    completion_reason: Option<String>,
    #[serde(default)]
    token_count: u32,
}

fn normalize_titan(raw: &Value) -> ChatResponse {
    let body: TitanBody = serde_json::from_value(raw.clone()).unwrap_or(TitanBody {
        input_text_token_count: 0,
        results: vec![],
    });
    // Titan reports prompt tokens at the top level (`inputTextTokenCount`);
    // populate `prompt_tokens` from it so `total_tokens` includes the prompt and
    // cost signals do not undercount (review issue 8). Absent (0) when the field
    // is not present.
    let prompt_tokens = body.input_text_token_count;
    let (text, reason, completion_tokens) = match body.results.into_iter().next() {
        Some(r) => (r.output_text, r.completion_reason, r.token_count),
        None => (String::new(), None, 0),
    };
    let finish_reason = match reason.as_deref() {
        Some("FINISH") | Some("COMPLETE") => Some(FinishReason::Stop),
        Some("LENGTH") | Some("MAX_TOKENS") => Some(FinishReason::Length),
        Some("CONTENT_FILTERED") => Some(FinishReason::ContentFilter),
        _ => None,
    };
    let usage = Some(Usage {
        prompt_tokens,
        completion_tokens,
        total_tokens: prompt_tokens + completion_tokens,
    });
    single_choice(text, finish_reason, usage)
}

/// Build a single-choice [`ChatResponse`] from normalized fields.
fn single_choice(
    text: String,
    finish_reason: Option<FinishReason>,
    usage: Option<Usage>,
) -> ChatResponse {
    ChatResponse {
        choices: vec![ChatChoice {
            index: 0,
            message: ChatMessage {
                role: MessageRole::Assistant,
                content: if text.is_empty() { None } else { Some(text) },
                tool_calls: vec![],
                tool_call_id: None,
                name: None,
            },
            finish_reason,
        }],
        usage,
        model: None,
    }
}

// ---- Event-stream normalization --------------------------------------------

/// Normalize one decoded Bedrock event-stream chunk payload (the inner JSON of
/// an event) into an internal [`ChatDelta`], dispatching on model family.
///
/// Bedrock's `invoke-with-response-stream` wraps each event in the binary AWS
/// event-stream framing; the inner payload for a `chunk` event is a
/// base64-`bytes` blob whose decoded content is the model's native streaming
/// JSON. This normalizes that decoded JSON. The binary de-framing lives in
/// [`decode_event_stream`].
fn normalize_stream_chunk(model: &str, payload: &Value) -> Option<ChatDelta> {
    match ModelFamily::from_model_id(model) {
        ModelFamily::Anthropic => {
            // Anthropic-on-Bedrock reuses the Messages streaming event shape.
            let kind = payload.get("type").and_then(|v| v.as_str());
            match kind {
                Some("content_block_delta") => {
                    let text = payload
                        .get("delta")
                        .and_then(|d| d.get("text"))
                        .and_then(|t| t.as_str())?;
                    Some(ChatDelta {
                        content: Some(text.to_string()),
                        ..ChatDelta::default()
                    })
                }
                Some("message_delta") => {
                    let reason = payload
                        .get("delta")
                        .and_then(|d| d.get("stop_reason"))
                        .and_then(|s| s.as_str());
                    let finish = match reason {
                        Some("end_turn") | Some("stop_sequence") => Some(FinishReason::Stop),
                        Some("max_tokens") => Some(FinishReason::Length),
                        Some("tool_use") => Some(FinishReason::ToolCalls),
                        _ => None,
                    };
                    finish.map(|f| ChatDelta {
                        finish_reason: Some(f),
                        ..ChatDelta::default()
                    })
                }
                _ => None,
            }
        }
        ModelFamily::Titan => {
            let text = payload.get("outputText").and_then(|t| t.as_str());
            let reason = payload.get("completionReason").and_then(|r| r.as_str());
            let finish = match reason {
                Some("FINISH") | Some("COMPLETE") => Some(FinishReason::Stop),
                Some("LENGTH") | Some("MAX_TOKENS") => Some(FinishReason::Length),
                _ => None,
            };
            match (text, finish) {
                (None, None) => None,
                (t, f) => Some(ChatDelta {
                    content: t.filter(|s| !s.is_empty()).map(|s| s.to_string()),
                    tool_calls: vec![],
                    finish_reason: f,
                }),
            }
        }
    }
}

/// Decode the binary AWS event-stream framing into the sequence of inner event
/// payload JSON values (architecture.md Section 4.3: "translate event stream").
///
/// Frame layout (AWS event-stream / `vnd.amazon.eventstream`):
///   `[total_len:u32][headers_len:u32][prelude_crc:u32][headers...][payload...][message_crc:u32]`
/// The payload for a Bedrock `chunk` event is `{"bytes": "<base64 model JSON>"}`;
/// this decodes the outer framing and the inner base64 to yield the model's
/// native streaming JSON. CRCs are not validated here (the transport already
/// guarantees integrity); framing lengths are.
pub fn decode_event_stream(buf: &[u8]) -> Result<Vec<Value>, ProviderError> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos + 12 <= buf.len() {
        match decode_one_frame(&buf[pos..])? {
            Some((value, consumed)) => {
                out.push(value);
                pos += consumed;
            }
            // Not enough bytes for a full frame; for a complete buffer this is a
            // truncation error (mirrors the previous behavior).
            None => {
                return Err(ProviderError::Decode(
                    "truncated event-stream frame".to_string(),
                ));
            }
        }
    }
    Ok(out)
}

/// Decode the FIRST event-stream frame at the start of `buf`, returning the
/// inner payload JSON and how many bytes the frame consumed, or `Ok(None)` when
/// `buf` does not yet hold a complete frame (the incremental caller should read
/// more bytes). Frame-length inconsistencies still surface as `Err`.
///
/// Shared by [`decode_event_stream`] (whole-buffer) and the incremental
/// streaming decoder so both apply identical framing rules.
fn decode_one_frame(buf: &[u8]) -> Result<Option<(Value, usize)>, ProviderError> {
    if buf.len() < 12 {
        return Ok(None);
    }
    let total_len = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
    let headers_len = u32::from_be_bytes([buf[4], buf[5], buf[6], buf[7]]) as usize;
    if total_len < 16 {
        return Err(ProviderError::Decode(
            "invalid event-stream frame lengths".to_string(),
        ));
    }
    if buf.len() < total_len {
        // Frame prelude is present but the body has not fully arrived yet.
        return Ok(None);
    }
    // Payload sits after the 12-byte prelude + headers, before the trailing
    // 4-byte message CRC.
    let payload_start = 12 + headers_len;
    let payload_end = total_len - 4;
    if payload_start > payload_end || payload_end > buf.len() {
        return Err(ProviderError::Decode(
            "invalid event-stream frame lengths".to_string(),
        ));
    }
    let payload = &buf[payload_start..payload_end];
    let outer: Value =
        serde_json::from_slice(payload).map_err(|e| ProviderError::Decode(e.to_string()))?;
    // A `chunk` event wraps base64 model JSON under `bytes`; decode it.
    let value = if let Some(b64) = outer.get("bytes").and_then(|b| b.as_str()) {
        let decoded = base64_decode(b64)
            .ok_or_else(|| ProviderError::Decode("invalid base64 in chunk".to_string()))?;
        serde_json::from_slice(&decoded).map_err(|e| ProviderError::Decode(e.to_string()))?
    } else {
        outer
    };
    Ok(Some((value, total_len)))
}

/// State threaded through the incremental Bedrock event-stream `unfold`.
struct BedrockStreamState {
    stream: BoxStream<'static, Result<bytes::Bytes, ProviderError>>,
    model: String,
    buf: Vec<u8>,
    /// Deltas already decoded from complete frames in `buf`, awaiting emission.
    pending: std::collections::VecDeque<ChatDelta>,
    done: bool,
}

/// Turn a byte stream of AWS event-stream framing into a stream of normalized
/// [`ChatDelta`]s, de-framing INCREMENTALLY: each complete frame is decoded and
/// its delta emitted as soon as its bytes arrive, without buffering the whole
/// response (review issue 3). Partial trailing bytes are retained until the
/// rest of the frame arrives.
fn bedrock_event_stream<S>(
    byte_stream: S,
    model: String,
) -> BoxStream<'static, Result<ChatDelta, ProviderError>>
where
    S: futures_util::Stream<Item = Result<bytes::Bytes, ProviderError>> + Send + 'static,
{
    let init = BedrockStreamState {
        stream: byte_stream.boxed(),
        model,
        buf: Vec::new(),
        pending: std::collections::VecDeque::new(),
        done: false,
    };

    futures_util::stream::unfold(init, |mut state| async move {
        loop {
            if let Some(delta) = state.pending.pop_front() {
                return Some((Ok(delta), state));
            }
            if state.done {
                return None;
            }
            match state.stream.next().await {
                Some(Ok(chunk)) => {
                    state.buf.extend_from_slice(&chunk);
                    // Drain every complete frame now buffered.
                    let mut consumed_total = 0usize;
                    loop {
                        match decode_one_frame(&state.buf[consumed_total..]) {
                            Ok(Some((payload, consumed))) => {
                                consumed_total += consumed;
                                if let Some(delta) = normalize_stream_chunk(&state.model, &payload)
                                {
                                    state.pending.push_back(delta);
                                }
                            }
                            Ok(None) => break, // need more bytes
                            Err(e) => {
                                state.done = true;
                                if consumed_total > 0 {
                                    state.buf.drain(..consumed_total);
                                }
                                return Some((Err(e), state));
                            }
                        }
                    }
                    if consumed_total > 0 {
                        state.buf.drain(..consumed_total);
                    }
                }
                Some(Err(e)) => {
                    state.done = true;
                    return Some((Err(e), state));
                }
                None => {
                    // Byte stream ended. Any leftover bytes that are not a full
                    // frame are dropped (a well-formed Bedrock stream ends on a
                    // frame boundary). The stream terminates here, so `state`
                    // is not returned and needs no further mutation.
                    return None;
                }
            }
        }
    })
    .boxed()
}

/// Minimal, dependency-free standard base64 decoder (Bedrock chunk payloads are
/// standard base64). Returns `None` on malformed input.
fn base64_decode(input: &str) -> Option<Vec<u8>> {
    fn val(c: u8) -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let bytes: Vec<u8> = input
        .bytes()
        .filter(|b| *b != b'\n' && *b != b'\r')
        .collect();
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    let mut chunk = [0u8; 4];
    let mut i = 0;
    for &b in &bytes {
        if b == b'=' {
            break;
        }
        chunk[i] = val(b)?;
        i += 1;
        if i == 4 {
            out.push((chunk[0] << 2) | (chunk[1] >> 4));
            out.push((chunk[1] << 4) | (chunk[2] >> 2));
            out.push((chunk[2] << 6) | chunk[3]);
            i = 0;
        }
    }
    match i {
        0 => {}
        2 => out.push((chunk[0] << 2) | (chunk[1] >> 4)),
        3 => {
            out.push((chunk[0] << 2) | (chunk[1] >> 4));
            out.push((chunk[1] << 4) | (chunk[2] >> 2));
        }
        _ => return None,
    }
    Some(out)
}

#[async_trait]
impl ChatProvider for BedrockAdapter {
    fn id(&self) -> &str {
        &self.id
    }

    fn capabilities(&self, model: &str) -> Capabilities {
        Self::caps_for(model)
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        // Bedrock model discovery lives on the control-plane `bedrock` API, not
        // the runtime endpoint; return the well-known runtime ids so the UI can
        // populate a picker without a control-plane round-trip.
        Ok(vec![
            ModelInfo::new("anthropic.claude-3-5-sonnet-20240620-v1:0"),
            ModelInfo::new("anthropic.claude-3-haiku-20240307-v1:0"),
            ModelInfo::new("amazon.titan-text-express-v1"),
        ])
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, ProviderError> {
        let body = Self::shape_body(&req);
        let resp = self.signed_request(&req.model, false, &body).await?;
        let raw: Value = resp
            .json()
            .await
            .map_err(|e| ProviderError::Decode(e.to_string()))?;
        Ok(normalize_response(&req.model, &raw))
    }

    async fn chat_stream(
        &self,
        req: ChatRequest,
    ) -> Result<BoxStream<'static, Result<ChatDelta, ProviderError>>, ProviderError> {
        let body = Self::shape_body(&req);
        let resp = self.signed_request(&req.model, true, &body).await?;
        let model = req.model.clone();
        // Stream the AWS event-stream framing INCREMENTALLY off `bytes_stream()`
        // rather than buffering the whole body first (review issue 3): each
        // complete frame is de-framed and yielded as soon as its bytes arrive.
        let byte_stream = resp
            .bytes_stream()
            .map(|chunk| chunk.map_err(|e| ProviderError::Transport(e.to_string())));
        Ok(bedrock_event_stream(byte_stream, model))
    }
}

/// Resolve AWS credentials for the adapter, preferring the keystore
/// [`secrets::SecretRef`] path (primary) and falling back to the AWS provider
/// chain when no `api_key_ref` is configured (secondary). See the module docs
/// and architecture.md Section 4.3.
fn resolve_credentials(
    cfg: &ProviderConfig,
    secrets: &dyn SecretStore,
) -> Result<(AwsCredentials, CredentialSource), ProviderError> {
    match &cfg.api_key_ref {
        Some(key_ref) => {
            // PRIMARY: resolve the JSON credential blob from the keystore.
            let blob = secrets
                .resolve(key_ref)
                .map_err(|e| ProviderError::Auth(e.to_string()))?;
            let creds = parse_credential_blob(&blob)?;
            Ok((creds, CredentialSource::SecretRef))
        }
        None => {
            // SECONDARY: the standard AWS provider chain (env / shared config /
            // instance role). We read the well-known environment variables here
            // (no network); a full provider-chain resolver can replace this
            // without changing the credential-source contract.
            let creds = resolve_from_env()?;
            Ok((creds, CredentialSource::ProviderChain))
        }
    }
}

/// Parse the keystore credential blob `{access_key_id, secret_access_key,
/// session_token?}` into [`AwsCredentials`].
fn parse_credential_blob(blob: &str) -> Result<AwsCredentials, ProviderError> {
    #[derive(Deserialize)]
    struct Blob {
        access_key_id: String,
        secret_access_key: String,
        #[serde(default)]
        session_token: Option<String>,
    }
    let parsed: Blob = serde_json::from_str(blob).map_err(|e| {
        ProviderError::Auth(format!("invalid AWS credential blob in keystore: {e}"))
    })?;
    Ok(AwsCredentials {
        access_key_id: parsed.access_key_id,
        secret_access_key: parsed.secret_access_key,
        session_token: parsed.session_token,
    })
}

/// Read AWS credentials from the standard environment variables (the entry
/// point of the AWS provider chain that requires no network).
fn resolve_from_env() -> Result<AwsCredentials, ProviderError> {
    let access_key_id = std::env::var("AWS_ACCESS_KEY_ID").map_err(|_| {
        ProviderError::Auth(
            "no api_key_ref configured and AWS_ACCESS_KEY_ID is unset (AWS provider chain)"
                .to_string(),
        )
    })?;
    let secret_access_key = std::env::var("AWS_SECRET_ACCESS_KEY").map_err(|_| {
        ProviderError::Auth(
            "no api_key_ref configured and AWS_SECRET_ACCESS_KEY is unset (AWS provider chain)"
                .to_string(),
        )
    })?;
    let session_token = std::env::var("AWS_SESSION_TOKEN").ok();
    Ok(AwsCredentials {
        access_key_id,
        secret_access_key,
        session_token,
    })
}

/// Compute the default Bedrock Runtime endpoint for a region.
fn default_base_url(region: &str) -> String {
    format!("https://bedrock-runtime.{region}.amazonaws.com")
}

/// Build the Bedrock adapter from a [`ProviderConfig`], resolving credentials
/// (SecretRef primary, AWS provider chain secondary) and the region from
/// `extra.region`.
pub fn build_bedrock(
    cfg: &ProviderConfig,
    secrets: &dyn SecretStore,
) -> Result<BedrockAdapter, ProviderError> {
    let region = cfg
        .extra
        .get("region")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ProviderError::Other("Bedrock provider requires extra.region".to_string()))?
        .to_string();
    let (credentials, source) = resolve_credentials(cfg, secrets)?;
    let base_url = cfg
        .base_url
        .clone()
        .unwrap_or_else(|| default_base_url(&region));
    Ok(BedrockAdapter::new(
        cfg.id.clone(),
        base_url,
        region,
        credentials,
        source,
    ))
}

/// [`ProviderFactory`] for [`ProviderKind::Bedrock`].
pub struct BedrockFactory;

impl ProviderFactory for BedrockFactory {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Bedrock
    }

    fn build(
        &self,
        cfg: &ProviderConfig,
        secrets: &dyn SecretStore,
    ) -> Result<Arc<dyn ChatProvider>, ProviderError> {
        Ok(Arc::new(build_bedrock(cfg, secrets)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use secrets::InMemorySecretStore;
    use std::time::{Duration, UNIX_EPOCH};

    fn fixed_creds() -> AwsCredentials {
        AwsCredentials {
            access_key_id: "AKIDEXAMPLE".to_string(),
            secret_access_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY".to_string(),
            session_token: None,
        }
    }

    /// A fixed signing instant so the SigV4 signature is deterministic.
    fn fixed_time() -> SystemTime {
        // 2015-08-30T12:36:00Z, the canonical AWS SigV4 test vector timestamp.
        UNIX_EPOCH + Duration::from_secs(1_440_938_160)
    }

    #[test]
    fn model_family_classification() {
        assert_eq!(
            ModelFamily::from_model_id("anthropic.claude-3-5-sonnet-20240620-v1:0"),
            ModelFamily::Anthropic
        );
        assert_eq!(
            ModelFamily::from_model_id("amazon.titan-text-express-v1"),
            ModelFamily::Titan
        );
    }

    #[test]
    fn shape_body_selects_per_model_shape() {
        let mut anth = ChatRequest::new(
            "anthropic.claude-3-5-sonnet-20240620-v1:0",
            vec![
                ChatMessage::text(MessageRole::System, "sys"),
                ChatMessage::text(MessageRole::User, "hi"),
            ],
        );
        anth.max_tokens = Some(256);
        let body = BedrockAdapter::shape_body(&anth);
        assert_eq!(body["anthropic_version"], json!("bedrock-2023-05-31"));
        assert_eq!(body["system"], json!("sys"));
        assert_eq!(body["max_tokens"], json!(256));
        assert_eq!(body["messages"][0]["role"], json!("user"));

        let titan = ChatRequest::new(
            "amazon.titan-text-express-v1",
            vec![ChatMessage::text(MessageRole::User, "hi")],
        );
        let body = BedrockAdapter::shape_body(&titan);
        assert!(body["inputText"].as_str().unwrap().contains("User: hi"));
        assert!(body.get("textGenerationConfig").is_some());
    }

    #[test]
    fn sigv4_signature_is_deterministic_from_fixed_inputs() {
        let headers = sign_request(
            &fixed_creds(),
            "us-east-1",
            "POST",
            "https://bedrock-runtime.us-east-1.amazonaws.com/model/amazon.titan-text-express-v1/invoke",
            b"{\"inputText\":\"hi\"}",
            fixed_time(),
        )
        .unwrap();

        let auth = headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("authorization"))
            .map(|(_, v)| v.clone())
            .expect("Authorization header present");

        // The Authorization header must reference our credential, the correct
        // scope (date/region/service), and be deterministic for fixed inputs.
        assert!(auth.starts_with("AWS4-HMAC-SHA256 "));
        assert!(auth.contains("Credential=AKIDEXAMPLE/20150830/us-east-1/bedrock/aws4_request"));
        assert!(auth.contains("SignedHeaders="));
        assert!(auth.contains("host"));
        assert!(auth.contains("Signature="));

        // Extract the signature hex and assert it is stable (deterministic).
        let sig = auth.split("Signature=").nth(1).unwrap().trim().to_string();
        assert_eq!(sig.len(), 64, "SigV4 signature is a 64-char hex digest");

        // Re-signing the identical inputs must reproduce the same signature.
        let headers2 = sign_request(
            &fixed_creds(),
            "us-east-1",
            "POST",
            "https://bedrock-runtime.us-east-1.amazonaws.com/model/amazon.titan-text-express-v1/invoke",
            b"{\"inputText\":\"hi\"}",
            fixed_time(),
        )
        .unwrap();
        let auth2 = headers2
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("authorization"))
            .map(|(_, v)| v.clone())
            .unwrap();
        assert_eq!(auth, auth2, "signing is deterministic for fixed inputs");

        // An X-Amz-Date header is emitted for the fixed instant.
        assert!(headers
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("x-amz-date") && v == "20150830T123600Z"));
    }

    #[test]
    fn credential_resolution_prefers_secret_ref() {
        // PRIMARY path: a configured api_key_ref resolves through the keystore.
        let store = InMemorySecretStore::new();
        let blob = json!({
            "access_key_id": "AKIA_FROM_KEYSTORE",
            "secret_access_key": "secret_from_keystore"
        })
        .to_string();
        let key_ref = store.store("bedrock-creds", &blob).unwrap();
        let cfg = ProviderConfig {
            id: "bedrock".to_string(),
            kind: ProviderKind::Bedrock,
            base_url: None,
            api_key_ref: Some(key_ref),
            extra: json!({"region": "us-east-1"}),
        };
        let (creds, source) = resolve_credentials(&cfg, &store).unwrap();
        assert_eq!(source, CredentialSource::SecretRef);
        assert_eq!(creds.access_key_id, "AKIA_FROM_KEYSTORE");
        assert_eq!(creds.secret_access_key, "secret_from_keystore");

        // The full build picks the keystore path and the region.
        let adapter = build_bedrock(&cfg, &store).unwrap();
        assert_eq!(adapter.credential_source(), &CredentialSource::SecretRef);
        assert_eq!(adapter.region, "us-east-1");
        assert_eq!(
            adapter.base_url,
            "https://bedrock-runtime.us-east-1.amazonaws.com"
        );
    }

    #[test]
    fn credential_resolution_falls_back_to_provider_chain() {
        // SECONDARY path: with no api_key_ref, the AWS provider chain is used.
        // We stub the chain's env entry point with fixed values; asserting the
        // SELECTED SOURCE is ProviderChain (no real AWS involved).
        let store = InMemorySecretStore::new();
        let cfg = ProviderConfig {
            id: "bedrock".to_string(),
            kind: ProviderKind::Bedrock,
            base_url: None,
            api_key_ref: None,
            extra: json!({"region": "eu-west-1"}),
        };

        // Guard the process-global env with fixed test creds for this call.
        std::env::set_var("AWS_ACCESS_KEY_ID", "AKIA_FROM_CHAIN");
        std::env::set_var("AWS_SECRET_ACCESS_KEY", "secret_from_chain");
        let (creds, source) = resolve_credentials(&cfg, &store).unwrap();
        std::env::remove_var("AWS_ACCESS_KEY_ID");
        std::env::remove_var("AWS_SECRET_ACCESS_KEY");

        assert_eq!(
            source,
            CredentialSource::ProviderChain,
            "no SecretRef -> AWS provider chain is selected"
        );
        assert_eq!(creds.access_key_id, "AKIA_FROM_CHAIN");
    }

    #[test]
    fn build_requires_region() {
        let store = InMemorySecretStore::new();
        let cfg = ProviderConfig {
            id: "bedrock".to_string(),
            kind: ProviderKind::Bedrock,
            base_url: None,
            api_key_ref: None,
            extra: Value::Null,
        };
        match build_bedrock(&cfg, &store) {
            Err(ProviderError::Other(msg)) => assert!(msg.contains("region")),
            other => panic!("expected Other(region) error, got {other:?}"),
        }
    }

    #[test]
    fn normalize_anthropic_and_titan_responses() {
        let anth = json!({
            "content": [{"type": "text", "text": "Hello"}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 10, "output_tokens": 3}
        });
        let resp = normalize_response("anthropic.claude-3-5-sonnet-20240620-v1:0", &anth);
        assert_eq!(resp.choices[0].message.content.as_deref(), Some("Hello"));
        assert_eq!(resp.choices[0].finish_reason, Some(FinishReason::Stop));
        assert_eq!(resp.usage.unwrap().total_tokens, 13);

        let titan = json!({
            "results": [{"outputText": "Hi there", "completionReason": "FINISH", "tokenCount": 4}]
        });
        let resp = normalize_response("amazon.titan-text-express-v1", &titan);
        assert_eq!(resp.choices[0].message.content.as_deref(), Some("Hi there"));
        assert_eq!(resp.choices[0].finish_reason, Some(FinishReason::Stop));
    }

    #[test]
    fn base64_decode_round_trips_known_vector() {
        // "Hello" -> "SGVsbG8=" (standard base64).
        assert_eq!(base64_decode("SGVsbG8=").unwrap(), b"Hello");
        // No padding.
        assert_eq!(base64_decode("Zm9vYmE").unwrap(), b"fooba");
    }

    /// Build a single AWS event-stream frame wrapping `payload_json` under the
    /// base64 `bytes` field (the Bedrock `chunk` event shape), computing the
    /// framing lengths our decoder validates.
    fn make_event_frame(inner_json: &str) -> Vec<u8> {
        // base64-encode the inner model JSON.
        fn b64(data: &[u8]) -> String {
            const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
            let mut out = String::new();
            for chunk in data.chunks(3) {
                let b = [
                    chunk[0],
                    *chunk.get(1).unwrap_or(&0),
                    *chunk.get(2).unwrap_or(&0),
                ];
                out.push(T[(b[0] >> 2) as usize] as char);
                out.push(T[(((b[0] & 0x03) << 4) | (b[1] >> 4)) as usize] as char);
                if chunk.len() > 1 {
                    out.push(T[(((b[1] & 0x0f) << 2) | (b[2] >> 6)) as usize] as char);
                } else {
                    out.push('=');
                }
                if chunk.len() > 2 {
                    out.push(T[(b[2] & 0x3f) as usize] as char);
                } else {
                    out.push('=');
                }
            }
            out
        }
        let outer = json!({"bytes": b64(inner_json.as_bytes())}).to_string();
        let payload = outer.as_bytes();
        let headers_len: u32 = 0;
        let total_len: u32 = 12 + headers_len + payload.len() as u32 + 4;
        let mut frame = Vec::new();
        frame.extend_from_slice(&total_len.to_be_bytes());
        frame.extend_from_slice(&headers_len.to_be_bytes());
        frame.extend_from_slice(&0u32.to_be_bytes()); // prelude CRC (unchecked)
        frame.extend_from_slice(payload);
        frame.extend_from_slice(&0u32.to_be_bytes()); // message CRC (unchecked)
        frame
    }

    #[test]
    fn decode_event_stream_and_normalize_anthropic_chunks() {
        let mut buf = Vec::new();
        buf.extend(make_event_frame(
            "{\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"Hel\"}}",
        ));
        buf.extend(make_event_frame(
            "{\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"lo\"}}",
        ));
        buf.extend(make_event_frame(
            "{\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"}}",
        ));

        let payloads = decode_event_stream(&buf).unwrap();
        assert_eq!(payloads.len(), 3);

        let model = "anthropic.claude-3-5-sonnet-20240620-v1:0";
        let deltas: Vec<ChatDelta> = payloads
            .iter()
            .filter_map(|p| normalize_stream_chunk(model, p))
            .collect();
        assert_eq!(deltas[0].content.as_deref(), Some("Hel"));
        assert_eq!(deltas[1].content.as_deref(), Some("lo"));
        assert_eq!(deltas[2].finish_reason, Some(FinishReason::Stop));
    }

    #[test]
    fn normalize_titan_stream_chunk() {
        let payload = json!({"outputText": "Hi", "completionReason": "FINISH"});
        let delta = normalize_stream_chunk("amazon.titan-text-express-v1", &payload).unwrap();
        assert_eq!(delta.content.as_deref(), Some("Hi"));
        assert_eq!(delta.finish_reason, Some(FinishReason::Stop));
    }

    #[test]
    fn normalize_titan_populates_prompt_tokens() {
        // Titan reports prompt tokens at the top level; total must include them.
        let titan = json!({
            "inputTextTokenCount": 7,
            "results": [{"outputText": "Hi", "completionReason": "FINISH", "tokenCount": 4}]
        });
        let resp = normalize_response("amazon.titan-text-express-v1", &titan);
        let usage = resp.usage.unwrap();
        assert_eq!(usage.prompt_tokens, 7);
        assert_eq!(usage.completion_tokens, 4);
        assert_eq!(usage.total_tokens, 11);
    }

    /// The incremental de-framer must yield each frame's delta even when frame
    /// bytes are split ACROSS byte-stream chunks (the whole-body buffering the
    /// old code did would have hidden any incremental-boundary bug).
    #[tokio::test]
    async fn chat_stream_incremental_across_split_chunks() {
        use futures_util::stream;

        let mut wire = Vec::new();
        wire.extend(make_event_frame(
            "{\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"Hel\"}}",
        ));
        wire.extend(make_event_frame(
            "{\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"lo\"}}",
        ));
        wire.extend(make_event_frame(
            "{\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"}}",
        ));

        // Split the wire bytes at an awkward offset that lands MID-frame so the
        // decoder must buffer a partial frame across chunk boundaries.
        let split = 7.min(wire.len());
        let (a, b) = wire.split_at(split);
        let mid = b.len() / 2;
        let (b1, b2) = b.split_at(mid);
        let byte_stream = stream::iter(vec![
            Ok(bytes::Bytes::copy_from_slice(a)),
            Ok(bytes::Bytes::copy_from_slice(b1)),
            Ok(bytes::Bytes::copy_from_slice(b2)),
        ]);

        let model = "anthropic.claude-3-5-sonnet-20240620-v1:0".to_string();
        let deltas: Vec<ChatDelta> = bedrock_event_stream(byte_stream, model)
            .map(|r| r.unwrap())
            .collect()
            .await;

        assert_eq!(deltas.len(), 3);
        assert_eq!(deltas[0].content.as_deref(), Some("Hel"));
        assert_eq!(deltas[1].content.as_deref(), Some("lo"));
        assert_eq!(deltas[2].finish_reason, Some(FinishReason::Stop));
    }

    #[tokio::test]
    async fn chat_signs_and_normalizes_via_mock() {
        use wiremock::matchers::{header_exists, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(
                "/model/anthropic.claude-3-5-sonnet-20240620-v1:0/invoke",
            ))
            .and(header_exists("authorization"))
            .and(header_exists("x-amz-date"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "content": [{"type": "text", "text": "signed ok"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 2, "output_tokens": 2}
            })))
            .mount(&server)
            .await;

        let adapter = BedrockAdapter::new(
            "bedrock",
            server.uri(),
            "us-east-1",
            fixed_creds(),
            CredentialSource::SecretRef,
        );
        let req = ChatRequest::new(
            "anthropic.claude-3-5-sonnet-20240620-v1:0",
            vec![ChatMessage::text(MessageRole::User, "hi")],
        );
        let resp = adapter.chat(req).await.unwrap();
        assert_eq!(
            resp.choices[0].message.content.as_deref(),
            Some("signed ok")
        );
        assert_eq!(resp.choices[0].finish_reason, Some(FinishReason::Stop));
    }
}
