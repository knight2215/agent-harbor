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
    case "run_web_search":
      // FEAT-004: the composer calls this when the 🌐 toggle is ON. Default to
      // no results so the plain message is sent unchanged.
      return [];
    case "get_web_search_config":
      // FEAT-004: unconfigured by default (the WebSearchSection reads this on
      // mount, and the composer path never depends on it).
      return null;
    case "clear_web_search_provider":
      return undefined;
    case "set_web_search_provider":
      return { kind: "tavily", hasApiKey: true, maxResults: 5, baseUrl: null };
    case "list_network_peers":
      // FEAT-006: no LAN peers configured by default.
      return [];
    case "add_network_peer":
      return {
        id: "network-peer-0000",
        label: "peer",
        baseUrl: "http://192.168.1.50:11435/v1",
        hasApiKey: false,
        warning: null,
      };
    case "remove_network_peer":
      return undefined;
    case "discover_network_peers":
      // FEAT-006: discovery returns empty in-sandbox (non-fatal empty path).
      return [];
    case "get_model_sharing":
      // FEAT-006: sharing OFF by default.
      return { enabled: false, port: 11435, status: "Sharing is off." };
    case "set_model_sharing":
      return { enabled: true, port: 11435, status: "Sharing on port 11435." };
    case "embedded_model_status":
      return { loadedModelId: null, registeredCount: 0 };
    case "provider_diagnostics":
      return { configuredCount: 0, totalModelCount: 0, providerCountWithModels: 0, providers: [] };
    case "test_key_storage":
      // FEAT-002: the Diagnostics section's "Test key storage" button invokes
      // this; a well-formed { ok, detail } view keeps the panel from blanking.
      return { ok: true, detail: "Keychain round-trip succeeded." };
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
    expect(within(welcome).getByRole("button", { name: "New conversation" })).toBeInTheDocument();
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

  // --- FEAT-001: single conversation on first send (Bug 1) -----------------

  it("Welcome -> New conversation -> send creates exactly ONE conversation and shows one row", async () => {
    // Bug 1 repro: starting a fresh conversation from the Welcome screen and
    // then typing the first message must create exactly ONE conversation and
    // show exactly ONE row in the sidebar list - never a duplicate "New
    // conversation". The flow drives the real create/open/reload + send
    // lifecycle: WelcomeScreen.start (create_conversation + open_conversation),
    // the MessageList mount reload (get_messages), the send (send_message), and
    // the pipeline's conversationUpdated -> loadConversations refetch that the
    // core emits after the first message persist.
    //
    // Once the conversation is created the core lists it, so the refetch
    // triggered by conversationUpdated returns the SINGLE persisted row (the
    // real backend behavior). Model this by returning the created conversation
    // from list_conversations for the remainder of the flow.
    const createdRow = {
      id: "c-new",
      title: "New conversation",
      createdAt: "2024-01-01T00:00:00Z",
      updatedAt: "2024-01-01T00:00:01Z",
      personaId: null,
      conversationPref: null,
      routingMode: null,
      privacyTags: [],
      enabledToolServers: [],
    };
    invoke.mockImplementation((command: string) => {
      if (command === "list_conversations") return Promise.resolve([createdRow]);
      return Promise.resolve(routeInvoke(command));
    });
    render(<App />);

    // Welcome is shown until a conversation is active.
    const welcome = screen.getByRole("region", { name: "Welcome" });
    fireEvent.click(within(welcome).getByRole("button", { name: "New conversation" }));

    // The chat surface appears once the created conversation is opened.
    expect(await screen.findByRole("region", { name: "Chat" })).toBeInTheDocument();

    // Type the first message and send it.
    const input = (await screen.findByLabelText("Message")) as HTMLTextAreaElement;
    fireEvent.change(input, { target: { value: "hello" } });
    fireEvent.keyDown(input, { key: "Enter" });

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("send_message", {
        conversationId: "c-new",
        content: "hello",
        overrideRoute: null,
      });
    });

    // Simulate the pipeline's post-persist refresh: the core emits
    // conversationUpdated after the first message persist, which drives the
    // store's loadConversations refetch. list_conversations returns the SINGLE
    // persisted conversation.
    const handler = listen.mock.calls[0][1] as (event: { payload: unknown }) => void;
    handler({ payload: { type: "conversationUpdated", conversationId: "c-new" } });

    // Exactly one create_conversation call for the whole flow ...
    await waitFor(() => {
      const creates = invoke.mock.calls.filter((c) => c[0] === "create_conversation");
      expect(creates).toHaveLength(1);
    });

    // ... and exactly ONE conversation row rendered (the sidebar Conversations
    // sub-list). A second "New conversation" row would be the Bug 1 regression.
    await waitFor(() => {
      const list = screen.getByRole("list", { name: "Conversations" });
      expect(within(list).getAllByRole("listitem")).toHaveLength(1);
    });
  });

  it("a seeded user message survives the MessageList mount reload (not blanked)", async () => {
    // Bug 2 lifecycle: after the user message is seeded (via messageStarted with
    // text), the MessageList mount effect must NOT re-run its get_messages
    // reload and clobber it. Here get_messages resolves to an EMPTY history (the
    // real mid-turn DB state before rows converge); the guarded reload must not
    // blank the seeded "hello" bubble. The ref guard in MessageList ensures the
    // reload runs at most once per conversation, so a second activation-driven
    // reload cannot wipe the live-seeded message.
    render(<App />);

    const welcome = screen.getByRole("region", { name: "Welcome" });
    fireEvent.click(within(welcome).getByRole("button", { name: "New conversation" }));
    expect(await screen.findByRole("region", { name: "Chat" })).toBeInTheDocument();

    // Seed the user message the way the pipeline does on send.
    const handler = listen.mock.calls[0][1] as (event: { payload: unknown }) => void;
    handler({
      payload: {
        type: "messageStarted",
        conversationId: "c-new",
        messageId: "u-1",
        role: "user",
        text: "hello",
      },
    });

    const log = await screen.findByRole("log", { name: "Conversation messages" });
    await waitFor(() => {
      expect(log.querySelector('[data-role="user"]')?.textContent).toContain("hello");
    });

    // Flush any pending get_messages microtasks so a stray reload would have
    // resolved (returning []) and blanked the bubble by now.
    await Promise.resolve();
    await Promise.resolve();
    expect(log.querySelector('[data-role="user"]')?.textContent).toContain("hello");
  });

  // --- FEAT-001: the user's own message is visible immediately (Bug 2) -----

  it("renders the user's own message text immediately on send (messageStarted carries text)", async () => {
    // Bug 2 repro: on send, the pipeline announces the persisted USER message
    // via messageStarted carrying its text. The chat surface must render that
    // text right away in a data-role="user" bubble instead of an empty bubble.
    render(<App />);

    const welcome = screen.getByRole("region", { name: "Welcome" });
    fireEvent.click(within(welcome).getByRole("button", { name: "New conversation" }));
    expect(await screen.findByRole("region", { name: "Chat" })).toBeInTheDocument();

    const input = (await screen.findByLabelText("Message")) as HTMLTextAreaElement;
    fireEvent.change(input, { target: { value: "hello there" } });
    fireEvent.keyDown(input, { key: "Enter" });
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("send_message", {
        conversationId: "c-new",
        content: "hello there",
        overrideRoute: null,
      });
    });

    // The core announces the persisted user message WITH its text.
    const handler = listen.mock.calls[0][1] as (event: { payload: unknown }) => void;
    handler({
      payload: {
        type: "messageStarted",
        conversationId: "c-new",
        messageId: "u-1",
        role: "user",
        text: "hello there",
      },
    });

    // The user's own words appear in a data-role="user" bubble immediately.
    const log = await screen.findByRole("log", { name: "Conversation messages" });
    await waitFor(() => {
      const userBubble = log.querySelector('[data-role="user"]');
      expect(userBubble).not.toBeNull();
      expect(userBubble?.textContent).toContain("hello there");
    });
  });
});
