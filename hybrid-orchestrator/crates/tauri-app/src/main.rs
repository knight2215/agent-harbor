//! Tauri shell binary (thin adapter over the reusable core libraries).
//!
//! This binary is intentionally tiny: it delegates to [`tauri_app::run`], which
//! wires the managed application state, exposes the `#[tauri::command]`
//! handlers, and bridges core events to Tauri's `emit` API. Keeping the app
//! body in the library (`lib.rs`) lets integration tests reach the command,
//! state, and event modules. The crate depends on the external `tauri` crate,
//! so it is built in CI (where crates.io is reachable) and is excluded from the
//! offline default workspace build.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    tauri_app::run();
}
