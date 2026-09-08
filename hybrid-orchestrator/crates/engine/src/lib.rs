//! `engine` crate: Strategy B, the zero-install embedded local inference engine
//! (architecture.md Section 4, embedded-engine mapping).
//!
//! This crate runs local GGUF models in-process via the llama.cpp family, with
//! no separate Ollama or LM Studio install required. It implements
//! [`providers::ChatProvider`] and emits OpenAI-shaped [`providers::ChatDelta`]s
//! exactly like the HTTP adapters, so the routing engine, the pipeline, and the
//! UI treat it uniformly with every other provider.
//!
//! Isolation scheme: the heavy native llama.cpp binding is an OPTIONAL
//! dependency gated behind the cargo feature `llama`, which is DEFAULT OFF.
//! Under default features this crate is a dependency-light STUB whose `chat` and
//! `chat_stream` return a clear [`providers::ProviderError`] instructing the
//! caller to rebuild with the `llama` feature. That is exactly what the routine
//! per-crate CI build compiles, so it never needs the native library, a C++
//! toolchain, or network access. Enabling the `llama` feature turns on the real
//! llama.cpp-backed implementation and its heavy dependency, which are verified
//! only by a dedicated gated CI job (network plus a C++ toolchain) and cannot be
//! compiled or run in the offline sandbox.
//!
//! Model management here is intentionally minimal: the user supplies a LOCAL
//! `.gguf` path (import/select) and the engine tracks registered models in an
//! in-memory registry that feeds [`providers::ChatProvider::list_models`].
//! Downloading and remote catalog browsing are deferred to a later feature
//! because they add a networked catalog client and a download manager that are
//! orthogonal to standing up the in-process inference seam; the local-path
//! import path is enough to run a model end to end.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use futures_util::stream::BoxStream;
use tokio::sync::Mutex;

use providers::{
    Capabilities, ChatDelta, ChatProvider, ChatRequest, ChatResponse, ModelInfo, ProviderError,
};

/// The stable provider id the embedded engine reports from
/// [`ChatProvider::id`]. Kept as a constant so the backend factory (FEAT-002)
/// and tests share one source of truth.
pub const EMBEDDED_ENGINE_ID: &str = "embedded";

/// A conservative context window (in tokens) advertised by
/// [`ChatProvider::capabilities`] when a model does not report its own. Small
/// local GGUF models commonly ship a 4K context; callers use this only for
/// truncation heuristics, not as a hard guarantee.
pub const DEFAULT_MAX_CONTEXT: u32 = 4096;

/// The error message returned by the stub `chat`/`chat_stream` when the crate
/// was built WITHOUT the `llama` feature. Shared so tests assert on the exact
/// text without duplicating the string.
pub const NOT_COMPILED_MESSAGE: &str =
    "embedded engine not compiled: rebuild with the `llama` feature";

/// A single registered local model: a stable id plus the on-disk path to its
/// `.gguf` file. The id defaults to the file stem when a caller registers by
/// path alone, so `list_models` surfaces a human-recognizable name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelEntry {
    /// Provider-scoped model id (defaults to the `.gguf` file stem).
    pub id: String,
    /// Absolute or relative on-disk path to the `.gguf` file.
    pub path: PathBuf,
}

impl ModelEntry {
    /// Build an entry from a `.gguf` path, deriving the id from the file stem.
    /// Falls back to the full file name (and then to `"model"`) when a stem
    /// cannot be extracted, so the id is never empty.
    pub fn from_path(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string())
            .or_else(|| {
                path.file_name()
                    .and_then(|s| s.to_str())
                    .map(|s| s.to_string())
            })
            .unwrap_or_else(|| "model".to_string());
        ModelEntry { id, path }
    }

    /// Build an entry with an explicit id and `.gguf` path.
    pub fn with_id(id: impl Into<String>, path: impl Into<PathBuf>) -> Self {
        ModelEntry {
            id: id.into(),
            path: path.into(),
        }
    }
}

/// Configuration for the embedded engine: the per-OS directory where local
/// GGUF models are expected to live, plus the set of registered model entries.
///
/// The registry logic is deliberately free of any native-library dependency so
/// it is fully unit-testable under default features. Registering a model does
/// NOT touch the filesystem or load anything; loading happens lazily on the
/// native path when a request selects a model.
#[derive(Debug, Clone, Default)]
pub struct EngineConfig {
    /// The directory local `.gguf` files are resolved against when a caller
    /// registers a bare (relative) file name. `None` means paths are used
    /// as-given.
    pub models_dir: Option<PathBuf>,
    /// Registered models, in registration order.
    pub models: Vec<ModelEntry>,
}

