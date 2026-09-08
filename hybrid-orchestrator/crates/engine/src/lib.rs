//! `engine` crate: Strategy B, the zero-install embedded local inference engine
//! (architecture.md Section 4, embedded-engine mapping).
//!
//! This crate runs local GGUF models in-process via the llama.cpp family, with
//! no separate Ollama or LM Studio install required. It is intentionally
//! SELF-CONTAINED and PROVIDER-AGNOSTIC: it owns a GGUF model registry plus a
//! low-level inference API that yields plain token strings and a crate-local
//! [`StopReason`], and it does NOT depend on the providers crate. The
//! `ChatProvider` implementation that wraps this engine and performs the
//! OpenAI-shaped mapping lives in the providers adapter
//! (`crates/providers/src/adapters/embedded.rs`). Keeping the dependency edge
//! one-directional (providers depends on engine, never the reverse) is what
//! avoids a cyclic package dependency between the two crates.
//!
//! Isolation scheme: the heavy native llama.cpp binding is an OPTIONAL
//! dependency gated behind the cargo feature `llama`, which is DEFAULT OFF.
//! Under default features this crate is a dependency-light STUB whose inference
//! entry point returns a crate-local [`EngineError::NotCompiled`] carrying
//! [`NOT_COMPILED_MESSAGE`], instructing the caller to rebuild with the `llama`
//! feature. That is exactly what the routine per-crate CI build compiles, so it
//! never needs the native library, a C++ toolchain, or network access. Enabling
//! the `llama` feature turns on the real llama.cpp-backed implementation and its
//! heavy dependency, which are verified only by a dedicated gated CI job
//! (network plus a C++ toolchain) and cannot be compiled or run in the offline
//! sandbox.
//!
//! Model management here is intentionally minimal: the user supplies a LOCAL
//! `.gguf` path (import/select) and the engine tracks registered models in an
//! in-memory registry. Downloading and remote catalog browsing are deferred to
//! a later feature because they add a networked catalog client and a download
//! manager that are orthogonal to standing up the in-process inference seam; the
//! local-path import path is enough to run a model end to end.

use std::path::{Path, PathBuf};

use tokio::sync::Mutex;

/// The stable provider id the embedded engine reports (used by the providers
/// adapter's `ChatProvider::id`). Kept as a constant so the adapter, the backend
/// factory, and tests share one source of truth.
pub const EMBEDDED_ENGINE_ID: &str = "embedded";

/// A conservative context window (in tokens) the providers adapter advertises
/// from `ChatProvider::capabilities` when a model does not report its own. Small
/// local GGUF models commonly ship a 4K context; callers use this only for
/// truncation heuristics, not as a hard guarantee.
pub const DEFAULT_MAX_CONTEXT: u32 = 4096;

/// The error message carried by [`EngineError::NotCompiled`] when the crate was
/// built WITHOUT the `llama` feature. Shared so the providers adapter and tests
/// assert on the exact text without duplicating the string.
pub const NOT_COMPILED_MESSAGE: &str =
    "embedded engine not compiled: rebuild with the `llama` feature";

/// Why a completed generation stopped. This is the engine's provider-agnostic
/// stop signal; the providers adapter maps it onto the OpenAI-shaped
/// `FinishReason` (`Stop -> Stop`, `Length -> Length`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// The model emitted an end-of-generation token.
    Stop,
    /// Generation hit the requested (or default) token budget.
    Length,
}

/// Errors surfaced by the engine's inference API. Provider-agnostic on purpose:
/// the providers adapter maps every variant into `ProviderError::Other(..)`.
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    /// The crate was built without the `llama` feature, so no native inference
    /// backend is available. Carries [`NOT_COMPILED_MESSAGE`].
    #[error("{}", NOT_COMPILED_MESSAGE)]
    NotCompiled,
    /// A model id was requested that is not present in the registry.
    #[error("no registered model with id `{0}`")]
    UnknownModel(String),
    /// The native inference backend failed (load/tokenize/decode/etc.). Only
    /// produced under the `llama` feature.
    #[error("{0}")]
    Inference(String),
}

/// A single registered local model: a stable id plus the on-disk path to its
/// `.gguf` file. The id defaults to the file stem when a caller registers by
/// path alone, so listings surface a human-recognizable name.
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

    /// The registered model ids, in registration order. Provider-agnostic: the
    /// providers adapter maps these to the OpenAI-shaped `ModelInfo` when
    /// implementing `list_models`.
    pub fn model_ids(&self) -> Vec<String> {
        self.models.iter().map(|m| m.id.clone()).collect()
    }
}

