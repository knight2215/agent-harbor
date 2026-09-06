// Personas store (architecture.md Section 8.4 agent editor).
//
// Owns the list of saved personas. The core is authoritative (Section 7.3):
// `load` fetches via `list_personas`, and a `personasChanged` CoreEvent
// invalidates + refetches the cache.

import { create } from "zustand";
import { listPersonas } from "../ipc/commands";
import type { AgentPersona, CoreEvent } from "../types";

export interface PersonasState {
  /** All saved personas. */
  personas: AgentPersona[];
  /** Load the personas from the core. */
  load: () => Promise<void>;
  /** Apply a CoreEvent: refetch on `personasChanged`. */
  applyCoreEvent: (event: CoreEvent) => void;
}

export const usePersonasStore = create<PersonasState>((set, get) => ({
  personas: [],

  load: async () => {
    const personas = await listPersonas();
    set({ personas });
  },

  applyCoreEvent: (event) => {
    if (event.type === "personasChanged") {
      void get().load();
    }
  },
}));
