//! Native Ollama adapter (architecture.md Section 4.3, tasks.md Phase 8).
//!
//! Ollama serves the OpenAI Chat Completions format natively at
//! `http://127.0.0.1:11434/v1`, so chat and streaming deliberately reuse the
//! EXACT SAME code path as the OpenAI adapter ([`super::native::NativeAdapter`]);
//! only the `base_url` differs and, by default, no API key is sent.
//!
//! The one net-new piece versus LM Studio is model discovery. Ollama does NOT
//! surface its installed models under the OpenAI `/v1/models` path; instead it
//! exposes a NATIVE discovery endpoint `GET /api/tags` rooted at the SERVER ROOT
//! (`http://127.0.0.1:11434`), NOT under `/v1`. It returns JSON of shape
//! `{"models":[{"name":"llama3.1:8b", ...}, ...]}`. So [`OllamaAdapter`] wraps a
//! [`NativeAdapter`] for chat/chat_stream/capabilities/id and overrides
//! [`ChatProvider::list_models`] to hit `/api/tags` on the tags root derived by
//! stripping a trailing `/v1` (and any trailing slash) off the configured chat
//! `base_url`, so a custom `base_url` is honored.
//!
//! # Manual integration check (NOT run in CI)
//!
//! With Ollama running locally and a model pulled, the adapter can be exercised
//! against the real `http://127.0.0.1:11434/v1` endpoint. This is documented
//! here and encoded as the `#[ignore]`d test
//! [`tests::manual_ollama_localhost_stream`] below; it is intentionally excluded
//! from CI (the sandbox and CI runners have no Ollama server) and is run by hand
//! with `cargo test -- --ignored manual_ollama`.

use std::sync::Arc;

use async_trait::async_trait;
use futures_util::stream::BoxStream;
use serde::Deserialize;

use domain::{ProviderConfig, ProviderKind};
use secrets::SecretStore;

use super::native::{default_openai_caps, AuthStrategy, NativeAdapter, Routing};
use crate::capability::HttpSseClient;
use crate::contract::{
    Capabilities, ChatDelta, ChatProvider, ChatRequest, ChatResponse, ModelInfo, ProviderError,
};
use crate::registry::ProviderFactory;

/// Default Ollama local server root when `ProviderConfig::base_url` is unset.
/// This is the OpenAI-compatible chat root; native discovery lives one level up
/// at the server root's `/api/tags`.
///
/// We target `127.0.0.1` (IPv4 loopback) rather than the `localhost` hostname on
/// purpose. Ollama binds `127.0.0.1:11434` by default, but on some systems
/// `localhost` resolves to `::1` (IPv6) first. In that case the reqwest client
/// dials `[::1]:11434`, gets connection-refused, and enumeration fails silently,
/// while `curl http://localhost:11434/api/tags` still works because curl retries
/// the other address family. Using the IPv4 literal avoids that resolution
/// mismatch. Tradeoff: a user who deliberately runs Ollama on an IPv6-only `::1`
/// bind must set a custom `base_url` (e.g. `http://[::1]:11434/v1`); a
/// user-supplied `base_url` is always honored unchanged.
pub const DEFAULT_BASE_URL: &str = "http://127.0.0.1:11434/v1";

/// Derive the Ollama server root (which hosts `/api/tags`) from the configured
/// OpenAI-compatible chat `base_url`, by stripping any trailing slash then any
/// trailing `/v1`. This keeps a custom `base_url` honored: pointing chat at
/// `http://host:11434/v1` yields the tags root `http://host:11434`.
fn tags_root(base_url: &str) -> String {
    let trimmed = base_url.trim_end_matches('/');
    trimmed.strip_suffix("/v1").unwrap_or(trimmed).to_string()
}

/// The native Ollama adapter. Chat and streaming delegate to the shared
/// [`NativeAdapter`] (built with [`Routing::Standard`] + [`default_openai_caps`])
/// at the `/v1` chat base; model discovery is overridden to call Ollama's native
/// `GET /api/tags` on the server root.
#[derive(Debug)]
pub struct OllamaAdapter {
    inner: NativeAdapter,
    /// A client rooted at the server root (not `/v1`), used only for `/api/tags`.
    tags_client: HttpSseClient,
}

