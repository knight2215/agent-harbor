import { listen, type Event, type UnlistenFn } from "@tauri-apps/api/event";

/**
 * Typed listeners over the Tauri event API.
 *
 * The Rust shell forwards `orchestrator-core` domain events onto a single
 * channel (see `crates/tauri-app/src/events.rs`). Phase 0 provides the typed
 * shape only; no events are emitted yet. Later phases define the concrete
 * `CoreEvent` union (mirrored from the core DTOs in `../types/`) and richer
 * per-variant listeners.
 */

/** Name of the channel core events are forwarded onto. Mirrors the Rust `CORE_EVENT_CHANNEL`. */
export const CORE_EVENT_CHANNEL = "core://event";

/** Placeholder payload for a forwarded core event. Replaced with the real union in later phases. */
export type CoreEvent = unknown;

/**
 * Subscribe to forwarded core events.
 *
 * @param handler invoked for each event received on the core channel.
 * @returns a promise resolving to an unlisten function that tears down the subscription.
 */
export function onCoreEvent(handler: (event: Event<CoreEvent>) => void): Promise<UnlistenFn> {
  return listen<CoreEvent>(CORE_EVENT_CHANNEL, handler);
}
