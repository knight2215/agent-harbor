//! Embedded local inference engine adapter (architecture.md Section 4,
//! Strategy B / FEAT-002).
//!
//! This adapter surfaces the `engine` crate's [`engine::EmbeddedEngine`] behind
//! the [`ProviderKind::Embedded`] factory, so the in-process llama.cpp-backed
//! engine is registered and routed exactly like the HTTP adapters. The engine
//! crate is provider-agnostic (it exposes a GGUF model registry plus a low-level
//! inference API returning plain token strings and a crate-local
//! [`engine::StopReason`]); the OpenAI shaping (`ChatRequest` -> prompt,
//! tokens/`StopReason` -> `ChatResponse`/`ChatDelta`/[`FinishReason`]) lives
//! HERE, in the adapter. Keeping the `ChatProvider` impl in the providers crate
//! means the only dependency edge between the two crates is one-directional
//! (providers -> engine): there is no cyclic package dependency.
//!
//! This mirrors how [`super::native::NativeAdapter`] wraps the lower-level
//! `HttpSseClient`: the adapter is a thin wrapper around a lower-level piece
//! that does the contract-level shaping.
//!
//! The engine needs NO API key (it runs locally), so [`EmbeddedFactory::build`]
//! never touches the secret store. The engine's models directory is resolved
//! from the [`ProviderConfig`]: a `base_url` value, when present, is treated as
//! the on-disk directory that bare relative `.gguf` file names resolve against;
//! when absent the engine uses paths as-given. Registered GGUF entries are
//! managed at runtime through the Tauri embedded-model commands (FEAT-002), not
//! from static config.
//!
//! The native inference path lives entirely behind the engine crate's `llama`
//! feature (default OFF), forwarded from this crate's `llama` passthrough. Under
//! default features the engine's inference entry point is a stub that returns a
//! clear "not compiled" error, which this adapter maps to
//! [`ProviderError::Other`]; the routine providers build needs no native library.

use std::sync::Arc;

use async_trait::async_trait;
use domain::{ProviderConfig, ProviderKind};
use futures_util::stream::BoxStream;
use secrets::SecretStore;

use crate::contract::{
    Capabilities, ChatProvider, ChatRequest, ChatResponse, ModelInfo, ProviderError,
};
use crate::registry::ProviderFactory;

use engine::{EmbeddedEngine, EngineConfig};

/// Build an [`EmbeddedEngine`] from a [`ProviderConfig`]. A `base_url`, when
/// present, seeds the engine's models directory (relative `.gguf` paths resolve
/// against it); otherwise the engine resolves paths as-given. No API key is
/// consulted: the embedded engine runs in-process and is always local.
pub fn build_embedded(cfg: &ProviderConfig) -> EmbeddedEngine {
    let config = match cfg.base_url.as_deref() {
        Some(dir) if !dir.is_empty() => EngineConfig::new().with_models_dir(dir),
        _ => EngineConfig::new(),
    };
    EmbeddedEngine::new(config)
}

/// The [`ChatProvider`] wrapper around [`engine::EmbeddedEngine`]. It does the
/// OpenAI-shaped contract work (capabilities, model listing, prompt building,
/// and mapping the engine's tokens/[`engine::StopReason`] to
/// `ChatResponse`/`ChatDelta`) while delegating the actual inference to the
/// provider-agnostic engine crate.
///
/// The wrapped engine is held behind an [`Arc`] so a single shared engine can
/// be surfaced through the registry (the Tauri layer registers the SHARED
/// `AppState.embedded_engine` here so models imported at runtime stay visible
/// through the registry). All trait methods take `&self` on the engine, so
/// going through the `Arc` is behaviourally identical to holding it by value.
pub struct EmbeddedProvider {
    engine: Arc<EmbeddedEngine>,
}

impl EmbeddedProvider {
    /// Wrap an already-built [`EmbeddedEngine`] by value (the common factory
    /// path). Delegates to [`EmbeddedProvider::from_shared`].
    pub fn new(engine: EmbeddedEngine) -> Self {
        Self::from_shared(Arc::new(engine))
    }

    /// Wrap an already-shared [`EmbeddedEngine`]. Use this when the same engine
    /// handle must be observable elsewhere (e.g. runtime model imports mutate
    /// the shared engine and those models must remain visible through the
    /// registry instance built here).
    pub fn from_shared(engine: Arc<EmbeddedEngine>) -> Self {
        EmbeddedProvider { engine }
    }
}

