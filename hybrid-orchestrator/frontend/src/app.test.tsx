import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor, fireEvent, within } from "@testing-library/react";
import { App } from "./app";
import { useConversationsStore } from "./state/conversations";
import { useProvidersStore } from "./state/providers";

// Mock the Tauri IPC bridge so the test runs without a live backend. The shell
// mounts every surface, so `invoke` is routed per-command and `listen` (used by
// the single top-level `onCoreEvent` fan-out) is stubbed with a spy unlisten.
const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
}));

const unlisten = vi.fn();
const listen = vi.fn();
vi.mock("@tauri-apps/api/event", () => ({
  listen: (...args: unknown[]) => listen(...args),
}));

// The status footer reads the real app version via `@tauri-apps/api/app`
// getVersion(); mock it so the async render resolves deterministically and the
// test never hangs waiting on a live Tauri runtime.
const getVersion = vi.fn().mockResolvedValue("9.9.9-test");
vi.mock("@tauri-apps/api/app", () => ({
  getVersion: () => getVersion(),
}));

/** Route each mocked command to a shape its caller can consume. */
function routeInvoke(command: string): unknown {
  switch (command) {
    case "list_conversations":
    case "list_personas":
    case "list_mcp_servers":
      return [];
    case "list_available_models":
      return { models: [], errors: [] };
    case "list_cloud_providers":
    case "list_local_runtimes":
    case "list_embedded_models":
      return [];
    case "embedded_model_status":
      return { loadedModelId: null, registeredCount: 0 };
    case "provider_diagnostics":
      return { configuredCount: 0, totalModelCount: 0, providerCountWithModels: 0, providers: [] };
    case "get_messages":
      return [];
    case "read_text_file":
      // FEAT-003 attach: a well-formed text-file view.
      return { path: "/tmp/a.txt", name: "a.txt", byteLen: 5, text: "hello" };
    case "read_file_base64":
      // FEAT-003 attach: a well-formed image view.
      return {
        path: "/tmp/a.png",
        name: "a.png",
        mimeType: "image/png",
        base64: "Zm9v",
        byteLen: 3,
      };
    case "list_repo_files":
      // FEAT-003 repository: a well-formed (empty) listing.
      return { dir: "/tmp/repo", files: [], truncated: false };
    case "get_route_explanation":
      // The composer's "Why this model?" info icon fetches this when a
      // conversation is active; a null-rationale explanation renders no icon.
      return { providerId: null, model: null, rationale: null, source: "automatic" };
    case "create_conversation":
      return {
        id: "c-new",
        title: "New conversation",
        createdAt: "2024-01-01T00:00:00Z",
        updatedAt: "2024-01-01T00:00:00Z",
        personaId: null,
        conversationPref: null,
        routingMode: null,
        privacyTags: [],
        enabledToolServers: [],
      };
    case "open_conversation":
      return {
        conversation: {
          id: "c-new",
          title: "New conversation",
          createdAt: "2024-01-01T00:00:00Z",
          updatedAt: "2024-01-01T00:00:00Z",
          personaId: null,
          conversationPref: null,
          routingMode: null,
          privacyTags: [],
          enabledToolServers: [],
        },
        messages: [],
      };
    default:
      return undefined;
  }
}