/// The embedded local inference engine (Strategy B).
///
/// Wraps an [`EngineConfig`] registry behind a [`tokio::sync::Mutex`] taken
/// only for brief registry reads/writes. On the native path a SEPARATE
/// `model_slot` mutex serializes one-model-at-a-time decodes without holding the
/// registry lock across the CPU-bound generation, so listing/import/select stay
/// responsive during a decode. Under default features the inference method is a
/// stub; the real llama.cpp-backed body compiles only under the `llama` feature.
pub struct EmbeddedEngine {
    config: Mutex<EngineConfig>,
    /// Serializes native decodes so only one model runs at a time, WITHOUT
    /// holding the registry (`config`) lock across the CPU-bound decode. This is
    /// a dedicated model-slot lock distinct from the registry lock, so a long
    /// generation no longer blocks listing/import/select/status (which only take
    /// `config` briefly). Only used on the native `llama` path.
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

    /// Snapshot the registered model ids, in registration order. The providers
    /// adapter maps these to the OpenAI-shaped `ModelInfo` for `list_models`.
    pub async fn registered_model_ids(&self) -> Vec<String> {
        let cfg = self.config.lock().await;
        cfg.model_ids()
    }

    /// Snapshot the registered model entries (id plus on-disk `.gguf` path), in
    /// registration order. Used by the backend commands (FEAT-002) to present a
    /// display-safe id+path view of the imported local models.
    pub async fn registered_entries(&self) -> Vec<ModelEntry> {
        let cfg = self.config.lock().await;
        cfg.models.clone()
    }

    /// Run inference for the given registered model id and already-built prompt,
    /// returning the generated token strings plus the terminal [`StopReason`].
    /// This is the engine's PROVIDER-AGNOSTIC inference entry point: the
    /// providers adapter builds the prompt from a `ChatRequest` and maps the
    /// returned tokens/`StopReason` onto the OpenAI-shaped response types.
    ///
    /// Under default features (no `llama`) this is a stub that returns
    /// [`EngineError::NotCompiled`]. Under `#[cfg(feature = "llama")]` it
    /// resolves the model path from the registry, serializes on the dedicated
    /// `model_slot` lock, and drives the CPU-bound native decode via
    /// `tokio::task::spawn_blocking`.
    #[cfg(not(feature = "llama"))]
    pub async fn generate(
        &self,
        _model_id: &str,
        _prompt: &str,
        _max_tokens: u32,
    ) -> Result<(Vec<String>, StopReason), EngineError> {
        Err(EngineError::NotCompiled)
    }

    /// See the default-feature stub above for the contract. Under `llama` this
    /// runs the real native decode.
    #[cfg(feature = "llama")]
    pub async fn generate(
        &self,
        model_id: &str,
        prompt: &str,
        max_tokens: u32,
    ) -> Result<(Vec<String>, StopReason), EngineError> {
        // Resolve the on-disk path under the registry lock, then release it
        // before the (potentially long) decode.
        let path = {
            let cfg = self.config.lock().await;
            cfg.find(model_id)
                .map(|e| e.path.clone())
                .ok_or_else(|| EngineError::UnknownModel(model_id.to_string()))?
        };
        let prompt = prompt.to_string();
        // Serialize decodes on the dedicated model-slot lock (NOT the registry
        // `config` lock), and run the CPU-bound decode on a blocking thread so
        // the async runtime is never blocked and status/list/import stay
        // responsive during generation.
        let _slot = self.model_slot.lock().await;
        tokio::task::spawn_blocking(move || native::generate(&path, &prompt, max_tokens))
            .await
            .map_err(|e| EngineError::Inference(format!("decode task join: {e}")))?
    }
}

