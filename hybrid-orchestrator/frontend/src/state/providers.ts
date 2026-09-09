// Providers store (architecture.md Section 8.2 model selector).
//
// Owns the list of available provider/model options (with capabilities + price)
// that the model selector groups into Local vs Cloud. The core is authoritative
// (Section 7.3): `load` fetches via `list_available_models`, and a
// `providersChanged` CoreEvent invalidates + refetches the cache.

import { create } from "zustand";
import { listAvailableModels } from "../ipc/commands";
import type { AvailableModel, CoreEvent, ProviderEnumerationError } from "../types";

/**
 * The lifecycle of the most recent {@link ProvidersState.load}. `idle` before
 * the first load; `loading` while the `list_available_models` IPC call is in
 * flight; `loaded` once it resolved; `failed` when the IPC call REJECTED (a
 * thrown CommandError). The `failed` state exists so a rejected load is
 * self-reported instead of silently leaving `models`/`errors` empty with no
 * signal (the "No models, no error" clean-empty bug).
 */
export type ProvidersLoadState = "idle" | "loading" | "loaded" | "failed";

export interface ProvidersState {
  /** Available models across all configured providers. */
  models: AvailableModel[];
  /**
   * Per-provider enumeration failures (display-safe), so the UI can show why a
   * misconfigured or unreachable provider contributed no models. Empty when
   * every configured provider enumerated successfully.
   */
  errors: ProviderEnumerationError[];
  /** The lifecycle of the most recent {@link ProvidersState.load}. */
  loadState: ProvidersLoadState;
  /**
   * The display-safe message of the last load REJECTION (the thrown
   * CommandError), or `null` when the last load did not reject. Set when
   * `loadState === "failed"` so the UI can show WHY the load failed instead of
   * a blank picker. Never carries secret material.
   */
  lastError: string | null;
  /** Load the available models (and enumeration errors) from the core. */
  load: () => Promise<void>;
  /** Apply a CoreEvent: refetch on `providersChanged`. */
  applyCoreEvent: (event: CoreEvent) => void;
}

export const useProvidersStore = create<ProvidersState>((set, get) => ({
  models: [],
  errors: [],
  loadState: "idle",
  lastError: null,

  load: async () => {
    // Capture an IPC REJECTION into `loadState`/`lastError` instead of letting
    // it escape: `list_available_models` returning a CommandError rejects this
    // promise, and app.tsx fires the startup load as `void ...load()`, so a
    // thrown error would otherwise be swallowed and leave models:[]/errors:[]
    // with no signal (the clean-empty bug). On failure the previous `models`
    // are left as-is (not blanked) so a transient refetch failure does not wipe
    // a previously-good list.
    set({ loadState: "loading" });
    try {
      const result = await listAvailableModels();
      set({
        models: result.models ?? [],
        errors: result.errors ?? [],
        loadState: "loaded",
        lastError: null,
      });
    } catch (e) {
      set({ loadState: "failed", lastError: e instanceof Error ? e.message : String(e) });
    }
  },

  applyCoreEvent: (event) => {
    if (event.type === "providersChanged") {
      void get().load();
    }
  },
}));
