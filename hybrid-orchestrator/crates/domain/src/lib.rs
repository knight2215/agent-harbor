//! `domain` crate: the shared, framework-agnostic Section 7.1 domain models.
//!
//! This is the SINGLE source of truth for the domain DTOs (architecture.md
//! Section 7.1, plus ProviderConfig 4.2 and ManualRoute/PrivacyTag/RouteSource
//! 6.1). It is a LEAF crate: it depends only DOWNWARD on `secrets` (the
//! canonical home of [`secrets::SecretRef`], Section 3.3) and on serde/uuid/
//! chrono. Both `orchestrator-core` and `persistence` depend on `domain`, which
//! keeps the crate graph strictly downward (Section 3.3) and removes the
//! `persistence -> orchestrator-core` reverse edge that previously formed a hard
//! `cargo` cyclic-dependency error.
//!
//! `orchestrator-core` re-exports every type here at its crate root, so the
//! existing `orchestrator_core::Conversation` (etc.) paths used by `tauri-app`
//! and the tests keep working unchanged.

pub mod models;

pub use models::{
    AgentPersona, Attachment, Conversation, ManualRoute, McpServerConfig, McpTransport, Message,
    MessageContent, MessageStatus, ModelParameters, PermissionMode, PrivacyTag, ProviderConfig,
    ProviderKind, Role, RouteMetadata, RouteSource, RoutingHint, RoutingMode, SecretRef,
    TokenUsage, ToolCall, ToolResult,
};
