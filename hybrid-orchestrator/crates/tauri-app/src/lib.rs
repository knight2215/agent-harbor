//! Library surface of the Tauri shell.
//!
//! The shell is split into a thin `main.rs` binary and this library so that the
//! command handlers, managed state, and event bridge are reachable from
//! integration tests (`tests/`) and, in the standard Tauri v2 layout, from the
//! mobile entry points. `main.rs` calls [`run`] to build and start the app.
//!
//! It depends on the external `tauri` crate, so it is built in CI (where
//! crates.io is reachable) and is excluded from the offline default workspace
//! build. See the workspace `Cargo.toml` for the offline-build rationale.

pub mod commands;
pub mod events;
pub mod state;

use state::AppState;

/// The SQLite database filename used for the Phase 1 dev panel.
///
/// A fixed relative path is acceptable for now (architecture.md leaves the
/// per-OS app-data directory resolution to a later phase); the important
/// property exercised in tests is that the SAME file is reopened on restart and
/// the data survives (see `tests/restart_survival.rs`).
pub const DB_FILENAME: &str = "hybrid-orchestrator.sqlite3";

/// Build and run the Tauri application: open the DB, run migrations, construct
/// the subsystems, wire the core-event bridge, and start the event loop.
pub fn run() {
    // Open the DB, run migrations, and build the subsystems before starting the
    // Tauri event loop. `initialize` is async (sqlx), so block on it here. It
    // also hands back the receiver end of the core-event channel that the event
    // bridge drains onto the frontend.
    let (app_state, core_events_rx) =
        tauri::async_runtime::block_on(AppState::initialize(DB_FILENAME))
            .expect("failed to initialize application state (open db / run migrations)");

    tauri::Builder::default()
        // Phase 8b: enable the signed self-updater. `tauri_plugin_updater`
        // checks the GitHub Releases `latest.json` endpoint and verifies +
        // installs the signed update; `tauri_plugin_process` provides the
        // relaunch the in-app flow calls after a successful install.
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        // Native OS file-open dialog for the embedded-model "Browse…" button
        // (Local Runtimes settings), so a `.gguf` can be picked via the system
        // file explorer instead of typing an absolute path.
        .plugin(tauri_plugin_dialog::init())
        .manage(app_state)
        .setup(move |app| {
            // Pump core events to the frontend over CORE_EVENT_CHANNEL. The
            // sender half lives in `AppState.core_events`; the pipeline (Phase
            // 2) publishes onto it and the UI reacts (architecture.md 7.3).
            events::spawn_core_event_bridge(app.handle().clone(), core_events_rx);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::app_version,
            commands::list_conversations,
            commands::create_conversation,
            commands::rename_conversation,
            commands::set_conversation_tags,
            commands::delete_conversation,
            commands::list_personas,
            commands::create_persona,
            commands::update_persona,
            commands::delete_persona,
            commands::set_provider_secret,
            commands::set_local_runtime,
            commands::list_local_runtimes,
            commands::clear_local_runtime,
            commands::set_cloud_provider,
            commands::list_cloud_providers,
            commands::clear_cloud_provider,
            commands::list_available_models,
            commands::send_message,
            commands::resolve_permission,
            commands::get_messages,
            commands::set_conversation_route,
            commands::set_conversation_routing_mode,
            commands::assign_persona,
            commands::get_route_explanation,
            commands::list_mcp_servers,
            commands::add_mcp_server,
            commands::update_mcp_server,
            commands::remove_mcp_server,
            commands::set_mcp_enabled,
            commands::refresh_mcp_tools,
            commands::set_tool_permission,
            commands::export_conversation,
            commands::open_conversation,
            commands::stop_generation,
            commands::list_embedded_models,
            commands::import_embedded_model,
            commands::select_embedded_model,
            commands::load_embedded_model,
            commands::unload_embedded_model,
            commands::embedded_model_status,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
