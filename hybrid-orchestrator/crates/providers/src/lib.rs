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
    //! Placeholders for FEAT-002/003/004: each module compiles but does not yet
    //! implement [`crate::contract::ChatProvider`]. They are wired in as those
    //! features land.
    pub mod anthropic;
    pub mod azure_openai;
    pub mod bedrock;
    pub mod gemini;
    pub mod generic_openai;
    pub mod lmstudio;
    pub mod openai;
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

// Re-export the reused domain types so downstream crates can refer to them
// through this crate's surface (single source of truth remains `domain`).
pub use domain::{ProviderConfig, ProviderKind};
