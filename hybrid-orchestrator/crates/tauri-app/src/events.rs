//! Bridge from `orchestrator-core` domain events to Tauri's `emit` API.
//!
//! This subscribes to the core [`CoreEvent`] stream and forwards each event to
//! the frontend over a single Tauri event channel ([`CORE_EVENT_CHANNEL`]) so
//! the UI can react to streaming tokens, routing decisions, tool invocations,
//! and lifecycle changes (architecture.md Section 7.3: the core is
//! authoritative and events keep the frontend cache in sync).
//!
//! EVENT HYGIENE (Section 9.2): `CoreEvent` payloads are display-safe by
//! construction (ids, deltas, statuses, rationales); they never carry secret
//! material or `SecretRef` handles. This bridge forwards them verbatim and adds
//! nothing, so no secret can cross the boundary here.

use orchestrator_core::CoreEvent;
use tauri::{AppHandle, Emitter};
use tokio::sync::mpsc::UnboundedReceiver;

/// Name of the Tauri event channel that core events are forwarded onto.
///
/// The frontend listens on this channel via the typed helpers in
/// `frontend/src/ipc/events.ts`.
pub const CORE_EVENT_CHANNEL: &str = "core://event";

/// Forward a single core event to the frontend over [`CORE_EVENT_CHANNEL`].
///
/// The event is serialized by Tauri using the same serde `type`-tagged
/// camelCase representation the frontend's `CoreEvent` union mirrors. A failed
/// emit is logged rather than propagated: a dropped UI event must not tear down
/// the core.
pub fn forward_core_event(app: &AppHandle, event: CoreEvent) {
    if let Err(err) = app.emit(CORE_EVENT_CHANNEL, event) {
        eprintln!("failed to forward core event to frontend: {err}");
    }
}

/// Spawn the task that pumps the core event stream onto the Tauri channel.
///
/// The core produces [`CoreEvent`]s on an unbounded channel; this drains the
/// receiver and forwards each event to the frontend until the sender side is
/// dropped (app shutdown). Call once during setup with the receiver end of the
/// core's event channel.
pub fn spawn_core_event_bridge(app: AppHandle, mut rx: UnboundedReceiver<CoreEvent>) {
    tauri::async_runtime::spawn(async move {
        while let Some(event) = rx.recv().await {
            forward_core_event(&app, event);
        }
    });
}
