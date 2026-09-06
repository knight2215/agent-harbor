//! `orchestrator-core` crate: domain models and message pipeline.
//!
//! Framework-agnostic (no Tauri dependency). Coordinates routing, providers,
//! MCP, and persistence. Phase 1 provides the Section 7.1 domain models (see
//! [`models`]), the [`session::SessionManager`] (Section 7.5), and the
//! [`events::CoreEvent`] surface (Section 8). The streaming pipeline lands in a
//! later phase.
//!
//! ## Domain models live in the leaf `domain` crate
//!
//! The Section 7.1 DTOs are defined in the leaf [`domain`] crate, NOT here. Both
//! `orchestrator-core` and `persistence` depend DOWNWARD on `domain`
//! (architecture.md Section 3.3), which is what keeps the crate graph acyclic:
//! `persistence` no longer depends back on `orchestrator-core`. This crate
//! re-exports the models (see [`models`] and the crate-root re-exports below),
//! so `orchestrator_core::Conversation` (etc.) keeps resolving for `tauri-app`
//! and the tests.

pub mod events;
pub mod pipeline;
pub mod session;

// Re-export the `domain::models` module as `orchestrator_core::models` so the
// existing `crate::models::...` / `orchestrator_core::models::...` paths keep
// resolving after the DTOs moved into the leaf `domain` crate (the cycle-break
// remediation). `domain` is the single source of truth for these DTOs.
pub use domain::models;

// Re-export the domain models at the crate root so dependents can write
// `orchestrator_core::Conversation` etc. These are the authoritative DTOs
// (defined in the leaf `domain` crate) mirrored by hand into
// `frontend/src/types/index.ts`.
pub use domain::{
    AgentPersona, Attachment, Conversation, ManualRoute, McpServerConfig, McpTransport, Message,
    MessageContent, MessageStatus, ModelParameters, PermissionMode, PrivacyTag, ProviderConfig,
    ProviderKind, Role, RouteMetadata, RouteSource, RoutingHint, SecretRef, TokenUsage, ToolCall,
    ToolResult,
};

// Section 8 event surface and Section 7.5 session manager.
pub use events::{CoreEvent, McpConnectionState};
pub use session::{ConversationInit, SessionError, SessionManager};

#[cfg(test)]
mod tests {
    /// Smoke test that references an item from the library crates
    /// orchestrator-core depends on, proving the dependency edges compile.
    /// `persistence` and `secrets` now expose real APIs (exercised by their own
    /// crates' tests and by the session manager tests here), so they no longer
    /// expose a `placeholder()`. `providers` likewise now exposes real APIs
    /// (the registry + `ChatProvider` contract), so we reference its built-in
    /// registry constructor instead of a placeholder; `mcp-client` and
    /// `routing` are still placeholder crates.
    #[test]
    fn smoke() {
        let registry = providers::builtin_registry();
        assert!(registry.has_factory(providers::ProviderKind::OpenAI));
        mcp_client::placeholder();
        routing::placeholder();
    }
}
