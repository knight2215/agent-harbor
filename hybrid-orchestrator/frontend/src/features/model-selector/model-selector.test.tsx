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
import { AutoRationaleTooltip } from "./AutoRationaleTooltip";
import { EnumerationErrorModal } from "./EnumerationErrorModal";
import { InlineModelControl } from "./InlineModelControl";
import { PerMessageOverrideControl } from "./PerMessageOverrideControl";
import { ProviderModelPicker } from "./ProviderModelPicker";
import { RoutingModeToggle } from "./RoutingModeToggle";
import type {
  AvailableModel,
  AvailableModelsResult,
  Conversation,
  ProviderEnumerationError,
} from "../../types";

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

function result(
  models: AvailableModel[],
  errors: ProviderEnumerationError[] = [],
): AvailableModelsResult {
  return { models, errors };
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

// The providers store is a module-level singleton; capture its REAL load()
// before any test swaps in a spy so resetStores() can restore it (a leaked spy
// would otherwise break the tests that drive load()/applyCoreEvent directly).
const realProvidersLoad = useProvidersStore.getState().load;

function resetStores() {
  useConversationsStore.setState({
    conversations: [],
    activeConversationId: null,
    messages: [],
    pendingOverride: null,
    webSearchEnabled: false,
    pendingPermissions: [],
  });
  useProvidersStore.setState({
    models: [],
    errors: [],
    loadState: "idle",
    lastError: null,
    load: realProvidersLoad,
  });
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

  it("ProviderModelPicker groups a zero-priced embedded model under Local", async () => {
    // Embedded (in-process llama.cpp) models are seeded to zero price on the
    // backend, so the zero-price heuristic must surface them under Local. This
    // test covers the UI GROUPING contract; that the real backend pipeline
    // actually emits such a zero-priced embedded AvailableModel is proven by the
    // Rust test `imported_embedded_model_surfaces_in_list_available_models`
    // (crates/tauri-app/src/commands.rs). Drive the models through the providers
    // store (an async state set) and assert with findBy* after the update.
    useProvidersStore.setState({
      models: [
        model("embedded", "local-llama-3.gguf", true),
        model("anthropic", "claude-3-5-sonnet", false),
      ],
    });
    const models = useProvidersStore.getState().models;

    render(<ProviderModelPicker models={models} value={null} onChange={vi.fn()} />);

    const local = await screen.findByRole("region", { name: "Local" });
    const cloud = await screen.findByRole("region", { name: "Cloud" });
    // The embedded model is filed under Local (as a free/local model), not Cloud.
    expect(
      within(local).getByRole("button", { name: /embedded \/ local-llama-3\.gguf/ }),
    ).toBeInTheDocument();
    expect(
      within(cloud).queryByRole("button", { name: /embedded \/ local-llama-3\.gguf/ }),
    ).toBeNull();
    // Its price hint reads as free (the natural Local signal).
    expect(
      within(local).getByRole("button", { name: /embedded \/ local-llama-3\.gguf/ }),
    ).toHaveTextContent("free");
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
    invoke.mockResolvedValue(result([model("openai", "gpt-4o", false)]));
    useProvidersStore.getState().applyCoreEvent({ type: "providersChanged" });
    expect(invoke).toHaveBeenCalledWith("list_available_models");
    await waitFor(() => {
      expect(useProvidersStore.getState().models).toHaveLength(1);
    });
  });

  it("PerMessageOverrideControl renders enumeration errors alongside the models without blanking the picker", async () => {
    useConversationsStore.setState({ activeConversationId: "c-1" });
    // The core enumeration returned one healthy model AND one failing provider:
    // the picker must still show the model while the error is surfaced too.
    invoke.mockResolvedValue(
      result(
        [model("openai", "gpt-4o", false)],
        [{ providerId: "ollama-local", message: "transport error: connection refused" }],
      ),
    );
    await useProvidersStore.getState().load();

    render(<PerMessageOverrideControl />);

    // The healthy model is still selectable (the picker is NOT blanked).
    expect(await screen.findByRole("button", { name: /openai \/ gpt-4o/ })).toBeInTheDocument();
    // The empty state is NOT shown because models exist.
    expect(screen.queryByTestId("no-models-empty-state")).toBeNull();
    // The enumeration error is surfaced with the failing provider id + message.
    const errors = await screen.findByTestId("provider-enumeration-errors");
    expect(within(errors).getByText(/ollama-local/)).toBeInTheDocument();
    expect(within(errors).getByText(/transport error: connection refused/)).toBeInTheDocument();
    // The copyable status line reports the loaded outcome (1 model, 1 provider).
    expect(screen.getByTestId("model-load-status")).toHaveTextContent(
      "loaded 1 models from 1 providers",
    );
  });

  it("PerMessageOverrideControl status line reports a failed load with the error message", async () => {
    useConversationsStore.setState({ activeConversationId: "c-1" });
    // A rejected list_available_models must self-report via the status line
    // instead of a silent empty picker (the clean-empty bug).
    invoke.mockRejectedValue(new Error("provider registry build failed"));
    await useProvidersStore.getState().load();

    render(<PerMessageOverrideControl />);

    expect(screen.getByTestId("model-load-status")).toHaveTextContent(
      "failed: provider registry build failed",
    );
  });

  it("PerMessageOverrideControl exposes a Refresh models control that re-enumerates via the store", async () => {
    useConversationsStore.setState({ activeConversationId: "c-1" });
    // Start empty; the refresh will fetch a model and populate the picker.
    useProvidersStore.setState({ models: [], errors: [] });
    invoke.mockResolvedValue(result([model("openai", "gpt-4o", false)]));

    render(<PerMessageOverrideControl />);

    // Before refresh: the empty-state guidance is shown (no models yet).
    expect(screen.getByTestId("no-models-empty-state")).toBeInTheDocument();

    // Click the always-available Refresh affordance (accessible by aria-label).
    fireEvent.click(screen.getByRole("button", { name: "Refresh models" }));

    // It re-runs list_available_models through the store's load().
    expect(invoke).toHaveBeenCalledWith("list_available_models");
    // After the async load resolves, the fetched model is selectable.
    expect(await screen.findByRole("button", { name: /openai \/ gpt-4o/ })).toBeInTheDocument();
    expect(screen.queryByTestId("no-models-empty-state")).toBeNull();
  });

  it("PerMessageOverrideControl Refresh models control calls the providers store load()", () => {
    useConversationsStore.setState({ activeConversationId: "c-1" });
    const load = vi.fn();
    useProvidersStore.setState({ load });

    render(<PerMessageOverrideControl />);
    fireEvent.click(screen.getByRole("button", { name: "Refresh models" }));
    expect(load).toHaveBeenCalled();
    // The next test's beforeEach -> resetStores() restores the real load(), so
    // the seeded spy does not leak.
  });

  it("PerMessageOverrideControl shows enumeration errors even when no models exist", async () => {
    useConversationsStore.setState({ activeConversationId: "c-1" });
    invoke.mockResolvedValue(
      result([], [{ providerId: "ollama-local", message: "transport error: connection refused" }]),
    );
    await useProvidersStore.getState().load();

    render(<PerMessageOverrideControl />);

    // The truly-empty case still guides the user AND surfaces the error.
    expect(await screen.findByTestId("no-models-empty-state")).toBeInTheDocument();
    const errors = await screen.findByTestId("provider-enumeration-errors");
    expect(within(errors).getByText(/ollama-local/)).toBeInTheDocument();
  });

  it("InlineModelControl writes the SHARED store override (not its own copy)", () => {
    useConversationsStore.setState({ activeConversationId: "c-1" });
    useProvidersStore.setState({ models: [model("openai", "gpt-4o", false)] });

    render(<InlineModelControl />);
    // Open the compact dropdown, then pick a model.
    fireEvent.click(screen.getByRole("button", { name: "Select model for the next message" }));
    fireEvent.click(screen.getByRole("button", { name: /openai \/ gpt-4o/ }));

    // The SAME shared transient override the Composer send path consumes is set.
    expect(useConversationsStore.getState().pendingOverride).toEqual({
      providerId: "openai",
      model: "gpt-4o",
    });
    // The compact trigger reflects the effective selection.
    expect(screen.getByTestId("inline-model-current")).toHaveTextContent("openai / gpt-4o");
  });

  it("InlineModelControl shows a single 'No models' affordance when empty", () => {
    useConversationsStore.setState({ activeConversationId: "c-1" });
    useProvidersStore.setState({ models: [] });
    render(<InlineModelControl />);
    // Exactly one guidance node, not a stacked/blanked picker.
    expect(screen.getAllByTestId("no-models-empty-state")).toHaveLength(1);
  });

  it("EnumerationErrorModal renders no warning icon when there are no errors", () => {
    useProvidersStore.setState({ errors: [], lastError: null });
    const { container } = render(<EnumerationErrorModal />);
    // Zero errors => no icon at all.
    expect(screen.queryByRole("button", { name: "Show model load warnings" })).toBeNull();
    expect(container).toBeEmptyDOMElement();
  });

  it("EnumerationErrorModal shows a warning icon and lists errors + lastError in a modal", () => {
    useProvidersStore.setState({
      errors: [{ providerId: "ollama-local", message: "connection refused" }],
      lastError: "provider registry build failed",
    });
    render(<EnumerationErrorModal />);

    // The warning icon appears because errors exist; no dialog until clicked.
    const trigger = screen.getByRole("button", { name: "Show model load warnings" });
    expect(screen.queryByRole("dialog")).toBeNull();

    fireEvent.click(trigger);
    const dialog = screen.getByRole("dialog", { name: "Model load warnings" });
    expect(within(dialog).getByText(/ollama-local/)).toBeInTheDocument();
    expect(within(dialog).getByText(/connection refused/)).toBeInTheDocument();
    // The last load failure is surfaced in the same modal.
    expect(within(dialog).getByTestId("modal-last-error")).toHaveTextContent(
      "provider registry build failed",
    );

    // The modal is dismissible.
    fireEvent.click(within(dialog).getByRole("button", { name: "Close" }));
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("AutoRationaleTooltip reveals the rationale from an info icon on click", async () => {
    // get_route_explanation resolves a display-safe rationale for the active
    // conversation; the info icon reveals it (no always-on body text).
    invoke.mockResolvedValue({
      rationale: "cheapest local model that fits the context",
      source: "automatic",
    });

    render(<AutoRationaleTooltip conversationId="c-1" />);

    // The trigger is an accessible info icon; the rationale is hidden until click.
    const trigger = await screen.findByRole("button", { name: "Why this model?" });
    expect(screen.queryByText("cheapest local model that fits the context")).toBeNull();

    fireEvent.click(trigger);
    expect(
      await screen.findByText("cheapest local model that fits the context"),
    ).toBeInTheDocument();
  });
});
