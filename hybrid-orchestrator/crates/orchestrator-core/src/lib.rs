//! `orchestrator-core` crate: domain models and message pipeline.
//!
//! Framework-agnostic (no Tauri dependency). Coordinates routing, providers,
//! MCP, and persistence. Phase 1 introduces the real Section 7.1 domain models
//! (see [`models`]); the session manager, pipeline, and events land in later
//! phases.

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

#[cfg(test)]
mod tests {
    /// Smoke test that references an item from each of the five library crates
    /// orchestrator-core depends on, proving the dependency edges compile.
    #[test]
    fn smoke() {
        providers::placeholder();
        mcp_client::placeholder();
        routing::placeholder();
        persistence::placeholder();
        secrets::placeholder();
    }
}