describe("<App />", () => {
  // Capture the store's REAL load() before any test swaps in a spy, so
  // beforeEach can restore it (the store is a module-level singleton and a
  // leaked spy would otherwise persist across tests).
  const realProvidersLoad = useProvidersStore.getState().load;

  beforeEach(() => {
    invoke.mockReset();
    listen.mockReset();
    unlisten.mockReset();
    getVersion.mockClear();
    getVersion.mockResolvedValue("9.9.9-test");
    invoke.mockImplementation((command: string) => Promise.resolve(routeInvoke(command)));
    listen.mockResolvedValue(unlisten);
    try {
      window.localStorage.clear();
    } catch {
      // Ignore storage errors in constrained envs.
    }
    // Reset the shared conversations store to its real, complete default shape
    // before each test. The store is a module-level singleton, so a prior
    // test that seeds an active conversation must not bleed a partial state
    // (e.g. an activated conversation with an undefined `messages`) into a
    // sibling. Every field the shell reads is set to its real initial value.
    useConversationsStore.setState({
      conversations: [],
      activeConversationId: null,
      messages: [],
      pendingOverride: null,
      webSearchEnabled: false,
      attachments: [],
      pendingPermissions: [],
      sendState: "idle",
      sendError: null,
    });
    // The providers store is also a module-level singleton. Tests below seed
    // its `models` (and some swap in a spy `load`), so reset it to the real
    // default shape here so no partial providers state - or a leaked spy - bleeds
    // between tests. Restore the real `load` so the startup load() that the App
    // now runs on mount routes through the mocked `invoke` for tests that do not
    // seed their own spy.
    useProvidersStore.setState({
      models: [],
      errors: [],
      loadState: "idle",
      lastError: null,
      load: realProvidersLoad,
    });
  });

  it("renders the app version read at runtime via getVersion()", async () => {
    render(<App />);

    await waitFor(() => {
      expect(screen.getByTestId("app-version")).toHaveTextContent("9.9.9-test");
    });

    expect(getVersion).toHaveBeenCalled();
  });

  it("renders the sidebar with Chat / History / Settings destinations", () => {
    render(<App />);

    const nav = screen.getByRole("navigation", { name: "Primary" });
    expect(nav).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Chat/ })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /History/ })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Settings/ })).toBeInTheDocument();

    // Chat is the default destination, but with no active conversation the pane
    // shows the Welcome screen (not the chat surface).
    expect(screen.getByRole("region", { name: "Welcome" })).toBeInTheDocument();
    expect(screen.queryByRole("region", { name: "Chat" })).toBeNull();
  });

  it("renders expanded by default with visible nav labels", () => {
    render(<App />);
    // Expanded: the shell is not collapsed and the brand text label is present.
    expect(document.querySelector(".app")).toHaveAttribute("data-collapsed", "false");
    expect(screen.getByText("Agent Harbor")).toBeInTheDocument();
    // Each nav item carries a title tooltip mirroring its label.
    expect(screen.getByRole("button", { name: /Chat/ })).toHaveAttribute("title", "Chat");
  });

  it("collapses to an icon-only rail and persists the choice to localStorage", () => {
    render(<App />);

    const shell = document.querySelector(".app");
    expect(shell).toHaveAttribute("data-collapsed", "false");

    fireEvent.click(screen.getByRole("button", { name: "Collapse sidebar" }));

    expect(shell).toHaveAttribute("data-collapsed", "true");
    expect(window.localStorage.getItem("ah-sidebar-collapsed")).toBe("true");
    // The nav items and their tooltips remain (icon-only rail); labels are
    // hidden purely via CSS driven by data-collapsed.
    expect(screen.getByRole("button", { name: /Chat/ })).toHaveAttribute("title", "Chat");

    // Toggling back expands and updates the persisted value.
    fireEvent.click(screen.getByRole("button", { name: "Expand sidebar" }));
    expect(shell).toHaveAttribute("data-collapsed", "false");
    expect(window.localStorage.getItem("ah-sidebar-collapsed")).toBe("false");
  });

  it("restores the collapsed state from localStorage on mount", () => {
    window.localStorage.setItem("ah-sidebar-collapsed", "true");
    render(<App />);
    expect(document.querySelector(".app")).toHaveAttribute("data-collapsed", "true");
  });

  it("shows the Welcome screen (large logo + New conversation) by default", () => {
    render(<App />);
    const welcome = screen.getByRole("region", { name: "Welcome" });
    expect(welcome).toBeInTheDocument();
    // The large logo carries its stable testid.
    expect(screen.getByTestId("welcome-logo")).toBeInTheDocument();
    // A prominent primary "New conversation" button lives on the welcome screen
    // (plus the one nested under the Chat nav item in the sidebar).
    expect(
      within(welcome).getByRole("button", { name: "New conversation" }),
    ).toBeInTheDocument();
    // The chat surface is NOT rendered until a conversation is active.
    expect(screen.queryByRole("region", { name: "Chat" })).toBeNull();
  });

  it("renders the chat surface only after a conversation is opened", async () => {
    render(<App />);
    // Welcome first.
    expect(screen.getByRole("region", { name: "Welcome" })).toBeInTheDocument();

    // Opening a conversation flips the active id, so the chat surface appears.
    useConversationsStore.setState({
      conversations: [
        {
          id: "c-1",
          title: "Chat",
          createdAt: "2024-01-01T00:00:00Z",
          updatedAt: "2024-01-01T00:00:00Z",
          personaId: null,
          conversationPref: null,
          routingMode: null,
          privacyTags: [],
          enabledToolServers: [],
        },
      ],
      activeConversationId: "c-1",
      messages: [],
    });

    expect(await screen.findByRole("region", { name: "Chat" })).toBeInTheDocument();
    expect(screen.queryByRole("region", { name: "Welcome" })).toBeNull();
  });

  it("nests the Conversations sub-list under the Chat nav item", () => {
    render(<App />);
    // The Chat group's expanded body carries the conversation sub-list + its own
    // New conversation action (reusing the history components).
    const sub = screen.getByTestId("chat-conversations");
    expect(sub).toBeInTheDocument();
    expect(within(sub).getByText("Conversations")).toBeInTheDocument();
    expect(within(sub).getByRole("button", { name: "New conversation" })).toBeInTheDocument();
  });

  it("stacks only ONE no-models empty state with an active Manual conversation and no models", async () => {
    // Reproduces the residual-stacking path: with an ACTIVE conversation in
    // Manual mode and zero models, both RoutingModeToggle (its manual-pin
    // picker) and PerMessageOverrideControl could each render a
    // NoModelsEmptyState. Seed that exact state, then assert only one guidance
    // block surfaces in the chat pane.
    useConversationsStore.setState({
      conversations: [
        {
          id: "c-1",
          title: "Chat",
          createdAt: "2024-01-01T00:00:00Z",
          updatedAt: "2024-01-01T00:00:00Z",
          personaId: null,
          conversationPref: null,
          routingMode: "manual",
          privacyTags: [],
          enabledToolServers: [],
        },
      ],
      activeConversationId: "c-1",
      messages: [],
      pendingOverride: null,
      webSearchEnabled: false,
      attachments: [],
      pendingPermissions: [],
      sendState: "idle",
      sendError: null,
    });
    useProvidersStore.setState({ models: [] });

    render(<App />);

    // The store was mutated outside act() before this read, so wait for the
    // async shell render to settle before asserting on the DOM.
    await waitFor(() => {
      expect(screen.getByTestId("app-version")).toHaveTextContent("9.9.9-test");
    });

    // The chat surface renders because a conversation is active. The composer's
    // inline model control shows the single "No models" guidance; the routing
    // toggle suppresses its own manual-pin picker when models are empty, so
    // exactly ONE NoModelsEmptyState surfaces in the whole chat pane.
    expect(await screen.findByRole("region", { name: "Chat" })).toBeInTheDocument();
    expect(screen.getAllByTestId("no-models-empty-state")).toHaveLength(1);

    // Reset the shared stores so the seeded conversation does not bleed into
    // sibling tests in this file.
    useConversationsStore.setState({
      conversations: [],
      activeConversationId: null,
      messages: [],
      pendingOverride: null,
      webSearchEnabled: false,
      attachments: [],
      pendingPermissions: [],
      sendState: "idle",
      sendError: null,
    });
    useProvidersStore.setState({ models: [] });
  });

  it("switches to the History and Settings destinations", async () => {
    render(<App />);

    fireEvent.click(screen.getByRole("button", { name: /History/ }));
    // The History surface region appears in the main pane.
    expect(await screen.findByRole("region", { name: "Conversation history" })).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: /Settings/ }));
    // The Settings area exposes its sub-navigation.
    expect(
      await screen.findByRole("navigation", { name: "Settings sections" }),
    ).toBeInTheDocument();
  });

  it("loads the providers store once at startup so the chat picker is populated on launch", async () => {
    // Bug A regression: the chat model picker must be populated on launch
    // WITHOUT the user first opening Settings. The single root effect calls the
    // providers store's load() once at mount; seed a spy and assert it fired.
    const load = vi.fn();
    useProvidersStore.setState({ load });

    render(<App />);

    await waitFor(() => expect(load).toHaveBeenCalled());
  });

  it("opens a SINGLE core-event subscription that survives navigation between views", async () => {
    const load = vi.fn();
    useProvidersStore.setState({ load });

    render(<App />);

    // Exactly one subscription is opened for the whole shell (the fan-out).
    expect(listen).toHaveBeenCalledTimes(1);

    // The root effect also loads the providers store once at startup (bug A),
    // so the seeded spy is already called once from mount.
    expect(load).toHaveBeenCalledTimes(1);

    // Switching between destinations must NOT open a second subscription or
    // tear down the existing one.
    fireEvent.click(screen.getByRole("button", { name: /History/ }));
    fireEvent.click(screen.getByRole("button", { name: /Settings/ }));
    fireEvent.click(screen.getByRole("button", { name: /Chat/ }));
    expect(listen).toHaveBeenCalledTimes(1);
    expect(unlisten).not.toHaveBeenCalled();

    // Beyond the startup load, opening Settings above mounted ProviderKeysSection
    // (the default "Providers & Keys" section), whose own mount effect calls the
    // providers store's load() once. That mount-load is orthogonal to the event
    // fan-out under test, so capture the current count right before dispatching
    // the event rather than hardcoding a total; the test then asserts purely
    // that the providersChanged event drives EXACTLY ONE additional refetch.
    const before = load.mock.calls.length;

    // The registered handler dispatches to every store; a providersChanged
    // event drives the providers store to refetch its models (one more load
    // beyond whatever startup + navigation already triggered).
    const handler = listen.mock.calls[0][1] as (event: { payload: unknown }) => void;
    handler({ payload: { type: "providersChanged" } });
    expect(load).toHaveBeenCalledTimes(before + 1);
  });

  it("tears down the subscription on unmount", async () => {
    const { unmount } = render(<App />);
    // Let the listen() promise resolve so the unlisten fn is captured.
    await waitFor(() => expect(listen).toHaveBeenCalled());
    unmount();
    await waitFor(() => expect(unlisten).toHaveBeenCalled());
  });
});
