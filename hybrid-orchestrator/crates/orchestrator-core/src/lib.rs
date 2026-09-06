//! `orchestrator-core` crate: domain models and message pipeline.
//!
//! Framework-agnostic (no Tauri dependency). Coordinates routing, providers,
//! MCP, and persistence. Phase 1 provides the Section 7.1 domain models (see
//! [`models`]), the [`session::SessionManager`] (Section 7.5), and the
//! [`events::CoreEvent`] surface (Section 8). The streaming pipeline lands in a
//! later phase.

pub mod events;
pub mod models;
pub mod pipeline;
pub mod session;

// Re-export the domain models at the crate root so dependents can write
// `orchestrator_core::Conversation` etc. These are the authoritative DTOs
// mirrored by hand into `frontend/src/types/index.ts`.
pub use models::{
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
    /// Smoke test that references an item from the still-placeholder library
    /// crates orchestrator-core depends on, proving the dependency edges
    /// compile. `persistence` and `secrets` now expose real APIs (exercised by
    /// their own crates' tests and by the session manager tests here), so they
    /// no longer expose a `placeholder()`.
    #[test]
    fn smoke() {
        providers::placeholder();
        mcp_client::placeholder();
        routing::placeholder();
    }
}
