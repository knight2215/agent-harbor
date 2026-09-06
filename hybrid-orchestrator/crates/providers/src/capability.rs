//! Capability negotiation (architecture.md Section 4.4) and the shared HTTP +
//! SSE client used by every native OpenAI-compatible adapter (Section 4.3).
//!
//! Two concerns live here:
//!
//! 1. [`negotiate`]: given a [`ChatRequest`] and a model's [`Capabilities`],
//!    gate features the model cannot do (strip tools, signal a streaming
//!    fallback, drop vision/json_mode) so the request the adapter finally sends
//!    is always something the model can serve.
//!
//! 2. [`HttpSseClient`]: an adapter-agnostic async HTTP client. Adapters supply
//!    a path, headers, a JSON body, and (for streaming) a per-event parse
//!    closure; the client POSTs the body and returns either a decoded JSON
//!    response (non-streaming) or a [`BoxStream`] of parsed SSE `data:` events,
//!    with `[DONE]` terminating the stream.

use futures_util::stream::{BoxStream, StreamExt};
use serde::de::DeserializeOwned;

// `Capabilities` is defined canonically in `contract.rs`; re-export it here so
// callers can `use providers::capability::Capabilities` interchangeably.
pub use crate::contract::Capabilities;
use crate::contract::{ChatRequest, ProviderError};

/// The outcome of [`negotiate`]: the (possibly adjusted) request plus flags
/// describing which features were gated so the caller can react (e.g. fall back
/// to non-streaming `chat` and synthesize a single delta).
#[derive(Debug, Clone, PartialEq)]
pub struct Negotiated {
    /// The request with unsupported features stripped/adjusted.
    pub request: ChatRequest,
    /// True when the caller asked to stream but the model cannot; the caller
    /// should call `chat` and synthesize one delta (Section 4.4).
    pub streaming_fallback: bool,
    /// True when tools were requested but stripped because the model does not
    /// support tool calling.
    pub tools_stripped: bool,
    /// The model's max context window, surfaced for truncation decisions.
    pub max_context: Option<u32>,
}

/// Gate a request against a model's capabilities (architecture.md Section 4.4).
///
/// - If `tools` is unsupported, the tool list is cleared (`tools_stripped`).
/// - If `streaming` is unsupported but the request set `stream = true`, the
///   request's `stream` flag is cleared and `streaming_fallback` is set so the
///   caller falls back to non-streaming `chat`.
/// - If `json_mode` is unsupported, a `response_format` hint in `extra` is
///   removed.
/// - `vision` gating is advisory here (image handling lives in message
///   construction); the flag is honored by not asserting vision support.
/// - `max_context` is surfaced for the session manager's truncation logic.
pub fn negotiate(mut request: ChatRequest, caps: &Capabilities) -> Negotiated {
    let tools_stripped = !caps.tools && !request.tools.is_empty();
    if tools_stripped {
        request.tools.clear();
    }

    let streaming_fallback = request.stream && !caps.streaming;
    if streaming_fallback {
        request.stream = false;
    }

    if !caps.json_mode {
        strip_json_mode(&mut request.extra);
    }

    Negotiated {
        request,
        streaming_fallback,
        tools_stripped,
        max_context: caps.max_context,
    }
}

/// Remove an OpenAI-style `response_format` hint from a request's `extra` blob
/// when the model does not support JSON mode.
fn strip_json_mode(extra: &mut serde_json::Value) {
    if let Some(obj) = extra.as_object_mut() {
        obj.remove("response_format");
    }
}

/// Errors surfaced by [`HttpSseClient`], mapped onto [`ProviderError`] variants.
type ClientResult<T> = Result<T, ProviderError>;

/// An adapter-agnostic HTTP + SSE client (architecture.md Section 4.3). Shared
/// by every native OpenAI-compatible adapter; translation-shim adapters can use
/// it too and translate the JSON at the edges.
#[derive(Debug, Clone)]
pub struct HttpSseClient {
    base_url: String,
    http: reqwest::Client,
}

impl HttpSseClient {
    /// Build a client rooted at `base_url` (no trailing slash required).
    pub fn new(base_url: impl Into<String>) -> Self {
        HttpSseClient {
            base_url: base_url.into(),
            http: reqwest::Client::new(),
        }
    }