/// The `GET /api/tags` response shape (`{ "models": [ { "name": ... } ] }`).
/// Only the `name` of each installed model is consumed.
#[derive(Debug, Deserialize)]
struct TagsResponse {
    #[serde(default)]
    models: Vec<TagEntry>,
}

#[derive(Debug, Deserialize)]
struct TagEntry {
    name: String,
}

/// Build the Ollama adapter: resolve auth exactly like LM Studio
/// ([`AuthStrategy::None`] when `api_key_ref` is unset, Bearer when set), pick
/// the chat `base_url` from the config or the local default, and root the tags
/// client at the derived server root.
pub fn build_ollama(
    cfg: &ProviderConfig,
    secrets: &dyn SecretStore,
) -> Result<OllamaAdapter, ProviderError> {
    let auth = match &cfg.api_key_ref {
        Some(key_ref) => {
            let key = secrets
                .resolve(key_ref)
                .map_err(|e| ProviderError::Auth(e.to_string()))?;
            AuthStrategy::Bearer(key)
        }
        None => AuthStrategy::None,
    };
    let base_url = cfg
        .base_url
        .clone()
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
    let tags_client = HttpSseClient::new(tags_root(&base_url));
    let inner = NativeAdapter::new(
        cfg.id.clone(),
        base_url,
        auth,
        Routing::Standard,
        default_openai_caps(),
    );
    Ok(OllamaAdapter { inner, tags_client })
}

#[async_trait]
impl ChatProvider for OllamaAdapter {
    fn id(&self) -> &str {
        self.inner.id()
    }

    fn capabilities(&self, model: &str) -> Capabilities {
        self.inner.capabilities(model)
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        // Ollama's native discovery endpoint is `GET /api/tags` on the server
        // root, NOT the OpenAI `/v1/models` path. Reuse the shared client's
        // `get_json` helper against the tags-root client. Send an explicit
        // `Accept: application/json` header so a content-negotiating server or
        // proxy cannot hand back a non-JSON representation.
        let tags: TagsResponse = self
            .tags_client
            .get_json(
                "api/tags",
                &[("Accept".to_string(), "application/json".to_string())],
            )
            .await?;
        Ok(tags
            .models
            .into_iter()
            .map(|m| ModelInfo::new(m.name))
            .collect())
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, ProviderError> {
        self.inner.chat(req).await
    }

    async fn chat_stream(
        &self,
        req: ChatRequest,
    ) -> Result<BoxStream<'static, Result<ChatDelta, ProviderError>>, ProviderError> {
        self.inner.chat_stream(req).await
    }
}

/// [`ProviderFactory`] for [`ProviderKind::Ollama`].
pub struct OllamaFactory;