impl EngineConfig {
    /// An empty configuration with no models directory and no registered models.
    pub fn new() -> Self {
        EngineConfig::default()
    }

    /// Set the base directory that relative `.gguf` paths resolve against.
    pub fn with_models_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.models_dir = Some(dir.into());
        self
    }

    /// Resolve a caller-supplied path against `models_dir`. An absolute path is
    /// returned unchanged; a relative path is joined onto `models_dir` when one
    /// is configured, and returned as-given otherwise.
    pub fn resolve_path(&self, path: &Path) -> PathBuf {
        if path.is_absolute() {
            return path.to_path_buf();
        }
        match &self.models_dir {
            Some(dir) => dir.join(path),
            None => path.to_path_buf(),
        }
    }

    /// Register (or update) a model by `.gguf` path, deriving its id from the
    /// file stem. The path is resolved against `models_dir` first. If an entry
    /// with the same id already exists it is replaced (re-registration wins).
    /// Returns the id of the registered model.
    pub fn register_path(&mut self, path: impl AsRef<Path>) -> String {
        let resolved = self.resolve_path(path.as_ref());
        let entry = ModelEntry::from_path(resolved);
        let id = entry.id.clone();
        self.insert(entry);
        id
    }

    /// Register (or update) a model with an explicit id and `.gguf` path. The
    /// path is resolved against `models_dir` first.
    pub fn register_with_id(&mut self, id: impl Into<String>, path: impl AsRef<Path>) {
        let resolved = self.resolve_path(path.as_ref());
        self.insert(ModelEntry::with_id(id, resolved));
    }

    /// Insert an entry, replacing any existing entry with the same id.
    fn insert(&mut self, entry: ModelEntry) {
        if let Some(existing) = self.models.iter_mut().find(|m| m.id == entry.id) {
            *existing = entry;
        } else {
            self.models.push(entry);
        }
    }

    /// Look up a registered model entry by id.
    pub fn find(&self, id: &str) -> Option<&ModelEntry> {
        self.models.iter().find(|m| m.id == id)
    }

    /// The registered models as OpenAI-shaped [`ModelInfo`] (id only; the
    /// display name and context length are left unset because a `.gguf` header
    /// is read only on the native load path).
    pub fn model_infos(&self) -> Vec<ModelInfo> {
        self.models
            .iter()
            .map(|m| ModelInfo::new(m.id.clone()))
            .collect()
    }
}

/// The embedded local inference engine (Strategy B).
///
/// Wraps an [`EngineConfig`] registry behind a [`tokio::sync::Mutex`] taken
/// only for brief registry reads/writes. On the native path a SEPARATE
/// `model_slot` mutex serializes one-model-at-a-time decodes without holding the
/// registry lock across the CPU-bound generation, so `list_models`/import/select
/// stay responsive during a decode. Under default features the inference methods
/// are stubs; the real llama.cpp-backed bodies compile only under the `llama`
/// feature.
pub struct EmbeddedEngine {
    config: Mutex<EngineConfig>,
    /// Serializes native decodes so only one model runs at a time, WITHOUT
    /// holding the registry (`config`) lock across the CPU-bound decode. This is
    /// a dedicated model-slot lock distinct from the registry lock, so a long
    /// generation no longer blocks `list_models`/import/select/status (which
    /// only take `config` briefly). Only used on the native `llama` path.
    #[cfg(feature = "llama")]
    model_slot: Mutex<()>,
}

impl EmbeddedEngine {
    /// Build an engine with the given configuration.
    pub fn new(config: EngineConfig) -> Self {
        EmbeddedEngine {
            config: Mutex::new(config),
            #[cfg(feature = "llama")]
            model_slot: Mutex::new(()),
        }
    }

    /// Build an engine with an empty configuration (no registered models).
    pub fn empty() -> Self {
        EmbeddedEngine::new(EngineConfig::new())
    }

