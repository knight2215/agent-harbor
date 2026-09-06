//! Managed application state.
//!
//! Holds the Arc-wrapped subsystems the command handlers borrow across the IPC
//! boundary: the [`SessionManager`] (Section 7.5, over the persistence `Db`)
//! and the [`SecretStore`] (Section 9.1). Registered with the Tauri builder via
//! `.manage(...)` in `main.rs`.

use std::sync::Arc;

use mcp_client::McpServerHandle;
use orchestrator_core::{CoreEvent, PermissionRegistry, SessionManager};
use persistence::config::AppConfig;
use persistence::{Db, PersistenceError};
use routing::PolicyRegistry;
use secrets::{KeyringSecretStore, SecretStore};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

/// Application-wide state managed by Tauri and shared across command handlers.
///
/// Subsystems are behind `Arc` so the state is cheap to clone into async
/// command tasks and is `Send + Sync` for Tauri's managed-state requirements.
/// The [`SecretStore`] is boxed behind a trait object so the concrete backend
/// (keyring in production, in-memory in tests) can vary without touching the
/// command surface.
#[derive(Clone)]
pub struct AppState {
    /// The Section 7.5 session orchestration seam over the persistence pool.
    pub session_manager: Arc<SessionManager>,
    /// The keystore. Only [`SecretStore::store`]/`delete`/`rotate` are reachable
    /// from commands; `resolve` (the plaintext seam) is never wired to IPC.
    pub secret_store: Arc<dyn SecretStore + Send + Sync>,
    /// Sender the core uses to publish [`CoreEvent`]s toward the frontend. The
    /// receiver half is drained by the event bridge (`events.rs`) and forwarded
    /// over the Tauri channel. Phase 1 wires the plumbing; the streaming
    /// pipeline (Phase 2) starts pushing events onto it.
    pub core_events: UnboundedSender<CoreEvent>,
    /// Registry of in-flight `Ask`-mode tool-permission requests (Section 5.6 /
    /// 9.4). The core's `PermissionGate` registers a pending request when it
    /// emits a [`CoreEvent::PermissionRequested`]; the `resolve_permission`
    /// command delivers the user's decision here to unblock the awaiting
    /// invocation. Phase 3 (FEAT-002) wires this seam; Phase 4 (FEAT-002) wires
    /// the pipeline that constructs the `PermissionGate` around it in
    /// `send_message`.
    pub permission_registry: PermissionRegistry,
    /// The routing policy registry (architecture.md Section 6.4). Holds the
    /// selectable/persisted active automatic policy; the `send_message`
    /// pipeline calls [`PolicyRegistry::resolve`] to route each turn. The active
    /// id is read from `AppConfig.active_routing_policy` at
    /// [`AppState::initialize`], defaulting to the built-in `autoDefault`.
    /// Behind `Arc` so it is cheaply cloned into the spawned pipeline task.
    pub policies: Arc<PolicyRegistry>,
    /// Connected MCP server handles the pipeline attaches tools from
    /// (architecture.md Section 5). EMPTY for now: the MCP server-manager wiring
    /// that connects and tracks handles lands in Phase 5, so the pipeline runs
    /// with zero connected servers (no tools attached) until then. The pipeline
    /// tolerates an empty set (it simply skips tool attachment).
    pub mcp_servers: Arc<Vec<Arc<McpServerHandle>>>,
}

impl AppState {
    /// Initialize the application state: open the SQLite database at `db_path`,
    /// run migrations, build the [`SessionManager`], and construct the
    /// keyring-backed [`SecretStore`].
    ///
    /// A fixed/app-data path is acceptable for the Phase 1 dev panel; the real
    /// app-data resolution (per-OS config dir) is refined in a later phase.
    pub async fn initialize(
        db_path: impl AsRef<std::path::Path>,
    ) -> Result<(Self, UnboundedReceiver<CoreEvent>), PersistenceError> {
        let db = Db::open(db_path).await?;

        // Read the persisted active routing policy id (Section 6.4 / 10.4) and
        // select it, falling back to the built-in `autoDefault` if it is unset
        // or names a policy this build does not register.
        let app_config = AppConfig::load(&db).await?;
        let mut policies = PolicyRegistry::new();
        if let Some(active) = app_config.active_routing_policy.as_deref() {
            // A stale/unknown persisted id must not fail startup: keep the
            // default active policy in that case.
            let _ = policies.set_active(active);
        }

        // The provider registry + candidate model list are (re)built per turn in
        // the `send_message` command path from the persisted `ProviderConfig`
        // rows and pricing (mirroring `list_available_models_inner`), so nothing
        // provider-related is built here.

        let session_manager = Arc::new(SessionManager::new(db));
        let secret_store: Arc<dyn SecretStore + Send + Sync> = Arc::new(KeyringSecretStore::new());
        let (core_events, rx) = tokio::sync::mpsc::unbounded_channel();
        let state = AppState {
            session_manager,
            secret_store,
            core_events,
            permission_registry: PermissionRegistry::new(),
            policies: Arc::new(policies),
            // No MCP server manager yet (Phase 5); the pipeline tolerates an
            // empty set by attaching no tools.
            mcp_servers: Arc::new(Vec::new()),
        };
        Ok((state, rx))
    }

    /// Construct state around already-built subsystems. Used by tests (which
    /// pass an in-memory `Db` and an `InMemorySecretStore`) and by any caller
    /// that wants to inject a specific backend. Returns the state plus the
    /// receiver half of the core-event channel.
    pub fn new(
        session_manager: SessionManager,
        secret_store: Arc<dyn SecretStore + Send + Sync>,
    ) -> (Self, UnboundedReceiver<CoreEvent>) {
        let (core_events, rx) = tokio::sync::mpsc::unbounded_channel();
        let state = AppState {
            session_manager: Arc::new(session_manager),
            secret_store,
            core_events,
            permission_registry: PermissionRegistry::new(),
            // Tests default to the built-in `autoDefault` policy and no connected
            // MCP servers; a test that needs a specific policy can override the
            // field on the returned state.
            policies: Arc::new(PolicyRegistry::new()),
            mcp_servers: Arc::new(Vec::new()),
        };
        (state, rx)
    }
}
