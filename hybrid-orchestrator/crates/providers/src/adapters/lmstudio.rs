//! Native LM Studio adapter (architecture.md Section 4.3, tasks.md P2.5).
//!
//! LM Studio serves the OpenAI Chat Completions format natively, so local
//! inference deliberately reuses the EXACT SAME code path as the OpenAI adapter
//! ([`super::native::NativeAdapter`]); only the `base_url` differs and, by
//! default, no API key is sent. This keeps the local path simple and reliable.
//!
//! # Manual integration check (NOT run in CI)
//!
//! With LM Studio running locally and a model loaded, the adapter can be
//! exercised against the real `http://localhost:1234/v1` endpoint. This is
//! documented here and encoded as the `#[ignore]`d test
//! [`tests::manual_lmstudio_localhost_stream`] below; it is intentionally
//! excluded from CI (the sandbox and CI runners have no LM Studio server) and
//! is run by hand with `cargo test -- --ignored manual_lmstudio`.

use std::sync::Arc;

use domain::{ProviderConfig, ProviderKind};
use secrets::SecretStore;

use super::native::{default_openai_caps, AuthStrategy, NativeAdapter, Routing};
use crate::contract::{ChatProvider, ProviderError};
use crate::registry::ProviderFactory;

/// Default LM Studio local server root when `ProviderConfig::base_url` is unset.
pub const DEFAULT_BASE_URL: &str = "http://localhost:1234/v1";

/// Build the shared native adapter configured for LM Studio: the config's
/// `base_url` (or the local default) and Bearer auth ONLY when an
/// `api_key_ref` is set (LM Studio needs no key by default, so [`AuthStrategy::None`]
/// is used and no `Authorization` header is sent).
pub fn build_lmstudio(
    cfg: &ProviderConfig,
    secrets: &dyn SecretStore,
) -> Result<NativeAdapter, ProviderError> {
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
    Ok(NativeAdapter::new(
        cfg.id.clone(),
        base_url,
        auth,
        Routing::Standard,
        default_openai_caps(),
    ))
}

/// [`ProviderFactory`] for [`ProviderKind::LmStudio`].
pub struct LmStudioFactory;

impl ProviderFactory for LmStudioFactory {
    fn kind(&self) -> ProviderKind {
        ProviderKind::LmStudio
    }

    fn build(
        &self,
        cfg: &ProviderConfig,
        secrets: &dyn SecretStore,
    ) -> Result<Arc<dyn ChatProvider>, ProviderError> {
        Ok(Arc::new(build_lmstudio(cfg, secrets)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{ChatMessage, ChatProvider, ChatRequest, MessageRole};
    use futures_util::StreamExt;
    use secrets::InMemorySecretStore;
    use serde_json::Value;

    /// LM Studio on the default local path sends NO implicit API key: with
    /// `api_key_ref = None`, [`build_lmstudio`] must select [`AuthStrategy::None`]
    /// so no `Authorization` header goes on the wire (architecture.md Section
    /// 9.3 / 4.3, the local path needs no key by default).
    #[test]
    fn lmstudio_sends_no_implicit_api_key_when_ref_is_none() {
        let store = InMemorySecretStore::new();
        let cfg = ProviderConfig {
            id: "lmstudio-local".to_string(),
            kind: ProviderKind::LmStudio,
            base_url: None, // default http://localhost:1234/v1
            api_key_ref: None,
            extra: Value::Null,
        };
        let adapter = build_lmstudio(&cfg, &store).expect("build lmstudio adapter");
        // AuthStrategy::None emits no headers; a Bearer key would add an
        // `Authorization` header. Asserting on the produced headers proves the
        // local path sends no implicit API key.
        assert!(
            adapter.auth_headers_for_test().is_empty(),
            "LM Studio must use AuthStrategy::None (no auth header) when api_key_ref is None"
        );
    }

    /// OPTIONAL manual integration check against a REAL LM Studio server at
    /// `http://localhost:1234/v1` (default). Requires LM Studio running with a
    /// model loaded. NOT run in CI (no such server there); run by hand with:
    ///
    /// ```text
    /// cargo test -p providers -- --ignored manual_lmstudio_localhost_stream
    /// ```
    ///
    /// Asserts that a streamed completion yields at least one delta and the
    /// stream terminates. Uses the real default base_url and no API key.
    #[tokio::test]
    #[ignore = "manual: requires a local LM Studio server on localhost:1234"]
    async fn manual_lmstudio_localhost_stream() {
        let store = InMemorySecretStore::new();
        let cfg = ProviderConfig {
            id: "lmstudio-local".to_string(),
            kind: ProviderKind::LmStudio,
            base_url: None, // default http://localhost:1234/v1
            api_key_ref: None,
            extra: Value::Null,
        };
        let adapter = build_lmstudio(&cfg, &store).expect("build lmstudio adapter");

        // Use whatever model the local server currently has loaded; LM Studio
        // accepts the loaded model id or a placeholder.
        let mut req = ChatRequest::new(
            "local-model",
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
            "expected at least one streamed delta from LM Studio"
        );
    }
}
