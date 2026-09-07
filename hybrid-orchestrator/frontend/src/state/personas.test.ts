import { describe, it, expect, vi, beforeEach } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
}));

import { usePersonasStore } from "./personas";
import type { AgentPersona } from "../types";

function persona(id: string): AgentPersona {
  return {
    id,
    name: "Coder",
    systemPrompt: "write code",
    defaultRoute: null,
    routingHint: null,
    allowedToolServers: [],
    parameters: {
      temperature: null,
      maxTokens: null,
      topP: null,
      frequencyPenalty: null,
      presencePenalty: null,
      stop: null,
    },
  };
}

describe("personas store", () => {
  beforeEach(() => {
    invoke.mockReset();
    invoke.mockResolvedValue([]);
    usePersonasStore.setState({ personas: [], selectedId: null });
  });

  it("load fetches personas from the core", async () => {
    invoke.mockResolvedValue([persona("p-1")]);
    await usePersonasStore.getState().load();
    expect(invoke).toHaveBeenCalledWith("list_personas");
    expect(usePersonasStore.getState().personas).toHaveLength(1);
  });

  it("create appends the persona and selects it", async () => {
    invoke.mockResolvedValue(persona("p-1"));
    const input = {
      name: "Coder",
      systemPrompt: "write code",
      defaultRoute: null,
      routingHint: null,
      allowedToolServers: [],
      parameters: persona("p-1").parameters,
    };
    await usePersonasStore.getState().create(input);
    expect(invoke).toHaveBeenCalledWith("create_persona", { persona: input });
    expect(usePersonasStore.getState().personas).toHaveLength(1);
    expect(usePersonasStore.getState().selectedId).toBe("p-1");
  });

  it("update replaces the matching persona", async () => {
    usePersonasStore.setState({ personas: [persona("p-1")] });
    const updated = { ...persona("p-1"), name: "Renamed" };
    invoke.mockResolvedValue(updated);
    const input = {
      name: "Renamed",
      systemPrompt: "write code",
      defaultRoute: null,
      routingHint: null,
      allowedToolServers: [],
      parameters: persona("p-1").parameters,
    };
    await usePersonasStore.getState().update("p-1", input);
    expect(invoke).toHaveBeenCalledWith("update_persona", { personaId: "p-1", persona: input });
    expect(usePersonasStore.getState().personas[0].name).toBe("Renamed");
  });

  it("remove deletes the persona and clears the selection", async () => {
    usePersonasStore.setState({ personas: [persona("p-1")], selectedId: "p-1" });
    invoke.mockResolvedValue(undefined);
    await usePersonasStore.getState().remove("p-1");
    expect(invoke).toHaveBeenCalledWith("delete_persona", { personaId: "p-1" });
    expect(usePersonasStore.getState().personas).toHaveLength(0);
    expect(usePersonasStore.getState().selectedId).toBeNull();
  });

  it("personasChanged invalidates + refetches", () => {
    invoke.mockResolvedValue([persona("p-1")]);
    usePersonasStore.getState().applyCoreEvent({ type: "personasChanged" });
    expect(invoke).toHaveBeenCalledWith("list_personas");
  });

  it("ignores unrelated events", () => {
    usePersonasStore.getState().applyCoreEvent({ type: "providersChanged" });
    expect(invoke).not.toHaveBeenCalled();
  });
});
