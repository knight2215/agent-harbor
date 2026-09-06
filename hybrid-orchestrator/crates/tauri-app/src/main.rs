//! Tauri shell binary (thin adapter over the reusable core libraries).
//!
//! This crate wires the managed application state, exposes the
//! `#[tauri::command]` handlers, and bridges core events to Tauri's `emit`
//! API. It depends on the external `tauri` crate, so it is built in CI (where
//! crates.io is reachable) and is excluded from the offline default workspace
//! build. See the workspace `Cargo.toml` for the offline-build rationale.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod events;
mod state;

use state::AppState;

fn main() {
    tauri::Builder::default()
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![commands::app_version])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