    /// Register (or update) a model by `.gguf` path. Returns the registered id.
    pub async fn register_path(&self, path: impl AsRef<Path>) -> String {
        let mut cfg = self.config.lock().await;
        cfg.register_path(path)
    }

    /// Register (or update) a model with an explicit id and `.gguf` path.
    pub async fn register_with_id(&self, id: impl Into<String>, path: impl AsRef<Path>) {
        let mut cfg = self.config.lock().await;
        cfg.register_with_id(id, path);
    }

    /// Snapshot the registered models as [`ModelInfo`] values.
    pub async fn registered_models(&self) -> Vec<ModelInfo> {
        let cfg = self.config.lock().await;
        cfg.model_infos()
    }

    /// Snapshot the registered model entries (id plus on-disk `.gguf` path), in
    /// registration order. Used by the backend commands (FEAT-002) to present a
    /// display-safe id+path view of the imported local models.
    pub async fn registered_entries(&self) -> Vec<ModelEntry> {
        let cfg = self.config.lock().await;
        cfg.models.clone()
    }
}

impl Default for EmbeddedEngine {
    fn default() -> Self {
        EmbeddedEngine::empty()
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
        max_context: Some(DEFAULT_MAX_CONTEXT),
    }
}

#[async_trait]
impl ChatProvider for EmbeddedEngine {
    fn id(&self) -> &str {
        EMBEDDED_ENGINE_ID
    }

    fn capabilities(&self, _model: &str) -> Capabilities {
        embedded_caps()
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        Ok(self.registered_models().await)
    }

    #[cfg(not(feature = "llama"))]
    async fn chat(&self, _req: ChatRequest) -> Result<ChatResponse, ProviderError> {
        Err(ProviderError::Other(NOT_COMPILED_MESSAGE.to_string()))
    }

    #[cfg(not(feature = "llama"))]
    async fn chat_stream(
        &self,
        _req: ChatRequest,
    ) -> Result<BoxStream<'static, Result<ChatDelta, ProviderError>>, ProviderError> {
        Err(ProviderError::Other(NOT_COMPILED_MESSAGE.to_string()))
    }

    #[cfg(feature = "llama")]
    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, ProviderError> {
        native::chat(self, req).await
    }

    #[cfg(feature = "llama")]
    async fn chat_stream(
        &self,
        req: ChatRequest,
    ) -> Result<BoxStream<'static, Result<ChatDelta, ProviderError>>, ProviderError> {
        native::chat_stream(self, req).await
    }
}

/// The native llama.cpp-backed inference path.
///
/// This module compiles ONLY under the `llama` feature and pulls in the heavy
/// `llama-cpp-2` binding, which builds a large C++ library and therefore needs a
/// C/C++ toolchain plus network access. It cannot be compiled, linked, or run in
/// the offline sandbox and is proven only by a dedicated gated CI job. Under
/// default features this whole module is absent, so the routine per-crate build
/// never touches the native library.
///
/// The path loads a GGUF via the binding, runs inference, maps each generated
/// token to an OpenAI-shaped `ChatDelta { content: Some(token), .. }`, and emits
/// a terminal delta carrying `finish_reason: Some(FinishReason::Stop)` (or
/// `Length` when generation stops on the token budget). One model is loaded at a
/// time behind the engine's `tokio::sync::Mutex`; loading a different model
/// unloads the previous one and frees its memory.
#[cfg(feature = "llama")]
mod native {
    use super::{EmbeddedEngine, DEFAULT_MAX_CONTEXT};
    use futures_util::stream::{self, BoxStream};
    use providers::{
        ChatChoice, ChatDelta, ChatMessage, ChatRequest, ChatResponse, FinishReason, MessageRole,
        ProviderError,
    };

    use llama_cpp_2::context::params::LlamaContextParams;
    use llama_cpp_2::llama_backend::LlamaBackend;
    use llama_cpp_2::llama_batch::LlamaBatch;
    use llama_cpp_2::model::params::LlamaModelParams;
    use llama_cpp_2::model::{AddBos, LlamaModel, Special};
    use llama_cpp_2::sampling::LlamaSampler;

