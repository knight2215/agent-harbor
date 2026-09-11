import { describe, it, expect, vi, beforeEach } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
}));

import { useProvidersStore } from "./providers";
import type { AvailableModel, AvailableModelsResult, ProviderEnumerationError } from "../types";

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
    quality: 0.6,
  };
}

function result(
  models: AvailableModel[],
  errors: ProviderEnumerationError[] = [],
): AvailableModelsResult {
  return { models, errors };
}

describe("providers store", () => {
  beforeEach(() => {
    invoke.mockReset();
    invoke.mockResolvedValue(result([]));
    useProvidersStore.setState({ models: [], errors: [], loadState: "idle", lastError: null });
  });

  it("load fetches available models from the core", async () => {
    invoke.mockResolvedValue(result([model("openai", "gpt-4o")]));
    await useProvidersStore.getState().load();
    expect(invoke).toHaveBeenCalledWith("list_available_models");
    expect(useProvidersStore.getState().models).toHaveLength(1);
    expect(useProvidersStore.getState().errors).toHaveLength(0);
  });

  it("load stores per-provider enumeration errors alongside the models", async () => {
    invoke.mockResolvedValue(
      result(
        [model("openai", "gpt-4o")],
        [{ providerId: "ollama-local", message: "transport error: connection refused" }],
      ),
    );
    await useProvidersStore.getState().load();
    expect(useProvidersStore.getState().models).toHaveLength(1);
    const errors = useProvidersStore.getState().errors;
    expect(errors).toHaveLength(1);
    expect(errors[0].providerId).toBe("ollama-local");
    expect(errors[0].message).toContain("transport error");
  });

  it("load marks loadState 'loaded' on success", async () => {
    invoke.mockResolvedValue(result([model("openai", "gpt-4o")]));
    await useProvidersStore.getState().load();
    expect(useProvidersStore.getState().loadState).toBe("loaded");
    expect(useProvidersStore.getState().lastError).toBeNull();
  });

  it("load captures an IPC rejection into loadState 'failed' + lastError (does not throw)", async () => {
    // The clean-empty cause A: a rejected `list_available_models` must NOT
    // escape (it would be swallowed by app.tsx's `void ...load()`), and must NOT
    // leave models:[]/errors:[] with no signal. Instead it records the failure.
    invoke.mockRejectedValue(new Error("provider registry build failed"));
    await expect(useProvidersStore.getState().load()).resolves.toBeUndefined();
    expect(useProvidersStore.getState().loadState).toBe("failed");
    expect(useProvidersStore.getState().lastError).toBe("provider registry build failed");
  });

  it("providersChanged invalidates + refetches", () => {
    invoke.mockResolvedValue(result([model("openai", "gpt-4o")]));
    useProvidersStore.getState().applyCoreEvent({ type: "providersChanged" });
    expect(invoke).toHaveBeenCalledWith("list_available_models");
  });

  it("ignores unrelated events", () => {
    useProvidersStore.getState().applyCoreEvent({ type: "personasChanged" });
    expect(invoke).not.toHaveBeenCalled();
  });
});
