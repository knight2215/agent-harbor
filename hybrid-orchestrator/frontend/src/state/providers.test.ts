import { describe, it, expect, vi, beforeEach } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
}));

import { useProvidersStore } from "./providers";
import type { AvailableModel } from "../types";

function model(providerId: string, id: string): AvailableModel {
  return {
    providerId,
    model: id,
    capabilities: {
      streaming: true,
      tools: true,
      vision: false,
      jsonMode: false,
      maxContext: null,
    },
    price: { inputPerMtok: 0, outputPerMtok: 0 },
  };
}

describe("providers store", () => {
  beforeEach(() => {
    invoke.mockReset();
    invoke.mockResolvedValue([]);
    useProvidersStore.setState({ models: [] });
  });

  it("load fetches available models from the core", async () => {
    invoke.mockResolvedValue([model("openai", "gpt-4o")]);
    await useProvidersStore.getState().load();
    expect(invoke).toHaveBeenCalledWith("list_available_models");
    expect(useProvidersStore.getState().models).toHaveLength(1);
  });

  it("providersChanged invalidates + refetches", () => {
    invoke.mockResolvedValue([model("openai", "gpt-4o")]);
    useProvidersStore.getState().applyCoreEvent({ type: "providersChanged" });
    expect(invoke).toHaveBeenCalledWith("list_available_models");
  });

  it("ignores unrelated events", () => {
    useProvidersStore.getState().applyCoreEvent({ type: "personasChanged" });
    expect(invoke).not.toHaveBeenCalled();
  });
});
