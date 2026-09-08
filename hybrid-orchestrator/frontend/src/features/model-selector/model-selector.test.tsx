import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor, within } from "@testing-library/react";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn().mockResolvedValue(() => undefined),
}));

import { useConversationsStore } from "../../state/conversations";
import { useProvidersStore } from "../../state/providers";
import { PerMessageOverrideControl } from "./PerMessageOverrideControl";
import { ProviderModelPicker } from "./ProviderModelPicker";
import { RoutingModeToggle } from "./RoutingModeToggle";
import type { AvailableModel, Conversation } from "../../types";

function model(providerId: string, id: string, local: boolean): AvailableModel {
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
    price: local ? { inputPerMtok: 0, outputPerMtok: 0 } : { inputPerMtok: 5, outputPerMtok: 15 },
  };
}

function conversation(id: string): Conversation {
  return {
    id,
    title: "Chat",
    createdAt: "2024-01-01T00:00:00Z",
    updatedAt: "2024-01-01T00:00:00Z",
    personaId: null,
    conversationPref: null,
    routingMode: null,
    privacyTags: [],
    enabledToolServers: [],
  };
}

function resetStores() {
  useConversationsStore.setState({
    conversations: [],
    activeConversationId: null,
    messages: [],
    pendingOverride: null,
    pendingPermissions: [],
  });
  useProvidersStore.setState({ models: [] });
}

