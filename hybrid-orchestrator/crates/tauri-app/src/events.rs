//! Bridge from `orchestrator-core` domain events to Tauri's `emit` API.
//!
//! In later phases this subscribes to the core `CoreEvent` stream and forwards
//! each event to the frontend over the Tauri event channel so the UI can react
//! to streaming tokens, routing decisions, tool invocations, and lifecycle
//! changes. Phase 0 provides only the placeholder shape.

use tauri::{AppHandle, Emitter};

/// Name of the Tauri event channel that core events are forwarded onto.
///
/// The frontend listens on this channel via the typed helpers in
/// `frontend/src/ipc/events.ts`.
// Temporary Phase 0 scaffolding: nothing emits on this channel yet. Remove this
// `allow` once Phase 1 wires the core event stream through `forward_core_event`.
#[allow(dead_code)]
pub const CORE_EVENT_CHANNEL: &str = "core://event";

/// Placeholder bridge that would forward a serialized core event to the
/// frontend. Phase 0 wires the shape only; no core events are emitted yet.
///
/// Later phases replace the `payload: ()` parameter with the real
/// `orchestrator_core::events::CoreEvent` type and spawn a task that pumps the
/// core event stream through here.
// Temporary Phase 0 scaffolding: no caller exists yet. Remove this `allow` once
// Phase 1 spawns the task that pumps the core event stream through here.
#[allow(dead_code)]
pub fn forward_core_event(app: &AppHandle, payload: ()) {
    // The result is intentionally ignored in the placeholder; real error
    // handling (logging failed emits) is added alongside the event stream.
    let _ = app.emit(CORE_EVENT_CHANNEL, payload);
}
