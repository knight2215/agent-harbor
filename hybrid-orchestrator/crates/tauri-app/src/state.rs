//! Managed application state.
//!
//! Holds the Arc-wrapped subsystems the command handlers borrow across the IPC
//! boundary: the [`SessionManager`] (Section 7.5, over the persistence `Db`)
//! and the [`SecretStore`] (Section 9.1). Registered with the Tauri builder via
//! `.manage(...)` in `main.rs`.

use std::collections::HashMap;
use std::sync::Arc;

use engine::EmbeddedEngine;
use mcp_client::{McpConnectionState, McpServerHandle};
use orchestrator_core::{CoreEvent, McpServerConfig, PermissionRegistry, SessionManager};
use persistence::config::AppConfig;
use persistence::{Db, McpServerRepo, PersistenceError};
use routing::PolicyRegistry;
use secrets::{KeyringSecretStore, SecretStore};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::RwLock;
use uuid::Uuid;

/// A shared, runtime-mutable registry of connected MCP server handles
/// (architecture.md Section 5.3 / 8.3). The MCP-manager commands
/// (`add_mcp_server`, `set_mcp_enabled`, `remove_mcp_server`, ...) add, replace,
/// reconnect, and tear down handles here at runtime, and `send_message` reads
/// the live connected set from it instead of a fixed empty vec.
///
/// Keyed by the server's [`Uuid`]. Behind an `Arc<RwLock<..>>` so it is cheap to
/// clone into command tasks and the startup connector, `Send + Sync` for Tauri
/// managed state, and safe to mutate while the pipeline reads a snapshot.
#[derive(Clone, Default)]
pub struct McpRegistry {
    handles: Arc<RwLock<HashMap<Uuid, Arc<McpServerHandle>>>>,
}

impl McpRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        McpRegistry {
            handles: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Insert (or replace) the handle for `id`, returning the previous handle
    /// if one was registered (so the caller can tear it down).
    pub async fn insert(
        &self,
        id: Uuid,
        handle: Arc<McpServerHandle>,
    ) -> Option<Arc<McpServerHandle>> {
        self.handles.write().await.insert(id, handle)
    }

    /// Remove and return the handle for `id`, if present.
    pub async fn remove(&self, id: Uuid) -> Option<Arc<McpServerHandle>> {
        self.handles.write().await.remove(&id)
    }

    /// The handle for `id`, if present.
    pub async fn get(&self, id: Uuid) -> Option<Arc<McpServerHandle>> {
        self.handles.read().await.get(&id).cloned()
    }

    /// A snapshot of the currently registered handles. `send_message` reads this
    /// so the pipeline attaches tools from the live connected set.
    pub async fn snapshot(&self) -> Vec<Arc<McpServerHandle>> {
        self.handles.read().await.values().cloned().collect()
    }
}

/// Connect `handle` in the background and report its resulting state to the
/// frontend via [`CoreEvent`]s. Never blocks the caller: a slow or broken
/// server must not stall startup or a command.
///
/// On success emits [`CoreEvent::McpStateChanged`] with the handle's live state;
/// on failure emits [`CoreEvent::McpError`] plus a `Disconnected` state so the
/// Tool/MCP manager surface (Section 8.3) reflects the outcome.
pub fn spawn_connect(
    handle: Arc<McpServerHandle>,
    server_id: Uuid,
    events: UnboundedSender<CoreEvent>,
) {
    tokio::spawn(async move {
        let _ = events.send(CoreEvent::McpStateChanged {
            server_id,
            state: to_core_state(McpConnectionState::Connecting),
        });
        match handle.connect().await {
            Ok(()) => {
                let _ = events.send(CoreEvent::McpStateChanged {
                    server_id,
                    state: to_core_state(handle.state().await),
                });
            }
            Err(err) => {
                let _ = events.send(CoreEvent::McpError {
                    server_id,
                    message: err.to_string(),
                });
                let _ = events.send(CoreEvent::McpStateChanged {
                    server_id,
                    state: to_core_state(McpConnectionState::Disconnected),
                });
            }
        }
    });
}

