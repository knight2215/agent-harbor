// Personas store (architecture.md Section 8.4 agent editor).
//
// Owns the list of saved personas plus the currently selected one (for the
// editor). The core is authoritative (Section 7.3): `load` fetches via
// `list_personas`, mutations go through the `create_persona` / `update_persona`
// / `delete_persona` commands, and a `personasChanged` CoreEvent invalidates +
// refetches the cache.

import { create } from "zustand";
import {
  createPersona as createPersonaCmd,
  deletePersona as deletePersonaCmd,
  listPersonas,
  updatePersona as updatePersonaCmd,
  type PersonaInput,
} from "../ipc/commands";
import type { AgentPersona, CoreEvent } from "../types";

export interface PersonasState {
  /** All saved personas. */
  personas: AgentPersona[];
  /** The id of the persona currently open in the editor, or null. */
  selectedId: string | null;
  /** Load the personas from the core. */
  load: () => Promise<void>;
  /** Select a persona for editing (or clear the selection with null). */
  select: (id: string | null) => void;
  /** Create a new persona and select it. */
  create: (persona: PersonaInput) => Promise<AgentPersona>;
  /** Update an existing persona. */
  update: (id: string, persona: PersonaInput) => Promise<AgentPersona>;
  /** Delete a persona by id (clears the selection when it was selected). */
  remove: (id: string) => Promise<void>;
  /** Apply a CoreEvent: refetch on `personasChanged`. */
  applyCoreEvent: (event: CoreEvent) => void;
}

export const usePersonasStore = create<PersonasState>((set, get) => ({
  personas: [],
  selectedId: null,

  load: async () => {
    const personas = await listPersonas();
    set({ personas });
  },

  select: (id) => {
    set({ selectedId: id });
  },

  create: async (persona) => {
    const created = await createPersonaCmd(persona);
    set((state) => ({
      personas: [...state.personas, created],
      selectedId: created.id,
    }));
    return created;
  },

  update: async (id, persona) => {
    const updated = await updatePersonaCmd(id, persona);
    set((state) => ({
      personas: state.personas.map((p) => (p.id === updated.id ? updated : p)),
    }));
    return updated;
  },

  remove: async (id) => {
    await deletePersonaCmd(id);
    set((state) => ({
      personas: state.personas.filter((p) => p.id !== id),
      selectedId: state.selectedId === id ? null : state.selectedId,
    }));
  },

  applyCoreEvent: (event) => {
    if (event.type === "personasChanged") {
      void get().load();
    }
  },
}));