    /// Build a client from a pre-configured reqwest client (e.g. with timeouts).
    pub fn with_client(base_url: impl Into<String>, http: reqwest::Client) -> Self {
        HttpSseClient {
            base_url: base_url.into(),
            http,
        }
    }

    /// Join the base URL with a request path, tolerating a leading slash on
    /// `path` and a trailing slash on the base.
    fn url(&self, path: &str) -> String {
        let base = self.base_url.trim_end_matches('/');
        let path = path.trim_start_matches('/');
        format!("{base}/{path}")
    }

    fn apply_headers(
        mut builder: reqwest::RequestBuilder,
        headers: &[(String, String)],
    ) -> reqwest::RequestBuilder {
        for (name, value) in headers {
            builder = builder.header(name, value);
        }
        builder
    }

    /// GET `path` with `headers` and decode the JSON response into `T`. Used by
    /// adapters for endpoints like OpenAI's `GET /models`.
    pub async fn get_json<T: DeserializeOwned>(
        &self,
        path: &str,
        headers: &[(String, String)],
    ) -> ClientResult<T> {
        let builder = self.http.get(self.url(path));
        let builder = Self::apply_headers(builder, headers);

        let resp = builder
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

        resp.json::<T>()
            .await
            .map_err(|e| ProviderError::Decode(e.to_string()))
    }

    /// POST `body` as JSON to `path` with `headers` and decode the JSON response
    /// into `T` (non-streaming path).
    pub async fn post_json<T: DeserializeOwned>(
        &self,
        path: &str,
        headers: &[(String, String)],
        body: &serde_json::Value,
    ) -> ClientResult<T> {
        let builder = self.http.post(self.url(path)).json(body);
        let builder = Self::apply_headers(builder, headers);

        let resp = builder
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

        resp.json::<T>()
            .await
            .map_err(|e| ProviderError::Decode(e.to_string()))
    }

    /// POST `body` as JSON to `path` with `headers` and return a stream of
    /// parsed SSE events (streaming path).
    ///
    /// `parse` is called once per SSE `data:` payload (the JSON between frames);
    /// returning `Ok(None)` skips a frame (e.g. keep-alives). The stream ends
    /// when a `data: [DONE]` sentinel is seen or the body completes.
    pub async fn post_sse<T, F>(
        &self,
        path: &str,
        headers: &[(String, String)],
        body: &serde_json::Value,
        parse: F,
    ) -> ClientResult<BoxStream<'static, ClientResult<T>>>
    where
        T: Send + 'static,
        F: Fn(&str) -> ClientResult<Option<T>> + Send + 'static,
    {
        let builder = self.http.post(self.url(path)).json(body);
        let builder = Self::apply_headers(builder, headers);

        let resp = builder
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

        let byte_stream = resp
            .bytes_stream()
            .map(|chunk| chunk.map_err(|e| ProviderError::Transport(e.to_string())));

        Ok(sse_event_stream(byte_stream, parse))
    }
}

/// Owned state threaded through the SSE `unfold`.
struct SseState<T, F> {
    stream: std::pin::Pin<Box<dyn futures_util::Stream<Item = ClientResult<bytes::Bytes>> + Send>>,
    parse: F,
    buf: String,
    /// Parsed events ready to emit from already-buffered frames.
    pending: std::collections::VecDeque<T>,
    /// Set once `[DONE]` was seen or the byte stream ended; suppresses further
    /// polling of the underlying stream.
    done: bool,
}