describe("model selector", () => {
  beforeEach(() => {
    invoke.mockReset();
    invoke.mockResolvedValue(undefined);
    resetStores();
  });

  it("ProviderModelPicker groups Local vs Cloud with capability + price hints", () => {
    const onChange = vi.fn();
    render(
      <ProviderModelPicker
        models={[model("lmstudio", "llama-3", true), model("openai", "gpt-4o", false)]}
        value={null}
        onChange={onChange}
      />,
    );
    // Groups exist.
    expect(screen.getByRole("region", { name: "Local" })).toBeInTheDocument();
    expect(screen.getByRole("region", { name: "Cloud" })).toBeInTheDocument();
    // Capability + price hints.
    expect(screen.getByText("free")).toBeInTheDocument();
    expect(screen.getByText("$5/$15 per Mtok")).toBeInTheDocument();
    expect(screen.getAllByText(/tools/).length).toBeGreaterThan(0);

    fireEvent.click(screen.getByRole("button", { name: /openai \/ gpt-4o/ }));
    expect(onChange).toHaveBeenCalledWith({ providerId: "openai", model: "gpt-4o" });
  });

  it("ProviderModelPicker groups a zero-priced Ollama model under Local", () => {
    render(
      <ProviderModelPicker
        models={[model("ollama-local", "llama3.1:8b", true), model("openai", "gpt-4o", false)]}
        value={null}
        onChange={vi.fn()}
      />,
    );
    // The zero-priced Ollama model surfaces under the Local group; the cloud
    // model under Cloud.
    const local = screen.getByRole("region", { name: "Local" });
    const cloud = screen.getByRole("region", { name: "Cloud" });
    expect(
      within(local).getByRole("button", { name: /ollama-local \/ llama3\.1:8b/ }),
    ).toBeInTheDocument();
    expect(within(cloud).getByRole("button", { name: /openai \/ gpt-4o/ })).toBeInTheDocument();
    // The Ollama option is not misfiled under Cloud.
    expect(
      within(cloud).queryByRole("button", { name: /ollama-local \/ llama3\.1:8b/ }),
    ).toBeNull();
  });

  it("RoutingModeToggle: renders four segmented positions", () => {
    useConversationsStore.setState({
      conversations: [conversation("c-1")],
      activeConversationId: "c-1",
    });

    render(<RoutingModeToggle />);
    expect(screen.getByRole("radio", { name: "Auto" })).toBeInTheDocument();
    expect(screen.getByRole("radio", { name: "Prefer Local" })).toBeInTheDocument();
    expect(screen.getByRole("radio", { name: "Prefer Quality" })).toBeInTheDocument();
    expect(screen.getByRole("radio", { name: "Manual" })).toBeInTheDocument();
  });

  it("RoutingModeToggle: Auto sets mode auto and clears the pin when leaving Manual", async () => {
    invoke.mockResolvedValue(conversation("c-1"));
    useConversationsStore.setState({
      conversations: [
        { ...conversation("c-1"), conversationPref: { providerId: "openai", model: "gpt-4o" } },
      ],
      activeConversationId: "c-1",
    });
    useProvidersStore.setState({ models: [model("openai", "gpt-4o", false)] });

    render(<RoutingModeToggle />);
    // Currently Manual (has a pin).
    expect(screen.getByRole("radio", { name: "Manual" })).toBeChecked();

    fireEvent.click(screen.getByRole("radio", { name: "Auto" }));
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("set_conversation_routing_mode", {
        conversationId: "c-1",
        mode: "auto",
      });
    });
    // Leaving Manual clears the pin.
    expect(invoke).toHaveBeenCalledWith("set_conversation_route", {
      conversationId: "c-1",
      route: null,
    });
  });

  it("RoutingModeToggle: Prefer Local / Prefer Quality set the matching mode", async () => {
    invoke.mockResolvedValue(conversation("c-1"));
    useConversationsStore.setState({
      conversations: [conversation("c-1")],
      activeConversationId: "c-1",
    });

    render(<RoutingModeToggle />);

    fireEvent.click(screen.getByRole("radio", { name: "Prefer Local" }));
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("set_conversation_routing_mode", {
        conversationId: "c-1",
        mode: "preferLocal",
      });
    });

    fireEvent.click(screen.getByRole("radio", { name: "Prefer Quality" }));
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("set_conversation_routing_mode", {
        conversationId: "c-1",
        mode: "preferQuality",
      });
    });
    // No pin existed, so no clearing call is issued.
    expect(invoke).not.toHaveBeenCalledWith("set_conversation_route", {
      conversationId: "c-1",
      route: null,
    });
  });

  it("RoutingModeToggle: Manual sets mode manual, reveals the picker, and pins", async () => {
    invoke.mockResolvedValue(conversation("c-1"));
    useConversationsStore.setState({
      conversations: [conversation("c-1")],
      activeConversationId: "c-1",
    });
    useProvidersStore.setState({ models: [model("openai", "gpt-4o", false)] });

    render(<RoutingModeToggle />);
    expect(screen.getByRole("radio", { name: "Auto" })).toBeChecked();

    fireEvent.click(screen.getByRole("radio", { name: "Manual" }));
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("set_conversation_routing_mode", {
        conversationId: "c-1",
        mode: "manual",
      });
    });

    fireEvent.click(screen.getByRole("button", { name: /openai \/ gpt-4o/ }));
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("set_conversation_route", {
        conversationId: "c-1",
        route: { providerId: "openai", model: "gpt-4o" },
      });
    });
  });

  it("PerMessageOverrideControl writes the shared store override; a send carries it", async () => {
    useConversationsStore.setState({ activeConversationId: "c-1" });
    useProvidersStore.setState({ models: [model("openai", "gpt-4o", false)] });

    render(<PerMessageOverrideControl />);
    fireEvent.click(screen.getByRole("button", { name: /openai \/ gpt-4o/ }));

    // The shared transient override in the conversations store is updated.
    expect(useConversationsStore.getState().pendingOverride).toEqual({
      providerId: "openai",
      model: "gpt-4o",
    });

    // A subsequent send consumes-and-clears it.
    await useConversationsStore.getState().sendMessage("hi");
    expect(invoke).toHaveBeenCalledWith("send_message", {
      conversationId: "c-1",
      content: "hi",
      overrideRoute: { providerId: "openai", model: "gpt-4o" },
    });
    expect(useConversationsStore.getState().pendingOverride).toBeNull();
  });

  it("PerMessageOverrideControl shows guidance (not a bare picker) when no models exist", () => {
    useConversationsStore.setState({ activeConversationId: "c-1" });
    useProvidersStore.setState({ models: [] });

    render(<PerMessageOverrideControl />);

    // The 'Override next message' label renders exactly once.
    expect(screen.getAllByText("Override next message")).toHaveLength(1);
    // Empty state guides the user to Settings -> Providers & Keys and mentions
    // a local runtime, and there is exactly ONE such guidance node (no stacked
    // empty pickers). Assert via the stable testid as well as the copy.
    expect(screen.getAllByTestId("no-models-empty-state")).toHaveLength(1);
    const empties = screen.getAllByText(/Providers & Keys/);
    expect(empties).toHaveLength(1);
    expect(screen.getByText(/local runtime/)).toBeInTheDocument();
  });

  it("ProviderModelPicker empty state guides the user to configure a provider", () => {
    render(<ProviderModelPicker models={[]} value={null} onChange={vi.fn()} />);
    expect(screen.getByText(/Providers & Keys/)).toBeInTheDocument();
    expect(screen.getByText(/local runtime/)).toBeInTheDocument();
  });

  it("providers_changed refreshes the model list", async () => {
    invoke.mockResolvedValue([model("openai", "gpt-4o", false)]);
    useProvidersStore.getState().applyCoreEvent({ type: "providersChanged" });
    expect(invoke).toHaveBeenCalledWith("list_available_models");
    await waitFor(() => {
      expect(useProvidersStore.getState().models).toHaveLength(1);
    });
  });
});
