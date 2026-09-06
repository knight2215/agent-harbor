//! `#[tauri::command]` handlers exposed across the IPC boundary.
//!
//! Phase 0 exposes a single placeholder command, `app_version`, so the
//! frontend can prove the end-to-end IPC wiring works. Later phases add the
//! real command surface (conversations, providers, tools, personas), each
//! delegating into `orchestrator-core`.

/// Returns the application version compiled into the binary.
///
/// This is the single command registered in `main.rs`'s `generate_handler!`
/// for Phase 0. In Tauri v2, that registration is what authorizes a custom
/// command across the IPC boundary; no `capabilities/default.json` permission
/// entry is required (those gate Tauri's built-in plugins, not user commands).
#[tauri::command]
pub fn app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}
