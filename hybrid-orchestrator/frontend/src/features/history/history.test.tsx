import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";

// Mock the Tauri IPC bridge (invoke + listen) per the FEAT-001 pattern.
const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn().mockResolvedValue(() => undefined),
}));

import { useConversationsStore } from "../../state/conversations";
import { ConversationContextMenu } from "./ConversationContextMenu";
import { ConversationList } from "./ConversationList";
import { History } from "./History";
import { NewConversationButton } from "./NewConversationButton";
import { SessionSummary } from "./SessionSummary";
import type { Conversation, Message } from "../../types";

function conversation(
  id: string,
  title: string,
  overrides: Partial<Conversation> = {},
): Conversation {
  return {
    id,
    title,
    createdAt: "2024-01-01T00:00:00Z",
    updatedAt: "2024-01-01T00:00:00Z",
    personaId: null,
    conversationPref: null,
    privacyTags: [],
    enabledToolServers: [],
    ...overrides,
  };
}

function resetStore() {
  useConversationsStore.setState({
    conversations: [],
    activeConversationId: null,
    messages: [],
    pendingOverride: null,
    pendingPermissions: [],
  });
}

describe("history surface", () => {
  beforeEach(() => {
    invoke.mockReset();
    invoke.mockResolvedValue(undefined);
    resetStore();
  });

  it("ConversationList renders sorted rows and filters by title/tags", () => {
    useConversationsStore.setState({
      conversations: [
        conversation("c-1", "Alpha", { updatedAt: "2024-01-01T00:00:00Z" }),
        conversation("c-2", "Beta", {
          updatedAt: "2024-02-01T00:00:00Z",
          privacyTags: ["confidential"],
        }),
      ],
    });

    const { rerender } = render(<ConversationList query="" />);
    // Both rows present; most-recent (Beta) sorts first.
    const openButtons = screen.getAllByRole("button", { name: /Alpha|Beta/ });
    expect(openButtons[0]).toHaveTextContent("Beta");
    expect(openButtons[1]).toHaveTextContent("Alpha");

    // Filter by title.
    rerender(<ConversationList query="alp" />);
    expect(screen.getByText("Alpha")).toBeInTheDocument();
    expect(screen.queryByText("Beta")).toBeNull();

    // Filter by tag.
    rerender(<ConversationList query="confidential" />);
    expect(screen.getByText("Beta")).toBeInTheDocument();
    expect(screen.queryByText("Alpha")).toBeNull();
  });

  it("SearchBar filters the History list client-side", async () => {
    invoke.mockResolvedValue([conversation("c-1", "Alpha"), conversation("c-2", "Beta")]);
    render(<History />);
    await waitFor(() => expect(screen.getByText("Alpha")).toBeInTheDocument());

    fireEvent.change(screen.getByLabelText("Search conversations"), {
      target: { value: "beta" },
    });
    expect(screen.queryByText("Alpha")).toBeNull();
    expect(screen.getByText("Beta")).toBeInTheDocument();
  });

  it("NewConversationButton creates then resumes a conversation", async () => {
    const created = conversation("c-new", "New");
    invoke.mockImplementation((command: string) => {
      if (command === "create_conversation") return Promise.resolve(created);
      if (command === "open_conversation") {
        return Promise.resolve({ conversation: created, messages: [] });
      }
      return Promise.resolve(undefined);
    });

    render(<NewConversationButton />);
    fireEvent.click(screen.getByRole("button", { name: "New conversation" }));

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("create_conversation", { args: {} });
    });
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("open_conversation", { conversationId: "c-new" });
    });
    await waitFor(() => {
      expect(useConversationsStore.getState().activeConversationId).toBe("c-new");
    });
  });

  it("context menu rename/tag/delete call the matching commands", async () => {
    const target = conversation("c-1", "Alpha", { privacyTags: [] });
    useConversationsStore.setState({ conversations: [target] });
    invoke.mockImplementation((command: string) => {
      if (command === "rename_conversation") {
        return Promise.resolve({ ...target, title: "Renamed" });
      }
      if (command === "set_conversation_tags") {
        return Promise.resolve({ ...target, privacyTags: ["confidential"] });
      }
      return Promise.resolve(undefined);
    });

    const promptSpy = vi.spyOn(window, "prompt");
    render(<ConversationContextMenu conversation={target} />);

    // Rename.
    promptSpy.mockReturnValueOnce("Renamed");
    fireEvent.click(screen.getByRole("menuitem", { name: "Rename" }));
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("rename_conversation", {
        conversationId: "c-1",
        title: "Renamed",
      });
    });

    // Tag.
    promptSpy.mockReturnValueOnce("confidential, custom:legal");
    fireEvent.click(screen.getByRole("menuitem", { name: "Tag" }));
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("set_conversation_tags", {
        conversationId: "c-1",
        tags: ["confidential", { custom: "legal" }],
      });
    });

    // Delete.
    fireEvent.click(screen.getByRole("menuitem", { name: "Delete" }));
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("delete_conversation", { conversationId: "c-1" });
    });

    promptSpy.mockRestore();
  });

  it("context menu duplicate seeds a new conversation from the source", async () => {
    const source = conversation("c-1", "Alpha", {
      personaId: "p-1",
      privacyTags: ["confidential"],
      conversationPref: { providerId: "openai", model: "gpt-4o" },
    });
    useConversationsStore.setState({ conversations: [source] });
    const dupe = conversation("c-2", "Alpha (copy)", {
      personaId: "p-1",
      privacyTags: ["confidential"],
    });
    invoke.mockImplementation((command: string) => {
      if (command === "create_conversation") return Promise.resolve(dupe);
      if (command === "set_conversation_route") {
        return Promise.resolve({ ...dupe, conversationPref: source.conversationPref });
      }
      return Promise.resolve(undefined);
    });

    render(<ConversationContextMenu conversation={source} />);
    fireEvent.click(screen.getByRole("menuitem", { name: "Duplicate" }));

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("create_conversation", {
        args: {
          title: "Alpha (copy)",
          personaId: "p-1",
          privacyTags: ["confidential"],
        },
      });
    });
    // The source's route pin is carried over.
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("set_conversation_route", {
        conversationId: "c-2",
        route: { providerId: "openai", model: "gpt-4o" },
      });
    });
  });

  it("context menu export downloads the returned string", async () => {
    const target = conversation("c-1", "Alpha");
    invoke.mockImplementation((command: string) => {
      if (command === "export_conversation") return Promise.resolve("# Alpha\n");
      return Promise.resolve(undefined);
    });

    const createUrl = vi.fn(() => "blob:mock");
    const revokeUrl = vi.fn();
    // jsdom does not implement the blob URL helpers.
    (URL as unknown as { createObjectURL: unknown }).createObjectURL = createUrl;
    (URL as unknown as { revokeObjectURL: unknown }).revokeObjectURL = revokeUrl;
    const clickSpy = vi
      .spyOn(HTMLAnchorElement.prototype, "click")
      .mockImplementation(() => undefined);

    render(<ConversationContextMenu conversation={target} />);
    fireEvent.click(screen.getByRole("menuitem", { name: "Export Markdown" }));

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("export_conversation", {
        conversationId: "c-1",
        format: "markdown",
      });
    });
    await waitFor(() => expect(clickSpy).toHaveBeenCalled());
    expect(createUrl).toHaveBeenCalled();

    clickSpy.mockRestore();
  });

  it("refreshes the index on conversation_created / _updated / _deleted", async () => {
    // Created / updated invalidate + refetch the index.
    invoke.mockResolvedValue([conversation("c-1", "Alpha"), conversation("c-2", "Beta")]);
    useConversationsStore.getState().applyCoreEvent({
      type: "conversationCreated",
      conversationId: "c-2",
    });
    expect(invoke).toHaveBeenCalledWith("list_conversations");

    // Deleted drops the row locally.
    useConversationsStore.setState({
      conversations: [conversation("c-1", "Alpha"), conversation("c-2", "Beta")],
    });
    useConversationsStore.getState().applyCoreEvent({
      type: "conversationDeleted",
      conversationId: "c-1",
    });
    expect(useConversationsStore.getState().conversations.map((c) => c.id)).toEqual(["c-2"]);
  });

  it("resume restores persona, route pin, tags, and enabled tools into active state", async () => {
    const resumed = conversation("c-1", "Alpha", {
      personaId: "p-1",
      conversationPref: { providerId: "openai", model: "gpt-4o" },
      privacyTags: ["confidential"],
      enabledToolServers: ["srv-1"],
    });
    const messages: Message[] = [
      {
        id: "m-1",
        conversationId: "c-1",
        role: "user",
        content: { type: "text", text: "hi" },
        createdAt: "2024-01-01T00:00:00Z",
        route: null,
        usage: null,
        status: "complete",
      },
    ];
    useConversationsStore.setState({ conversations: [conversation("c-1", "Alpha")] });
    invoke.mockImplementation((command: string) => {
      if (command === "open_conversation") {
        return Promise.resolve({ conversation: resumed, messages });
      }
      return Promise.resolve(undefined);
    });

    render(<ConversationList query="" />);
    fireEvent.click(screen.getByRole("button", { name: /Alpha/ }));

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("open_conversation", { conversationId: "c-1" });
    });
    await waitFor(() => {
      const state = useConversationsStore.getState();
      expect(state.activeConversationId).toBe("c-1");
      expect(state.messages).toHaveLength(1);
      const active = state.conversations.find((c) => c.id === "c-1");
      expect(active?.personaId).toBe("p-1");
      expect(active?.conversationPref).toEqual({ providerId: "openai", model: "gpt-4o" });
      expect(active?.privacyTags).toEqual(["confidential"]);
      expect(active?.enabledToolServers).toEqual(["srv-1"]);
    });
  });

  it("SessionSummary derives models, tokens, and tags from the active session", () => {
    useConversationsStore.setState({
      activeConversationId: "c-1",
      conversations: [conversation("c-1", "Alpha", { privacyTags: ["confidential"] })],
      messages: [
        {
          id: "m-1",
          conversationId: "c-1",
          role: "assistant",
          content: { type: "text", text: "hi" },
          createdAt: "2024-01-01T00:00:00Z",
          route: { providerId: "openai", model: "gpt-4o", rationale: "r", source: "automatic" },
          usage: { promptTokens: 10, completionTokens: 5, totalTokens: 15 },
          status: "complete",
        },
      ],
    });

    render(<SessionSummary />);
    expect(screen.getByText("openai / gpt-4o")).toBeInTheDocument();
    expect(screen.getByText("15")).toBeInTheDocument();
    expect(screen.getByText("confidential")).toBeInTheDocument();
  });
});