/// A reusable SSE decoder over a byte stream. Buffers bytes, splits on the
/// blank-line frame boundary, extracts `data:` payloads, terminates on
/// `[DONE]`, and applies `parse` to each JSON payload. Exposed for adapters and
/// exercised directly by unit tests with a fixture byte buffer.
///
/// Implemented as a manual state machine over `futures_util::stream::unfold`, so
/// it needs no async-generator macro or extra crate.
pub fn sse_event_stream<S, T, F>(byte_stream: S, parse: F) -> BoxStream<'static, ClientResult<T>>
where
    S: futures_util::Stream<Item = ClientResult<bytes::Bytes>> + Send + 'static,
    T: Send + 'static,
    F: Fn(&str) -> ClientResult<Option<T>> + Send + 'static,
{
    let init = SseState {
        stream: Box::pin(byte_stream),
        parse,
        buf: String::new(),
        pending: std::collections::VecDeque::new(),
        done: false,
    };

    futures_util::stream::unfold(init, |mut state| async move {
        loop {
            // Emit any already-parsed events first.
            if let Some(item) = state.pending.pop_front() {
                return Some((Ok(item), state));
            }
            if state.done {
                return None;
            }
            match state.stream.next().await {
                Some(Ok(chunk)) => {
                    state.buf.push_str(&String::from_utf8_lossy(&chunk));
                    match drain_frames(&mut state.buf, &state.parse, &mut state.pending) {
                        Ok(true) => state.done = true,
                        Ok(false) => {}
                        Err(e) => {
                            state.done = true;
                            return Some((Err(e), state));
                        }
                    }
                }
                Some(Err(e)) => {
                    state.done = true;
                    return Some((Err(e), state));
                }
                None => {
                    // Byte stream ended: flush any trailing frame that lacked a
                    // blank-line terminator, then finish.
                    state.done = true;
                    if !state.buf.trim().is_empty() {
                        let leftover = std::mem::take(&mut state.buf);
                        match parse_frame(&leftover, &state.parse, &mut state.pending) {
                            Ok(_) => {}
                            Err(e) => return Some((Err(e), state)),
                        }
                        if let Some(item) = state.pending.pop_front() {
                            return Some((Ok(item), state));
                        }
                    }
                    return None;
                }
            }
        }
    })
    .boxed()
}

/// Split complete `\n\n`-delimited frames out of `buf`, parse each, and push
/// results into `pending`. Returns `Ok(true)` when a `[DONE]` sentinel was seen.
fn drain_frames<T, F>(
    buf: &mut String,
    parse: &F,
    pending: &mut std::collections::VecDeque<T>,
) -> ClientResult<bool>
where
    F: Fn(&str) -> ClientResult<Option<T>>,
{
    let mut saw_done = false;
    // Normalize CRLF to LF so `\r\n\r\n` boundaries split correctly.
    while let Some(pos) = find_frame_boundary(buf) {
        let frame: String = buf.drain(..pos.end).collect();
        let frame = &frame[..pos.frame_len];
        if parse_frame(frame, parse, pending)? {
            saw_done = true;
        }
    }
    Ok(saw_done)
}

/// A located frame boundary: `frame_len` bytes of frame content followed by the
/// blank-line terminator, `end` bytes total to drain from the buffer.
struct FrameBoundary {
    frame_len: usize,
    end: usize,
}

/// Find the EARLIEST `\n\n` or `\r\n\r\n` frame boundary in `buf`. Choosing the
/// earliest (rather than always preferring one style) keeps mixed-newline
/// buffers from merging two adjacent frames into one.
fn find_frame_boundary(buf: &str) -> Option<FrameBoundary> {
    let crlf = buf.find("\r\n\r\n").map(|i| FrameBoundary {
        frame_len: i,
        end: i + 4,
    });
    let lf = buf.find("\n\n").map(|i| FrameBoundary {
        frame_len: i,
        end: i + 2,
    });
    match (crlf, lf) {
        (Some(c), Some(l)) => {
            // Pick whichever terminator starts earlier in the buffer.
            if c.frame_len <= l.frame_len {
                Some(c)
            } else {
                Some(l)
            }
        }
        (Some(c), None) => Some(c),
        (None, l) => l,
    }
}

