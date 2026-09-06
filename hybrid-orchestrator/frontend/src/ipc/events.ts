import { listen, type Event, type UnlistenFn } from "@tauri-apps/api/event";
import type { CoreEvent } from "../types";

/**
 * Typed listeners over the Tauri event API.
 *
 * The Rust shell forwards `orchestrator-core` domain events onto a single
 * channel (see `crates/tauri-app/src/events.rs`). The payload is the real
 * {@link CoreEvent} discriminated union mirrored from the core DTOs in
 * `../types` (a `type`-tagged union matching the Rust serde representation), so
 * callers can `switch (event.payload.type)` with full type narrowing.
 *
 * Event hygiene (architecture.md Section 9.1 / 9.2): payloads carry only
 * display-safe data (ids, deltas, statuses, rationales), never secrets.
 */

/** Name of the channel core events are forwarded onto. Mirrors the Rust `CORE_EVENT_CHANNEL`. */
export const CORE_EVENT_CHANNEL = "core://event";

/** Re-export the real event union so consumers can import it from the ipc layer. */
export type { CoreEvent } from "../types";

/**
 * Subscribe to forwarded core events.
 *
 * @param handler invoked for each event received on the core channel.
 * @returns a promise resolving to an unlisten function that tears down the subscription.
 */
export function onCoreEvent(handler: (event: Event<CoreEvent>) => void): Promise<UnlistenFn> {
  return listen<CoreEvent>(CORE_EVENT_CHANNEL, handler);
}
