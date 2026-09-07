import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor, fireEvent } from "@testing-library/react";
import { App } from "./app";

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
    case "list_available_models":
      return [];
    default:
      return undefined;
  }
}

describe("<App />", () => {
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

    // Chat is the default destination.
    expect(screen.getByRole("region", { name: "Chat" })).toBeInTheDocument();
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

  it("renders the 'Override next message' control exactly once in the chat pane", () => {
    render(<App />);
    expect(screen.getAllByText("Override next message")).toHaveLength(1);
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

  it("opens a SINGLE core-event subscription that survives navigation between views", async () => {
    const { useProvidersStore } = await import("./state/providers");
    const load = vi.fn();
    useProvidersStore.setState({ load });

    render(<App />);

    // Exactly one subscription is opened for the whole shell (the fan-out).
    expect(listen).toHaveBeenCalledTimes(1);

    // Switching between destinations must NOT open a second subscription or
    // tear down the existing one.
    fireEvent.click(screen.getByRole("button", { name: /History/ }));
    fireEvent.click(screen.getByRole("button", { name: /Settings/ }));
    fireEvent.click(screen.getByRole("button", { name: /Chat/ }));
    expect(listen).toHaveBeenCalledTimes(1);
    expect(unlisten).not.toHaveBeenCalled();

    // The registered handler dispatches to every store; a providersChanged
    // event drives the providers store to refetch its models.
    const handler = listen.mock.calls[0][1] as (event: { payload: unknown }) => void;
    handler({ payload: { type: "providersChanged" } });
    expect(load).toHaveBeenCalled();
  });

  it("tears down the subscription on unmount", async () => {
    const { unmount } = render(<App />);
    // Let the listen() promise resolve so the unlisten fn is captured.
    await waitFor(() => expect(listen).toHaveBeenCalled());
    unmount();
    await waitFor(() => expect(unlisten).toHaveBeenCalled());
  });
});
