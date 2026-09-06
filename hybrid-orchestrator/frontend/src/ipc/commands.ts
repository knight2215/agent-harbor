import { invoke } from "@tauri-apps/api/core";

/**
 * Typed wrappers over Tauri `invoke()` commands.
 *
 * Each function mirrors a `#[tauri::command]` handler in the Rust shell and is
 * the only sanctioned way the frontend crosses the IPC boundary. Phase 0
 * exposes the single `app_version` command; later phases extend this module as
 * new commands come online.
 */

/**
 * Fetch the application version compiled into the Tauri binary.
 *
 * Backed by the `app_version` command in `crates/tauri-app/src/commands.rs`.
 */
export function appVersion(): Promise<string> {
  return invoke<string>("app_version");
}