impl Default for EmbeddedEngine {
    fn default() -> Self {
        EmbeddedEngine::empty()
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
/// The path loads a GGUF via the binding, runs a greedy decode loop, and returns
/// the generated token strings plus a [`StopReason`] (`Stop` when the model
/// emits an end-of-generation token, `Length` when it hits the token budget).
/// Prompt construction and OpenAI-shaping are deliberately kept OUT of the
/// engine: the providers adapter builds the prompt string and maps the result.
#[cfg(feature = "llama")]
mod native {
    use super::{EngineError, StopReason, DEFAULT_MAX_CONTEXT};

    use llama_cpp_2::context::params::LlamaContextParams;
    use llama_cpp_2::llama_backend::LlamaBackend;
    use llama_cpp_2::llama_batch::LlamaBatch;
    use llama_cpp_2::model::params::LlamaModelParams;
    use llama_cpp_2::model::{AddBos, LlamaModel, Special};
    use llama_cpp_2::sampling::LlamaSampler;

    /// Run inference to completion, returning the generated tokens as text plus
    /// the terminal [`StopReason`]. Loads the model, runs a greedy decode loop up
    /// to the requested (or default) token budget, and unloads on drop.
    ///
    /// This is a synchronous, CPU-bound function and MUST be driven off the
    /// async executor (via `tokio::task::spawn_blocking`) so a long decode never
    /// blocks the runtime's worker threads. It does NOT take the engine's
    /// registry (`config`) lock; the caller serializes concurrent decodes with
    /// the dedicated `model_slot` lock instead, so listing/import/select stay
    /// responsive during a generation. It takes an ALREADY-BUILT prompt string
    /// so it stays provider-agnostic (prompt construction lives in the providers
    /// adapter).
    pub(super) fn generate(
        model_path: &std::path::Path,
        prompt: &str,
        max_tokens: u32,
    ) -> Result<(Vec<String>, StopReason), EngineError> {
        let backend =
            LlamaBackend::init().map_err(|e| EngineError::Inference(format!("llama init: {e}")))?;
        let model_params = LlamaModelParams::default();
        let model = LlamaModel::load_from_file(&backend, model_path, &model_params)
            .map_err(|e| EngineError::Inference(format!("load gguf: {e}")))?;

        let ctx_params = LlamaContextParams::default();
        let mut ctx = model
            .new_context(&backend, ctx_params)
            .map_err(|e| EngineError::Inference(format!("new context: {e}")))?;

        let tokens = model
            .str_to_token(prompt, AddBos::Always)
            .map_err(|e| EngineError::Inference(format!("tokenize: {e}")))?;

        let mut batch = LlamaBatch::new(512, 1);
        let last = tokens.len() as i32 - 1;
        for (i, token) in tokens.into_iter().enumerate() {
            batch
                .add(token, i as i32, &[0], i as i32 == last)
                .map_err(|e| EngineError::Inference(format!("batch add: {e}")))?;
        }
        ctx.decode(&mut batch)
            .map_err(|e| EngineError::Inference(format!("decode: {e}")))?;

        let mut out = Vec::new();
        let mut sampler = LlamaSampler::greedy();
        let mut n_cur = batch.n_tokens();
        let budget = if max_tokens == 0 {
            DEFAULT_MAX_CONTEXT
        } else {
            max_tokens
        };
        let mut finish = StopReason::Length;

        for _ in 0..budget {
            let token = sampler.sample(&ctx, batch.n_tokens() - 1);
            sampler.accept(token);
            if model.is_eog_token(token) {
                finish = StopReason::Stop;
                break;
            }
            let piece = model
                .token_to_str(token, Special::Tokenize)
                .map_err(|e| EngineError::Inference(format!("detokenize: {e}")))?;
            out.push(piece);

            batch.clear();
            batch
                .add(token, n_cur, &[0], true)
                .map_err(|e| EngineError::Inference(format!("batch add: {e}")))?;
            n_cur += 1;
            ctx.decode(&mut batch)
                .map_err(|e| EngineError::Inference(format!("decode: {e}")))?;
        }

        Ok((out, finish))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let ids = cfg.model_ids();
        assert_eq!(ids.len(), 1);
        assert_eq!(ids[0], "phi-3-mini");
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
    async fn engine_register_and_list_model_ids() {
        let engine = EmbeddedEngine::empty();
        let id = engine.register_path("/models/tiny.gguf").await;
        assert_eq!(id, "tiny");
        let ids = engine.registered_model_ids().await;
        assert_eq!(ids.len(), 1);
        assert_eq!(ids[0], "tiny");
    }

    // Under DEFAULT features (no `llama`) the inference entry point is a stub
    // that returns the crate-local `NotCompiled` error carrying the exact
    // `NOT_COMPILED_MESSAGE`. This test runs in routine CI.
    #[cfg(not(feature = "llama"))]
    #[tokio::test]
    async fn stub_generate_returns_not_compiled_error() {
        let engine = EmbeddedEngine::empty();
        let err = engine
            .generate("tiny", "user: hello\nassistant: ", 16)
            .await
            .expect_err("stub generate must error");
        match &err {
            EngineError::NotCompiled => {
                assert_eq!(err.to_string(), NOT_COMPILED_MESSAGE);
                assert!(NOT_COMPILED_MESSAGE.contains("llama"));
            }
            other => panic!("expected EngineError::NotCompiled, got {other:?}"),
        }
    }
}
