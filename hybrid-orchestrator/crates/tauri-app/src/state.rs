//! Managed application state.
//!
//! Holds the Arc-wrapped subsystems the command handlers borrow across the IPC
//! boundary: the [`SessionManager`] (Section 7.5, over the persistence `Db`)
//! and the [`SecretStore`] (Section 9.1). Registered with the Tauri builder via
//! `.manage(...)` in `main.rs`.

use std::sync::Arc;

use orchestrator_core::{CoreEvent, SessionManager};
use persistence::{Db, PersistenceError};
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
        let session_manager = Arc::new(SessionManager::new(db));
        let secret_store: Arc<dyn SecretStore + Send + Sync> = Arc::new(KeyringSecretStore::new());
        let (core_events, rx) = tokio::sync::mpsc::unbounded_channel();
        let state = AppState {
            session_manager,
            secret_store,
            core_events,
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
        };
        (state, rx)
    }
}
