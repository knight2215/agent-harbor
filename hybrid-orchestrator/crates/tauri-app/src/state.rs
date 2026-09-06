//! Managed application state.
//!
//! In later phases this holds the Arc-wrapped subsystems (session manager,
//! provider registry, MCP client, persistence pool, secret store) that the
//! command handlers borrow. For Phase 0 it is an empty placeholder registered
//! with the Tauri builder via `.manage(AppState::default())`.

/// Application-wide state managed by Tauri and shared across command handlers.
///
/// Phase 0 keeps this empty; fields are added as subsystems come online. The
/// type derives `Default` so it can be constructed with `AppState::default()`
/// and is `Send + Sync` so Tauri can manage it behind an `Arc`.
#[derive(Debug, Default)]
pub struct AppState {}
