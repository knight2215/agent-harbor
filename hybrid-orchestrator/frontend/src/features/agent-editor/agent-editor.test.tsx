import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn().mockResolvedValue(() => undefined),
}));

import { usePersonasStore } from "../../state/personas";
import { useProvidersStore } from "../../state/providers";
import { useToolsStore } from "../../state/tools";
import { AllowedToolsSelector } from "./AllowedToolsSelector";
import { DefaultRoutePicker } from "./DefaultRoutePicker";
import { PersonaEditor } from "./PersonaEditor";
import { PersonaList } from "./PersonaList";
import type { AgentPersona, AvailableModel, McpServerConfig } from "../../types";

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

function model(providerId: string, id: string): AvailableModel {
  return {
    providerId,
    model: id,
    capabilities: {
      streaming: true,
      tools: true,
      vision: false,
      jsonMode: false,
      maxContext: 8000,
    },
    price: { inputPerMtok: 5, outputPerMtok: 15 },
    quality: 0.9,
  };
}

function server(id: string, name: string): McpServerConfig {
  return {
    id,
    name,
    transport: { type: "stdio", command: "mcp-fs", args: [], env: [] },
    permissionMode: "ask",
    enabled: true,
  };
}

function resetStores() {
  usePersonasStore.setState({ personas: [], selectedId: null });
  useProvidersStore.setState({ models: [] });
  useToolsStore.setState({ servers: [], connectionState: {}, tools: {}, errors: {} });
}

describe("agent editor", () => {
  beforeEach(() => {
    invoke.mockReset();
    invoke.mockResolvedValue(undefined);
    resetStores();
  });

  it("PersonaEditor save calls create_persona with the full PersonaInput", async () => {
    invoke.mockResolvedValue(persona("p-1"));
    useProvidersStore.setState({ models: [model("openai", "gpt-4o")] });
    useToolsStore.setState({ servers: [server("s-1", "fs")] });

    render(<PersonaEditor persona={null} />);

    fireEvent.change(screen.getByLabelText("Persona name"), { target: { value: "Coder" } });
    fireEvent.change(screen.getByLabelText("System prompt"), {
      target: { value: "write code" },
    });
    fireEvent.change(screen.getByLabelText("Temperature"), { target: { value: "0.7" } });
    // DefaultRoutePicker reuses the shared ProviderModelPicker.
    fireEvent.click(screen.getByRole("button", { name: /openai \/ gpt-4o/ }));
    // AllowedToolsSelector reflects the tools store.
    fireEvent.click(screen.getByRole("checkbox", { name: "fs" }));
    // RoutingHintControl.
    fireEvent.click(screen.getByRole("radio", { name: "Prefer local" }));

    fireEvent.click(screen.getByRole("button", { name: "Create persona" }));

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("create_persona", {
        persona: {
          name: "Coder",
          systemPrompt: "write code",
          defaultRoute: { providerId: "openai", model: "gpt-4o" },
          routingHint: "preferLocal",
          allowedToolServers: ["s-1"],
          parameters: {
            temperature: 0.7,
            maxTokens: null,
            topP: null,
            frequencyPenalty: null,
            presencePenalty: null,
            stop: null,
          },
        },
      });
    });
  });

  it("PersonaEditor save on an existing persona calls update_persona", async () => {
    invoke.mockResolvedValue(persona("p-1"));
    render(<PersonaEditor persona={persona("p-1")} />);
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith(
        "update_persona",
        expect.objectContaining({ personaId: "p-1" }),
      );
    });
  });

  it("PersonaEditor rejects an empty name / system prompt", () => {
    render(<PersonaEditor persona={null} />);
    fireEvent.click(screen.getByRole("button", { name: "Create persona" }));
    expect(screen.getByRole("alert")).toHaveTextContent("Name is required");
    expect(invoke).not.toHaveBeenCalled();
  });

  it("DefaultRoutePicker reuses the shared ProviderModelPicker (grouped)", () => {
    useProvidersStore.setState({ models: [model("openai", "gpt-4o")] });
    const onChange = vi.fn();
    render(<DefaultRoutePicker value={null} onChange={onChange} />);
    expect(screen.getByRole("region", { name: "Cloud" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /openai \/ gpt-4o/ }));
    expect(onChange).toHaveBeenCalledWith({ providerId: "openai", model: "gpt-4o" });
  });

  it("AllowedToolsSelector reflects the tools store and toggles ids", () => {
    useToolsStore.setState({ servers: [server("s-1", "fs"), server("s-2", "git")] });
    const onChange = vi.fn();
    render(<AllowedToolsSelector value={["s-1"]} onChange={onChange} />);
    expect(screen.getByRole("checkbox", { name: "fs" })).toBeChecked();
    expect(screen.getByRole("checkbox", { name: "git" })).not.toBeChecked();
    fireEvent.click(screen.getByRole("checkbox", { name: "git" }));
    expect(onChange).toHaveBeenCalledWith(["s-1", "s-2"]);
  });

  it("PersonaList selects and deletes personas", async () => {
    usePersonasStore.setState({ personas: [persona("p-1")] });
    invoke.mockResolvedValue(undefined);
    render(<PersonaList />);

    fireEvent.click(screen.getByRole("button", { name: "Coder" }));
    expect(usePersonasStore.getState().selectedId).toBe("p-1");

    fireEvent.click(screen.getByRole("button", { name: "Delete Coder" }));
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("delete_persona", { personaId: "p-1" });
    });
  });

  it("personas_changed refreshes the persona list", () => {
    invoke.mockResolvedValue([persona("p-1")]);
    usePersonasStore.getState().applyCoreEvent({ type: "personasChanged" });
    expect(invoke).toHaveBeenCalledWith("list_personas");
  });
});
