// Providers store (architecture.md Section 8.2 model selector).
//
// Owns the list of available provider/model options (with capabilities + price)
// that the model selector groups into Local vs Cloud. The core is authoritative
// (Section 7.3): `load` fetches via `list_available_models`, and a
// `providersChanged` CoreEvent invalidates + refetches the cache.

import { create } from "zustand";
import { listAvailableModels } from "../ipc/commands";
import type { AvailableModel, CoreEvent, ProviderEnumerationError } from "../types";

export interface ProvidersState {
  /** Available models across all configured providers. */
  models: AvailableModel[];
  /**
   * Per-provider enumeration failures (display-safe), so the UI can show why a
   * misconfigured or unreachable provider contributed no models. Empty when
   * every configured provider enumerated successfully.
   */
  errors: ProviderEnumerationError[];
  /** Load the available models (and enumeration errors) from the core. */
  load: () => Promise<void>;
  /** Apply a CoreEvent: refetch on `providersChanged`. */
  applyCoreEvent: (event: CoreEvent) => void;
}

export const useProvidersStore = create<ProvidersState>((set, get) => ({
  models: [],
  errors: [],

  load: async () => {
    const result = await listAvailableModels();
    set({ models: result.models ?? [], errors: result.errors ?? [] });
  },

  applyCoreEvent: (event) => {
    if (event.type === "providersChanged") {
      void get().load();
    }
  },
}));
