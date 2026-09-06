// Providers store (architecture.md Section 8.2 model selector).
//
// Owns the list of available provider/model options (with capabilities + price)
// that the model selector groups into Local vs Cloud. The core is authoritative
// (Section 7.3): `load` fetches via `list_available_models`, and a
// `providersChanged` CoreEvent invalidates + refetches the cache.

import { create } from "zustand";
import { listAvailableModels } from "../ipc/commands";
import type { AvailableModel, CoreEvent } from "../types";

export interface ProvidersState {
  /** Available models across all configured providers. */
  models: AvailableModel[];
  /** Load the available models from the core. */
  load: () => Promise<void>;
  /** Apply a CoreEvent: refetch on `providersChanged`. */
  applyCoreEvent: (event: CoreEvent) => void;
}

export const useProvidersStore = create<ProvidersState>((set, get) => ({
  models: [],

  load: async () => {
    const models = await listAvailableModels();
    set({ models });
  },

  applyCoreEvent: (event) => {
    if (event.type === "providersChanged") {
      void get().load();
    }
  },
}));
