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
    usePersonasStore.setState({ personas: [] });
  });

  it("load fetches personas from the core", async () => {
    invoke.mockResolvedValue([persona("p-1")]);
    await usePersonasStore.getState().load();
    expect(invoke).toHaveBeenCalledWith("list_personas");
    expect(usePersonasStore.getState().personas).toHaveLength(1);
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
