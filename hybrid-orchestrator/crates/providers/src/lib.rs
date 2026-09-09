//! `providers` crate: the `ChatProvider` contract, capability negotiation, the
//! shared HTTP/SSE client, the `ProviderRegistry`, and per-provider adapters
//! (architecture.md Section 4).
//!
//! Phase 2 (FEAT-001) implements the foundation (contract P2.1, registry P2.2,
//! capability + shared HTTP/SSE client P2.3). FEAT-002 adds the native
//! OpenAI-compatible adapters (P2.4-P2.6); FEAT-003 adds the Anthropic, Gemini,
//! and Bedrock translation shims (P2.7-P2.9) under [`adapters`].
//!
//! This crate carries crates.io deps (reqwest/futures/aws-sigv4/...), so it is
//! under `[workspace] exclude` in the root Cargo.toml and built/tested in CI via
//! `--manifest-path crates/providers/Cargo.toml` (the offline sandbox cannot
//! resolve those deps). Its tests use mocked HTTP + SSE, never live network.

pub mod builtins;
pub mod capability;
pub mod contract;
pub mod crypto;
pub mod registry;

pub mod adapters {
    //! Provider adapter implementations.
    //!
    //! The native OpenAI-compatible adapters (openai, lmstudio, generic_openai,
    //! azure_openai) are implemented (FEAT-002) and share the single code path
    //! in [`native`]. The translation-shim adapters (anthropic, gemini,
    //! bedrock) are implemented (FEAT-003): each presents the internal
    //! OpenAI-compatible contract while translating to its native wire format.
    pub mod native;

    pub mod azure_openai;
    pub mod embedded;
    pub mod generic_openai;
    pub mod lmstudio;
    pub mod ollama;
    pub mod openai;

    // Translation shims (FEAT-003): each presents [`crate::contract::ChatProvider`]
    // while translating to a native wire format (Anthropic Messages API, Gemini
    // generateContent, Bedrock SigV4 + per-model bodies).
    pub mod anthropic;
    pub mod bedrock;
    pub mod gemini;
}

// Ergonomic re-exports of the key public types (used by tauri-app and the rest
// of the core).
pub use capability::{negotiate, HttpSseClient, Negotiated};
pub use contract::{
    Capabilities, ChatChoice, ChatDelta, ChatMessage, ChatProvider, ChatRequest, ChatResponse,
    FinishReason, FunctionCall, FunctionSpec, MessageRole, ModelInfo, ProviderError, ToolCall,
    ToolCallDelta, ToolSpec, Usage,
};
// The process-wide rustls crypto-provider installer. Re-exported so callers
// (and integration tests) can install the `ring` default deterministically
// before constructing any rustls-backed client, independent of which adapter
// path builds the first client.
pub use crypto::ensure_crypto_provider;
pub use registry::{ProviderFactory, ProviderRegistry};

// Built-in registry wiring + the model-selector data source (FEAT-004 / P2.10):
// `builtin_registry` registers all seven factories, `build_registry`
// instantiates from persisted config, and `list_available_models` returns
// provider/model/capabilities/price (Section 4.5 / 6.1 / 8.2).
pub use builtins::{
    build_registry, builtin_registry, list_available_models, AvailableModel, AvailableModelsResult,
    PricingTable, ProviderEnumerationError, TokenPrice,
};

// Native OpenAI-compatible adapter factories (FEAT-002). Registering these on a
// `ProviderRegistry` wires up the OpenAI / LM Studio / generic / Azure kinds.
pub use adapters::azure_openai::AzureOpenAiFactory;
pub use adapters::generic_openai::GenericOpenAiFactory;
pub use adapters::lmstudio::LmStudioFactory;
pub use adapters::ollama::OllamaFactory;
pub use adapters::openai::OpenAiFactory;

// The embedded local inference engine factory (Strategy B / FEAT-002).
// Registering this on a `ProviderRegistry` wires up the in-process
// `ProviderKind::Embedded` engine.
pub use adapters::embedded::{EmbeddedFactory, EmbeddedProvider};

// Translation-shim adapter factories (FEAT-003). Registering these on a
// `ProviderRegistry` wires up the Anthropic / Gemini / Bedrock kinds.
pub use adapters::anthropic::AnthropicFactory;
pub use adapters::bedrock::BedrockFactory;
pub use adapters::gemini::GeminiFactory;

// Re-export the reused domain types so downstream crates can refer to them
// through this crate's surface (single source of truth remains `domain`).
pub use domain::{ProviderConfig, ProviderKind};