/// The capabilities the embedded engine advertises for any model: streaming
/// text only. A local GGUF realistically supports streamed text generation but
/// not tool calling, vision, or a structured JSON output mode, so those are all
/// reported as unsupported and the pipeline gates them off.
fn embedded_caps() -> Capabilities {
    Capabilities {
        streaming: true,
        tools: false,
        vision: false,
        json_mode: false,
        max_context: Some(engine::DEFAULT_MAX_CONTEXT),
    }
}

/// Flatten the request messages into a single prompt. Local GGUF chat templating
/// varies by model; a simple role-prefixed concatenation keeps this path free of
/// any per-model template and is sufficient for the in-process seam. A
/// model-specific chat template can be layered on later without changing the
/// `ChatProvider` surface. Kept here (not in the engine) so the engine's
/// inference API stays provider-agnostic and takes an already-built prompt.
#[cfg(feature = "llama")]
fn build_prompt(req: &ChatRequest) -> String {
    use crate::contract::MessageRole;
    let mut prompt = String::new();
    for msg in &req.messages {
        let role = match msg.role {
            MessageRole::System => "system",
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
            MessageRole::Tool => "tool",
        };
        if let Some(content) = &msg.content {
            prompt.push_str(role);
            prompt.push_str(": ");
            prompt.push_str(content);
            prompt.push('\n');
        }
    }
    prompt.push_str("assistant: ");
    prompt
}

#[async_trait]
impl ChatProvider for EmbeddedProvider {
    fn id(&self) -> &str {
        engine::EMBEDDED_ENGINE_ID
    }

    fn capabilities(&self, _model: &str) -> Capabilities {
        embedded_caps()
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        Ok(self
            .engine
            .registered_model_ids()
            .await
            .into_iter()
            .map(ModelInfo::new)
            .collect())
    }

    #[cfg(not(feature = "llama"))]
    async fn chat(&self, _req: ChatRequest) -> Result<ChatResponse, ProviderError> {
        Err(ProviderError::Other(
            engine::NOT_COMPILED_MESSAGE.to_string(),
        ))
    }

    #[cfg(not(feature = "llama"))]
    async fn chat_stream(
        &self,
        _req: ChatRequest,
    ) -> Result<BoxStream<'static, Result<crate::contract::ChatDelta, ProviderError>>, ProviderError>
    {
        Err(ProviderError::Other(
            engine::NOT_COMPILED_MESSAGE.to_string(),
        ))
    }

    #[cfg(feature = "llama")]
    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, ProviderError> {
        use crate::contract::{ChatChoice, ChatMessage, FinishReason, MessageRole};

        let prompt = build_prompt(&req);
        let max_tokens = req.max_tokens.unwrap_or(0);
        let model = req.model.clone();
        let (tokens, stop) = self
            .engine
            .generate(&req.model, &prompt, max_tokens)
            .await
            .map_err(|e| ProviderError::Other(e.to_string()))?;
        let finish = map_stop(stop);
        let text = tokens.concat();
        Ok(ChatResponse {
            choices: vec![ChatChoice {
                index: 0,
                message: ChatMessage::text(MessageRole::Assistant, text),
                finish_reason: Some(finish),
            }],
            usage: None,
            model: Some(model),
        })
    }

    #[cfg(feature = "llama")]
    async fn chat_stream(
        &self,
        req: ChatRequest,
    ) -> Result<BoxStream<'static, Result<crate::contract::ChatDelta, ProviderError>>, ProviderError>
    {
        use crate::contract::ChatDelta;
        use futures_util::stream;

        let prompt = build_prompt(&req);
        let max_tokens = req.max_tokens.unwrap_or(0);
        let (tokens, stop) = self
            .engine
            .generate(&req.model, &prompt, max_tokens)
            .await
            .map_err(|e| ProviderError::Other(e.to_string()))?;
        let finish = map_stop(stop);

        let mut deltas: Vec<Result<ChatDelta, ProviderError>> = tokens
            .into_iter()
            .map(|t| {
                Ok(ChatDelta {
                    content: Some(t),
                    thinking: None,
                    tool_calls: Vec::new(),
                    finish_reason: None,
                })
            })
            .collect();
        deltas.push(Ok(ChatDelta {
            content: None,
            thinking: None,
            tool_calls: Vec::new(),
            finish_reason: Some(finish),
        }));

        Ok(Box::pin(stream::iter(deltas)))
    }
}

/// Map the engine's provider-agnostic [`engine::StopReason`] to the OpenAI-shaped
/// [`FinishReason`].
#[cfg(feature = "llama")]
fn map_stop(stop: engine::StopReason) -> crate::contract::FinishReason {
    match stop {
        engine::StopReason::Stop => crate::contract::FinishReason::Stop,
        engine::StopReason::Length => crate::contract::FinishReason::Length,
    }
}