/// Map the mcp-client connection state onto the core event enum (the two are
/// intentionally-parallel enums kept in sync; see the mcp-client module docs).
pub fn to_core_state(state: McpConnectionState) -> orchestrator_core::McpConnectionState {
    match state {
        McpConnectionState::Connecting => orchestrator_core::McpConnectionState::Connecting,
        McpConnectionState::Connected => orchestrator_core::McpConnectionState::Connected,
        McpConnectionState::Disconnected => orchestrator_core::McpConnectionState::Disconnected,
    }
}

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
    /// invocation.
    pub permission_registry: PermissionRegistry,
    /// The routing policy registry (architecture.md Section 6.4). Holds the
    /// selectable/persisted active automatic policy; the `send_message`
    /// pipeline calls [`PolicyRegistry::resolve`] to route each turn. The active
    /// id is read from `AppConfig.active_routing_policy` at
    /// [`AppState::initialize`], defaulting to the built-in `autoDefault`.
    /// Behind `Arc` so it is cheaply cloned into the spawned pipeline task.
    pub policies: Arc<PolicyRegistry>,
    /// The runtime MCP handle registry (architecture.md Section 5.3 / 8.3). The
    /// MCP-manager commands add/replace/reconnect/teardown handles here and
    /// `send_message` reads the live connected set from it, so the pipeline
    /// attaches tools from whatever servers are currently connected.
    pub mcp_servers: McpRegistry,
    /// The embedded local inference engine (Strategy B / FEAT-002). Holds the
    /// registry of imported local `.gguf` models plus, on the native `llama`
    /// path, the one-model-at-a-time load state. The embedded-model lifecycle
    /// commands (`list`/`import`/`select`/`load`/`unload`/`status`) are thin
    /// adapters over it. Behind an `Arc` so it is cheap to clone into async
    /// command tasks and is `Send + Sync` for Tauri managed state.
    pub embedded_engine: Arc<EmbeddedEngine>,
    /// The id of the embedded model the user has selected/loaded (if any), for
    /// the embedded-model lifecycle commands. This is display-only lifecycle
    /// state distinct from the engine's model REGISTRY: `select`/`load` set it
    /// (after validating the id is registered), `unload` clears it, and
    /// `status` reports it. Behind an `RwLock` so command tasks can read/update
    /// it concurrently; `Arc` keeps it cheap to clone into those tasks.
    pub embedded_loaded_model: Arc<RwLock<Option<String>>>,
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

        // Load persisted MCP server rows so the enabled ones can be connected in
        // the background below (Section 5.3): startup must not block on a slow or
        // broken server, so each connect is spawned and reports status via
        // mcp_state_changed / mcp_error events.
        let persisted_servers: Vec<McpServerConfig> =
            McpServerRepo::new(&db).list().await.unwrap_or_default();

        let session_manager = Arc::new(SessionManager::new(db));
        let secret_store: Arc<dyn SecretStore + Send + Sync> = Arc::new(KeyringSecretStore::new());
        let (core_events, rx) = tokio::sync::mpsc::unbounded_channel();
        let mcp_servers = McpRegistry::new();

        // Register every persisted server's handle and connect the enabled ones
        // in the background (Section 5.3): a disabled server is registered but
        // left disconnected until `set_mcp_enabled` turns it on.
        for config in persisted_servers {
            let id = config.id;
            let enabled = config.enabled;
            let handle = Arc::new(McpServerHandle::new(config));
            mcp_servers.insert(id, handle.clone()).await;
            if enabled {
                spawn_connect(handle, id, core_events.clone());
            }
        }

        let state = AppState {
            session_manager,
            secret_store,
            core_events,
            permission_registry: PermissionRegistry::new(),
            policies: Arc::new(policies),
            mcp_servers,
            embedded_engine: Arc::new(EmbeddedEngine::empty()),
            embedded_loaded_model: Arc::new(RwLock::new(None)),
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
            // Tests default to the built-in `autoDefault` policy and an empty MCP
            // registry; a test that needs a specific policy or a connected server
            // can override the field on the returned state.
            policies: Arc::new(PolicyRegistry::new()),
            mcp_servers: McpRegistry::new(),
            embedded_engine: Arc::new(EmbeddedEngine::empty()),
            embedded_loaded_model: Arc::new(RwLock::new(None)),
        };
        (state, rx)
    }
}
