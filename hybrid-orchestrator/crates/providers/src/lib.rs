//! `providers` crate: the `ChatProvider` contract, capability negotiation, the
//! shared HTTP/SSE client, the `ProviderRegistry`, and per-provider adapters
//! (architecture.md Section 4).
//!
//! Phase 2 (FEAT-001) implements the foundation (contract P2.1, registry P2.2,
//! capability + shared HTTP/SSE client P2.3). The concrete adapters under
//! [`adapters`] are filled in by FEAT-002/003/004; they remain compiling stubs
//! here.
//!
//! This crate carries crates.io deps (reqwest/futures/aws-sigv4/...), so it is
//! under `[workspace] exclude` in the root Cargo.toml and built/tested in CI via
//! `--manifest-path crates/providers/Cargo.toml` (the offline sandbox cannot
//! resolve those deps). Its tests use mocked HTTP + SSE, never live network.

pub mod capability;
pub mod contract;
pub mod registry;

pub mod adapters {
    //! Provider adapter implementations.
    //!
    //! The native OpenAI-compatible adapters (openai, lmstudio, generic_openai,
    //! azure_openai) are implemented (FEAT-002) and share the single code path
    //! in [`native`]. The translation-shim adapters (anthropic, gemini,
    //! bedrock) remain compiling stubs, filled in by FEAT-003.
    pub mod native;

    pub mod azure_openai;
    pub mod generic_openai;
    pub mod lmstudio;
    pub mod openai;

    // Translation-shim placeholders (FEAT-003): each module compiles but does
    // not yet implement [`crate::contract::ChatProvider`].
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
pub use registry::{ProviderFactory, ProviderRegistry};

// Native OpenAI-compatible adapter factories (FEAT-002). Registering these on a
// `ProviderRegistry` wires up the OpenAI / LM Studio / generic / Azure kinds.
pub use adapters::azure_openai::AzureOpenAiFactory;
pub use adapters::generic_openai::GenericOpenAiFactory;
pub use adapters::lmstudio::LmStudioFactory;
pub use adapters::openai::OpenAiFactory;

// Re-export the reused domain types so downstream crates can refer to them
// through this crate's surface (single source of truth remains `domain`).
pub use domain::{ProviderConfig, ProviderKind};