/// [`ProviderFactory`] for [`ProviderKind::Embedded`].
pub struct EmbeddedFactory;

impl ProviderFactory for EmbeddedFactory {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Embedded
    }

    fn build(
        &self,
        cfg: &ProviderConfig,
        _secrets: &dyn SecretStore,
    ) -> Result<Arc<dyn ChatProvider>, ProviderError> {
        Ok(Arc::new(EmbeddedProvider::new(build_embedded(cfg))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use secrets::InMemorySecretStore;
    use serde_json::Value;

    fn config(base_url: Option<&str>) -> ProviderConfig {
        ProviderConfig {
            id: "embedded-local".to_string(),
            kind: ProviderKind::Embedded,
            base_url: base_url.map(str::to_string),
            api_key_ref: None,
            extra: Value::Null,
        }
    }

    #[test]
    fn factory_reports_embedded_kind() {
        assert_eq!(EmbeddedFactory.kind(), ProviderKind::Embedded);
    }

    #[test]
    fn builds_without_secret_store_or_api_key() {
        // The embedded engine is always local and needs no key: build must
        // succeed with an empty store and no `api_key_ref`.
        let store = InMemorySecretStore::new();
        let instance = EmbeddedFactory
            .build(&config(None), &store)
            .expect("build embedded engine");
        assert_eq!(instance.id(), "embedded");
        // Streaming text only: tools/vision/json are unsupported.
        let caps = instance.capabilities("any-model");
        assert!(caps.streaming);
        assert!(!caps.tools);
        assert!(!caps.vision);
        assert!(!caps.json_mode);
        assert_eq!(caps.max_context, Some(engine::DEFAULT_MAX_CONTEXT));
    }

    #[test]
    fn builds_with_models_dir_from_base_url() {
        let store = InMemorySecretStore::new();
        let instance = EmbeddedFactory
            .build(&config(Some("/data/models")), &store)
            .expect("build embedded engine");
        assert_eq!(instance.id(), "embedded");
    }

    #[tokio::test]
    async fn provider_lists_registered_models() {
        let provider = EmbeddedProvider::new(build_embedded(&config(None)));
        provider.engine.register_path("/models/tiny.gguf").await;
        let models = provider.list_models().await.expect("list ok");
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "tiny");
    }

    #[tokio::test]
    async fn from_shared_reflects_models_registered_on_the_shared_handle() {
        // A provider built via `from_shared` wraps the SAME engine handle, so a
        // model registered on that shared `Arc` afterwards is enumerated through
        // the provider. This mirrors the Tauri seam where the registry instance
        // must stay in sync with runtime imports on the shared engine.
        let shared = Arc::new(build_embedded(&config(None)));
        let provider = EmbeddedProvider::from_shared(Arc::clone(&shared));
        shared.register_path("/models/tiny.gguf").await;
        let models = provider.list_models().await.expect("list ok");
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "tiny");
    }

    // Under DEFAULT features (no `llama`) the engine's inference entry point is a
    // stub, so the adapter's `chat`/`chat_stream` surface the "not compiled"
    // error as `ProviderError::Other`. These tests run in routine CI.
    #[cfg(not(feature = "llama"))]
    #[tokio::test]
    async fn stub_chat_returns_not_compiled_error() {
        use crate::contract::{ChatMessage, ChatRequest, MessageRole};
        let provider = EmbeddedProvider::new(EmbeddedEngine::empty());
        let req = ChatRequest::new("tiny", vec![ChatMessage::text(MessageRole::User, "hello")]);
        let err = provider.chat(req).await.expect_err("stub chat must error");
        match err {
            ProviderError::Other(msg) => {
                assert_eq!(msg, engine::NOT_COMPILED_MESSAGE);
                assert!(msg.contains("llama"));
            }
            other => panic!("expected ProviderError::Other, got {other:?}"),
        }
    }

    #[cfg(not(feature = "llama"))]
    #[tokio::test]
    async fn stub_chat_stream_returns_not_compiled_error() {
        use crate::contract::{ChatMessage, ChatRequest, MessageRole};
        let provider = EmbeddedProvider::new(EmbeddedEngine::empty());
        let req = ChatRequest::new("tiny", vec![ChatMessage::text(MessageRole::User, "hello")]);
        let err = provider
            .chat_stream(req)
            .await
            .err()
            .expect("stub chat_stream must error");
        match err {
            ProviderError::Other(msg) => assert_eq!(msg, engine::NOT_COMPILED_MESSAGE),
            other => panic!("expected ProviderError::Other, got {other:?}"),
        }
    }
}