/// Parse a single SSE frame (which may contain multiple `data:` lines). Returns
/// `Ok(true)` when the frame carried the `[DONE]` sentinel.
fn parse_frame<T, F>(
    frame: &str,
    parse: &F,
    pending: &mut std::collections::VecDeque<T>,
) -> ClientResult<bool>
where
    F: Fn(&str) -> ClientResult<Option<T>>,
{
    // Concatenate all `data:` lines of the frame (SSE allows multi-line data).
    let mut data = String::new();
    for line in frame.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(rest) = line.strip_prefix("data:") {
            let rest = rest.strip_prefix(' ').unwrap_or(rest);
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(rest);
        }
        // Comment lines (`:`), `event:`, `id:`, etc. are ignored.
    }

    if data.is_empty() {
        return Ok(false);
    }
    if data.trim() == "[DONE]" {
        return Ok(true);
    }

    if let Some(item) = parse(data.trim())? {
        pending.push_back(item);
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{ChatDelta, ChatMessage, ChatRequest, MessageRole, ToolSpec};
    use futures_util::stream;
    use serde_json::json;

    fn caps(streaming: bool, tools: bool, json_mode: bool) -> Capabilities {
        Capabilities {
            streaming,
            tools,
            vision: false,
            json_mode,
            max_context: Some(8192),
        }
    }

    fn req_with_tools_stream() -> ChatRequest {
        let mut r = ChatRequest::new("m", vec![ChatMessage::text(MessageRole::User, "hi")]);
        r.tools = vec![ToolSpec::function("f", "d", json!({"type": "object"}))];
        r.stream = true;
        r.extra = json!({"response_format": {"type": "json_object"}});
        r
    }

    #[test]
    fn negotiate_strips_tools_when_unsupported() {
        let n = negotiate(req_with_tools_stream(), &caps(true, false, true));
        assert!(n.tools_stripped);
        assert!(n.request.tools.is_empty());
        assert!(!n.streaming_fallback);
    }

    #[test]
    fn negotiate_signals_streaming_fallback() {
        let n = negotiate(req_with_tools_stream(), &caps(false, true, true));
        assert!(n.streaming_fallback);
        assert!(!n.request.stream);
        assert!(!n.tools_stripped);
    }

    #[test]
    fn negotiate_strips_json_mode_when_unsupported() {
        let n = negotiate(req_with_tools_stream(), &caps(true, true, false));
        // response_format removed from extra.
        assert!(n.request.extra.get("response_format").is_none());
        assert_eq!(n.max_context, Some(8192));
    }

    #[test]
    fn negotiate_passes_through_when_supported() {
        let n = negotiate(req_with_tools_stream(), &caps(true, true, true));
        assert!(!n.tools_stripped);
        assert!(!n.streaming_fallback);
        assert!(n.request.stream);
        assert_eq!(n.request.tools.len(), 1);
        assert!(n.request.extra.get("response_format").is_some());
    }

    /// Feed a fixture byte buffer of `data: {...}\n\n` frames including `[DONE]`
    /// and assert the parsed deltas. No network involved.
    #[tokio::test]
    async fn sse_parses_data_frames_and_terminates_on_done() {
        let fixture = concat!(
            "data: {\"content\":\"Hel\"}\n\n",
            "data: {\"content\":\"lo\"}\n\n",
            ": keep-alive comment\n\n",
            "data: {\"content\":\" world\",\"finish_reason\":\"stop\"}\n\n",
            "data: [DONE]\n\n",
            "data: {\"content\":\"AFTER-DONE-IGNORED\"}\n\n",
        );
        // Deliver the fixture in two arbitrary chunks to exercise buffering
        // across a frame boundary.
        let (a, b) = fixture.split_at(30);
        let byte_stream = stream::iter(vec![
            Ok(bytes::Bytes::from(a.to_string())),
            Ok(bytes::Bytes::from(b.to_string())),
        ]);

        let parse = |data: &str| -> ClientResult<Option<ChatDelta>> {
            serde_json::from_str::<ChatDelta>(data)
                .map(Some)
                .map_err(|e| ProviderError::Decode(e.to_string()))
        };

        let deltas: Vec<ChatDelta> = sse_event_stream(byte_stream, parse)
            .map(|r| r.unwrap())
            .collect()
            .await;

        assert_eq!(
            deltas.len(),
            3,
            "must stop at [DONE], ignoring trailing data"
        );
        assert_eq!(deltas[0].content.as_deref(), Some("Hel"));
        assert_eq!(deltas[1].content.as_deref(), Some("lo"));
        assert_eq!(deltas[2].content.as_deref(), Some(" world"));
        assert_eq!(
            deltas[2].finish_reason,
            Some(crate::contract::FinishReason::Stop)
        );
    }

    #[tokio::test]
    async fn sse_handles_crlf_frame_boundaries() {
        let fixture = "data: {\"content\":\"a\"}\r\n\r\ndata: [DONE]\r\n\r\n";
        let byte_stream = stream::iter(vec![Ok(bytes::Bytes::from(fixture.to_string()))]);
        let parse = |data: &str| -> ClientResult<Option<ChatDelta>> {
            serde_json::from_str::<ChatDelta>(data)
                .map(Some)
                .map_err(|e| ProviderError::Decode(e.to_string()))
        };
        let deltas: Vec<ChatDelta> = sse_event_stream(byte_stream, parse)
            .map(|r| r.unwrap())
            .collect()
            .await;
        assert_eq!(deltas.len(), 1);
        assert_eq!(deltas[0].content.as_deref(), Some("a"));
    }
}