impl ProviderFactory for OllamaFactory {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Ollama
    }

    fn build(
        &self,
        cfg: &ProviderConfig,
        secrets: &dyn SecretStore,
    ) -> Result<Arc<dyn ChatProvider>, ProviderError> {
        Ok(Arc::new(build_ollama(cfg, secrets)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{ChatMessage, ChatProvider, ChatRequest, MessageRole};
    use futures_util::StreamExt;
    use secrets::InMemorySecretStore;
    use serde_json::Value;

    #[test]
    fn tags_root_strips_v1_and_trailing_slash() {
        assert_eq!(
            tags_root("http://127.0.0.1:11434/v1"),
            "http://127.0.0.1:11434"
        );
        assert_eq!(
            tags_root("http://127.0.0.1:11434/v1/"),
            "http://127.0.0.1:11434"
        );
        assert_eq!(tags_root("http://host:9999"), "http://host:9999");
    }

    /// The keyless local default targets the IPv4 loopback literal `127.0.0.1`,
    /// NOT the `localhost` hostname, so the reqwest client dials the address
    /// family Ollama actually binds and does not fail when `localhost` resolves
    /// to `::1` first. A user-supplied `base_url` remains honored (proven by the
    /// wiremock contract test).
    #[test]
    fn default_base_url_uses_ipv4_loopback() {
        assert_eq!(DEFAULT_BASE_URL, "http://127.0.0.1:11434/v1");
        assert_eq!(tags_root(DEFAULT_BASE_URL), "http://127.0.0.1:11434");
    }

    /// Decode proof (FEAT-004): deserialize the EXACT rich `/api/tags` payload a
    /// real Ollama server returns -- including `model`, `modified_at`, `size`,
    /// `digest`, a nested `details` object, and a `capabilities` array -- into
    /// [`TagsResponse`]. [`TagEntry`] derives `Deserialize` WITHOUT
    /// `deny_unknown_fields`, so serde's default leniency ignores every extra
    /// field and only `name` is consumed. This guards against a regression where
    /// a stricter attribute or a type-mismatched field would make the real
    /// payload fail to decode and the model silently vanish from the picker.
    #[test]
    fn tags_response_decodes_real_rich_payload() {
        let body = r#"{"models":[{"name":"qwen3:8b","model":"qwen3:8b","modified_at":"2026-09-02T00:42:29.5225957-07:00","size":5225388164,"digest":"500a1f067a9f782620b40bee6f7b0c89e17ae61f686b92c24933e4ca4b2b8b41","details":{"parent_model":"","format":"gguf","family":"qwen3","families":["qwen3"],"parameter_size":"8.2B","quantization_level":"Q4_K_M","context_length":40960,"embedding_length":4096},"capabilities":["completion","tools","thinking"]}]}"#;
        let parsed: TagsResponse =
            serde_json::from_str(body).expect("decode rich /api/tags payload");
        assert_eq!(parsed.models.len(), 1);
        assert_eq!(parsed.models[0].name, "qwen3:8b");
    }

    /// Ollama on the default local path sends NO implicit API key: with
    /// `api_key_ref = None`, [`build_ollama`] must select [`AuthStrategy::None`]
    /// so no `Authorization` header goes on the wire (the local path needs no
    /// key by default, like LM Studio).
    #[test]
    fn ollama_sends_no_implicit_api_key_when_ref_is_none() {
        let store = InMemorySecretStore::new();
        let cfg = ProviderConfig {
            id: "ollama-local".to_string(),
            kind: ProviderKind::Ollama,
            base_url: None, // default http://127.0.0.1:11434/v1
            api_key_ref: None,
            extra: Value::Null,
        };
        let adapter = build_ollama(&cfg, &store).expect("build ollama adapter");
        // AuthStrategy::None emits no headers; a Bearer key would add an
        // `Authorization` header. Asserting on the inner adapter's produced
        // headers proves the local path sends no implicit API key.
        assert!(
            adapter.inner.auth_headers_for_test().is_empty(),
            "Ollama must use AuthStrategy::None (no auth header) when api_key_ref is None"
        );
    }

    /// OPTIONAL manual integration check against a REAL Ollama server at
    /// `http://127.0.0.1:11434/v1` (default). Requires Ollama running with a
    /// model pulled. NOT run in CI (no such server there); run by hand with:
    ///
    /// `cargo test -p providers -- --ignored manual_ollama_localhost_stream`
    ///
    /// Asserts that a streamed completion yields at least one delta and the
    /// stream terminates. Uses the real default base_url and no API key.
    #[tokio::test]
    #[ignore = "manual: requires a local Ollama server on 127.0.0.1:11434"]
    async fn manual_ollama_localhost_stream() {
        let store = InMemorySecretStore::new();
        let cfg = ProviderConfig {
            id: "ollama-local".to_string(),
            kind: ProviderKind::Ollama,
            base_url: None, // default http://127.0.0.1:11434/v1
            api_key_ref: None,
            extra: Value::Null,
        };
        let adapter = build_ollama(&cfg, &store).expect("build ollama adapter");

        // Use whatever model the local server currently has pulled; substitute
        // a model id that `ollama list` reports.
        let mut req = ChatRequest::new(
            "llama3.1:8b",
            vec![ChatMessage::text(
                MessageRole::User,
                "Say hello in one word.",
            )],
        );
        req.stream = true;

        let mut stream = adapter.chat_stream(req).await.expect("open stream");
        let mut saw_delta = false;
        while let Some(item) = stream.next().await {
            let delta = item.expect("delta ok");
            if delta.content.is_some() || !delta.tool_calls.is_empty() {
                saw_delta = true;
            }
        }
        assert!(
            saw_delta,
            "expected at least one streamed delta from Ollama"
        );
    }
}