    /// Flatten the request messages into a single prompt. Local GGUF chat
    /// templating varies by model; a simple role-prefixed concatenation keeps
    /// this path dependency-free of any per-model template and is sufficient for
    /// the in-process seam. A model-specific chat template can be layered on
    /// later without changing the `ChatProvider` surface.
    fn build_prompt(req: &ChatRequest) -> String {
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

    /// Resolve the on-disk `.gguf` path for the requested model id from the
    /// engine's registry.
    async fn resolve_model_path(
        engine: &EmbeddedEngine,
        model_id: &str,
    ) -> Result<std::path::PathBuf, ProviderError> {
        let cfg = engine.config.lock().await;
        cfg.find(model_id).map(|e| e.path.clone()).ok_or_else(|| {
            ProviderError::Other(format!("no registered model with id `{model_id}`"))
        })
    }

    /// Run inference to completion, returning the generated tokens as text plus
    /// the terminal finish reason. Loads the model, runs a greedy decode loop up
    /// to the requested (or default) token budget, and unloads on drop.
    ///
    /// This is a synchronous, CPU-bound function and MUST be driven off the
    /// async executor (via `tokio::task::spawn_blocking`) so a long decode never
    /// blocks the runtime's worker threads. It does NOT take the engine's
    /// registry (`config`) lock; the caller serializes concurrent decodes with
    /// the dedicated `model_slot` lock instead, so `list_models`/import/select
    /// stay responsive during a generation.
    fn generate(
        model_path: &std::path::Path,
        prompt: &str,
        max_tokens: u32,
    ) -> Result<(Vec<String>, FinishReason), ProviderError> {
        let backend =
            LlamaBackend::init().map_err(|e| ProviderError::Other(format!("llama init: {e}")))?;
        let model_params = LlamaModelParams::default();
        let model = LlamaModel::load_from_file(&backend, model_path, &model_params)
            .map_err(|e| ProviderError::Other(format!("load gguf: {e}")))?;

        let ctx_params = LlamaContextParams::default();
        let mut ctx = model
            .new_context(&backend, ctx_params)
            .map_err(|e| ProviderError::Other(format!("new context: {e}")))?;

        let tokens = model
            .str_to_token(prompt, AddBos::Always)
            .map_err(|e| ProviderError::Other(format!("tokenize: {e}")))?;

        let mut batch = LlamaBatch::new(512, 1);
        let last = tokens.len() as i32 - 1;
        for (i, token) in tokens.into_iter().enumerate() {
            batch
                .add(token, i as i32, &[0], i as i32 == last)
                .map_err(|e| ProviderError::Other(format!("batch add: {e}")))?;
        }
        ctx.decode(&mut batch)
            .map_err(|e| ProviderError::Other(format!("decode: {e}")))?;

        let mut out = Vec::new();
        let mut sampler = LlamaSampler::greedy();
        let mut n_cur = batch.n_tokens();
        let budget = if max_tokens == 0 {
            DEFAULT_MAX_CONTEXT
        } else {
            max_tokens
        };
        let mut finish = FinishReason::Length;

        for _ in 0..budget {
            let token = sampler.sample(&ctx, batch.n_tokens() - 1);
            sampler.accept(token);
            if model.is_eog_token(token) {
                finish = FinishReason::Stop;
                break;
            }
            let piece = model
                .token_to_str(token, Special::Tokenize)
                .map_err(|e| ProviderError::Other(format!("detokenize: {e}")))?;
            out.push(piece);

            batch.clear();
            batch
                .add(token, n_cur, &[0], true)
                .map_err(|e| ProviderError::Other(format!("batch add: {e}")))?;
            n_cur += 1;
            ctx.decode(&mut batch)
                .map_err(|e| ProviderError::Other(format!("decode: {e}")))?;
        }

        Ok((out, finish))
    }

    /// Non-streaming completion: aggregate the generated tokens into a single
    /// [`ChatResponse`] choice with the terminal finish reason.
    pub(super) async fn chat(
        engine: &EmbeddedEngine,
        req: ChatRequest,
    ) -> Result<ChatResponse, ProviderError> {
        let path = resolve_model_path(engine, &req.model).await?;
        let prompt = build_prompt(&req);
        let max_tokens = req.max_tokens.unwrap_or(0);
        let model = req.model.clone();
        // Serialize decodes on the dedicated model-slot lock (NOT the registry
        // `config` lock), and run the CPU-bound decode on a blocking thread so
        // the async runtime is never blocked and status/list/import stay
        // responsive during generation.
        let _slot = engine.model_slot.lock().await;
        let (tokens, finish) =
            tokio::task::spawn_blocking(move || generate(&path, &prompt, max_tokens))
                .await
                .map_err(|e| ProviderError::Other(format!("decode task join: {e}")))??;
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

    /// Streaming completion: map each generated token to a
    /// `ChatDelta { content: Some(token), .. }` and append a terminal delta
    /// carrying the finish reason. The whole generation runs before the stream
    /// is handed back (a bounded local decode), then replayed as deltas.
    pub(super) async fn chat_stream(
        engine: &EmbeddedEngine,
        req: ChatRequest,
    ) -> Result<BoxStream<'static, Result<ChatDelta, ProviderError>>, ProviderError> {
        let path = resolve_model_path(engine, &req.model).await?;
        let prompt = build_prompt(&req);
        let max_tokens = req.max_tokens.unwrap_or(0);
        // Serialize decodes on the dedicated model-slot lock (NOT the registry
        // `config` lock), and run the CPU-bound decode on a blocking thread. The
        // slot is released as soon as generation finishes, before the collected
        // deltas are replayed.
        let (tokens, finish) = {
            let _slot = engine.model_slot.lock().await;
            tokio::task::spawn_blocking(move || generate(&path, &prompt, max_tokens))
                .await
                .map_err(|e| ProviderError::Other(format!("decode task join: {e}")))??
        };

        let mut deltas: Vec<Result<ChatDelta, ProviderError>> = tokens
            .into_iter()
            .map(|t| {
                Ok(ChatDelta {
                    content: Some(t),
                    tool_calls: Vec::new(),
                    finish_reason: None,
                })
            })
            .collect();
        deltas.push(Ok(ChatDelta {
            content: None,
            tool_calls: Vec::new(),
            finish_reason: Some(finish),
        }));

        Ok(Box::pin(stream::iter(deltas)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;
    use providers::{ChatMessage, ChatRequest, MessageRole};

    fn sample_request(model: &str) -> ChatRequest {
        ChatRequest::new(model, vec![ChatMessage::text(MessageRole::User, "hello")])
    }

    #[test]
    fn id_is_stable() {
        let engine = EmbeddedEngine::empty();
        assert_eq!(engine.id(), "embedded");
        assert_eq!(engine.id(), EMBEDDED_ENGINE_ID);
    }

    #[test]
    fn capabilities_report_streaming_text_only() {
        let engine = EmbeddedEngine::empty();
        let caps = engine.capabilities("any-model");
        assert!(caps.streaming);
        assert!(!caps.tools);
        assert!(!caps.vision);
        assert!(!caps.json_mode);
        assert_eq!(caps.max_context, Some(DEFAULT_MAX_CONTEXT));
    }

    #[test]
    fn model_entry_derives_id_from_file_stem() {
        let entry = ModelEntry::from_path("/models/llama3.1-8b-instruct.gguf");
        assert_eq!(entry.id, "llama3.1-8b-instruct");
        assert_eq!(
            entry.path,
            std::path::PathBuf::from("/models/llama3.1-8b-instruct.gguf")
        );
    }

    #[test]
    fn model_entry_falls_back_when_no_stem() {
        // A path with no usable stem still yields a non-empty id.
        let entry = ModelEntry::from_path("");
        assert!(!entry.id.is_empty());
    }

    #[test]
    fn config_resolves_relative_paths_against_models_dir() {
        let cfg = EngineConfig::new().with_models_dir("/data/models");
        let resolved = cfg.resolve_path(Path::new("small.gguf"));
        assert_eq!(resolved, PathBuf::from("/data/models/small.gguf"));
    }

    #[test]
    fn config_leaves_absolute_paths_unchanged() {
        let cfg = EngineConfig::new().with_models_dir("/data/models");
        let resolved = cfg.resolve_path(Path::new("/elsewhere/big.gguf"));
        assert_eq!(resolved, PathBuf::from("/elsewhere/big.gguf"));
    }

    #[test]
    fn register_and_list_by_path() {
        let mut cfg = EngineConfig::new();
        let id = cfg.register_path("/models/phi-3-mini.gguf");
        assert_eq!(id, "phi-3-mini");
        let infos = cfg.model_infos();
        assert_eq!(infos.len(), 1);
        assert_eq!(infos[0].id, "phi-3-mini");
        assert!(cfg.find("phi-3-mini").is_some());
    }

    #[test]
    fn register_with_explicit_id_and_select() {
        let mut cfg = EngineConfig::new();
        cfg.register_with_id("my-model", "/models/whatever.gguf");
        let entry = cfg.find("my-model").expect("registered");
        assert_eq!(entry.path, PathBuf::from("/models/whatever.gguf"));
    }

    #[test]
    fn re_registering_same_id_replaces_entry() {
        let mut cfg = EngineConfig::new();
        cfg.register_with_id("m", "/models/a.gguf");
        cfg.register_with_id("m", "/models/b.gguf");
        assert_eq!(cfg.models.len(), 1);
        assert_eq!(cfg.find("m").unwrap().path, PathBuf::from("/models/b.gguf"));
    }

    #[tokio::test]
    async fn engine_register_and_list_models() {
        let engine = EmbeddedEngine::empty();
        let id = engine.register_path("/models/tiny.gguf").await;
        assert_eq!(id, "tiny");
        let models = engine.list_models().await.expect("list ok");
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "tiny");
    }

    // Under DEFAULT features (no `llama`) the inference methods are stubs that
    // return the clear "not compiled" error. These tests run in routine CI.
    #[cfg(not(feature = "llama"))]
    #[tokio::test]
    async fn stub_chat_returns_not_compiled_error() {
        let engine = EmbeddedEngine::empty();
        let err = engine
            .chat(sample_request("tiny"))
            .await
            .expect_err("stub chat must error");
        match err {
            ProviderError::Other(msg) => {
                assert_eq!(msg, NOT_COMPILED_MESSAGE);
                assert!(msg.contains("llama"));
            }
            other => panic!("expected ProviderError::Other, got {other:?}"),
        }
    }

    #[cfg(not(feature = "llama"))]
    #[tokio::test]
    async fn stub_chat_stream_returns_not_compiled_error() {
        let engine = EmbeddedEngine::empty();
        let err = engine
            .chat_stream(sample_request("tiny"))
            .await
            .err()
            .expect("stub chat_stream must error");
        match err {
            ProviderError::Other(msg) => assert_eq!(msg, NOT_COMPILED_MESSAGE),
            other => panic!("expected ProviderError::Other, got {other:?}"),
        }
    }

    // Keep `StreamExt`/`sample_request` referenced under default features so the
    // imports never go unused (they are used by the native manual test too).
    #[cfg(not(feature = "llama"))]
    #[tokio::test]
    async fn sample_request_shape_is_usable() {
        let req = sample_request("m");
        assert_eq!(req.model, "m");
        // Exercise the StreamExt import against an empty stream.
        let mut s = futures_util::stream::iter(Vec::<u8>::new());
        assert!(StreamExt::next(&mut s).await.is_none());
    }

    /// OPTIONAL manual integration check against a REAL small `.gguf` model.
    /// Requires the crate built with `--features llama` (a C++ toolchain) and a
    /// local model file. NOT run in routine CI. Run by hand with:
    ///
    /// `cargo test -p engine --features llama -- --ignored manual_embedded`
    ///
    /// Set `ENGINE_TEST_GGUF` to the path of a small `.gguf` model; the test
    /// registers it, streams a short completion, and asserts at least one
    /// content delta arrives followed by a terminal finish reason.
    #[cfg(feature = "llama")]
    #[tokio::test]
    #[ignore = "manual: requires --features llama and a local .gguf (set ENGINE_TEST_GGUF)"]
    async fn manual_embedded_stream() {
        let path =
            std::env::var("ENGINE_TEST_GGUF").expect("set ENGINE_TEST_GGUF to a local .gguf path");
        let engine = EmbeddedEngine::empty();
        let id = engine.register_path(&path).await;

        let mut req = sample_request(&id);
        req.stream = true;
        req.max_tokens = Some(16);

        let mut stream = engine.chat_stream(req).await.expect("open stream");
        let mut saw_content = false;
        let mut saw_finish = false;
        while let Some(item) = stream.next().await {
            let delta = item.expect("delta ok");
            if delta.content.is_some() {
                saw_content = true;
            }
            if delta.finish_reason.is_some() {
                saw_finish = true;
            }
        }
        assert!(saw_content, "expected at least one content delta");
        assert!(saw_finish, "expected a terminal finish_reason delta");
    }
}
