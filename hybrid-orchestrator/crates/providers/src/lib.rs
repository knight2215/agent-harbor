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
pub mod net_share;
pub mod registry;
pub mod web_search;

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
    build_registry, builtin_registry, list_available_models,
    list_available_models_with_build_errors, AvailableModel, AvailableModelsResult, PricingTable,
    ProviderEnumerationError, TokenPrice,
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

// Pluggable web search (FEAT-004): the `WebSearchProvider` trait, the selectable
// `WebSearchKind` enum (Tavily default), the working Tavily adapter, and the
// display-safe result/error/options types. Consumed by the tauri-app web-search
// commands and unit-tested offline with wiremock (no live network).
pub use web_search::{
    TavilyProvider, WebSearchError, WebSearchKind, WebSearchOptions, WebSearchProvider,
    WebSearchResult, TAVILY_DEFAULT_BASE_URL,
};

// LAN model sharing (FEAT-006): the SHARE/serve seam (`ModelShareServer` +
// `render_models_response` + `ShareServerStatus`) that re-exposes this
// instance's local models to peers over an OpenAI-compatible read surface, and
// the peer-DISCOVERY seam (`PeerDiscovery` trait + `StubPeerDiscovery` +
// `DiscoveredPeer`). The consume side needs no code here: a peer is a generic
// OpenAI-compatible `ProviderConfig` row that enumerates + routes through the
// existing path. Unit-tested offline; live LAN binding + discovery are user-only.
pub use net_share::{
    render_models_response, DiscoveredPeer, ModelShareServer, PeerDiscovery, ShareServerStatus,
    SharedModel, StubPeerDiscovery,
};

// Re-export the reused domain types so downstream crates can refer to them
// through this crate's surface (single source of truth remains `domain`).
pub use domain::{ProviderConfig, ProviderKind};
