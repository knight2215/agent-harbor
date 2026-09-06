import { describe, it, expect, vi, beforeEach } from "vitest";

// Mock the Tauri IPC bridge so the store can be exercised without a live
// backend, matching the pattern in `src/ipc/commands.test.ts`.
const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
}));

import { useConversationsStore } from "./conversations";
import type { Conversation, Message } from "../types";

function conversation(id: string, title = "Chat"): Conversation {
  return {
    id,
    title,
    createdAt: "2024-01-01T00:00:00Z",
    updatedAt: "2024-01-01T00:00:00Z",
    personaId: null,
    conversationPref: null,
    privacyTags: [],
    enabledToolServers: [],
  };
}

function streamingMessage(id: string, conversationId: string): Message {
  return {
    id,
    conversationId,
    role: "assistant",
    content: { type: "text", text: "" },
    createdAt: "2024-01-01T00:00:00Z",
    route: null,
    usage: null,
    status: "streaming",
  };
}

describe("conversations store", () => {
  beforeEach(() => {
    invoke.mockReset();
    invoke.mockResolvedValue(undefined);
    useConversationsStore.setState({
      conversations: [],
      activeConversationId: null,
      messages: [],
      pendingOverride: null,
      pendingPermissions: [],
    });
  });

  it("loadConversations fetches the index from the core", async () => {
    invoke.mockResolvedValue([conversation("c-1")]);
    await useConversationsStore.getState().loadConversations();
    expect(invoke).toHaveBeenCalledWith("list_conversations");
    expect(useConversationsStore.getState().conversations).toHaveLength(1);
  });

  it("openConversation sets the active id and loads messages", async () => {
    invoke.mockResolvedValue([streamingMessage("m-1", "c-1")]);
    await useConversationsStore.getState().openConversation("c-1");
    expect(invoke).toHaveBeenCalledWith("get_messages", { conversationId: "c-1" });
    expect(useConversationsStore.getState().activeConversationId).toBe("c-1");
    expect(useConversationsStore.getState().messages).toHaveLength(1);
  });

  it("messageDelta accumulates into the streaming message", () => {
    useConversationsStore.setState({
      activeConversationId: "c-1",
      messages: [streamingMessage("m-1", "c-1")],
    });
    const apply = useConversationsStore.getState().applyCoreEvent;
    apply({ type: "messageDelta", conversationId: "c-1", messageId: "m-1", delta: "Hel" });
    apply({ type: "messageDelta", conversationId: "c-1", messageId: "m-1", delta: "lo" });
    const message = useConversationsStore.getState().messages[0];
    expect(message.content).toEqual({ type: "text", text: "Hello" });
    expect(message.status).toBe("streaming");
  });

  it("messageComplete finalizes route, usage, and status", () => {
    useConversationsStore.setState({
      activeConversationId: "c-1",
      messages: [streamingMessage("m-1", "c-1")],
    });
    useConversationsStore.getState().applyCoreEvent({
      type: "messageComplete",
      conversationId: "c-1",
      messageId: "m-1",
      status: "complete",
      route: { providerId: "openai", model: "gpt-4o", rationale: "why", source: "automatic" },
      usage: { promptTokens: 1, completionTokens: 2, totalTokens: 3 },
    });
    const message = useConversationsStore.getState().messages[0];
    expect(message.status).toBe("complete");
    expect(message.route?.providerId).toBe("openai");
    expect(message.usage?.totalTokens).toBe(3);
  });

  it("messageError marks the message as errored", () => {
    useConversationsStore.setState({
      activeConversationId: "c-1",
      messages: [streamingMessage("m-1", "c-1")],
    });
    useConversationsStore.getState().applyCoreEvent({
      type: "messageError",
      conversationId: "c-1",
      messageId: "m-1",
      message: "boom",
    });
    expect(useConversationsStore.getState().messages[0].status).toBe("error");
  });

  it("ignores deltas for a non-active conversation", () => {
    useConversationsStore.setState({
      activeConversationId: "c-1",
      messages: [streamingMessage("m-1", "c-1")],
    });
    useConversationsStore.getState().applyCoreEvent({
      type: "messageDelta",
      conversationId: "other",
      messageId: "m-1",
      delta: "x",
    });
    expect(useConversationsStore.getState().messages[0].content).toEqual({
      type: "text",
      text: "",
    });
  });

  it("conversationDeleted drops the conversation and clears active state", () => {
    useConversationsStore.setState({
      conversations: [conversation("c-1")],
      activeConversationId: "c-1",
      messages: [streamingMessage("m-1", "c-1")],
    });
    useConversationsStore.getState().applyCoreEvent({
      type: "conversationDeleted",
      conversationId: "c-1",
    });
    const state = useConversationsStore.getState();
    expect(state.conversations).toHaveLength(0);
    expect(state.activeConversationId).toBeNull();
    expect(state.messages).toHaveLength(0);
  });

  it("conversationCreated invalidates + refetches the index", () => {
    invoke.mockResolvedValue([conversation("c-1"), conversation("c-2")]);
    useConversationsStore.getState().applyCoreEvent({
      type: "conversationCreated",
      conversationId: "c-2",
    });
    expect(invoke).toHaveBeenCalledWith("list_conversations");
  });

  it("permissionRequested enqueues and resolvePermission dequeues", () => {
    useConversationsStore.getState().applyCoreEvent({
      type: "permissionRequested",
      requestId: "r-1",
      serverId: "s-1",
      toolName: "read_file",
      mode: "ask",
      rationale: "read a file",
    });
    expect(useConversationsStore.getState().pendingPermissions).toHaveLength(1);
    useConversationsStore.getState().resolvePermission("r-1");
    expect(useConversationsStore.getState().pendingPermissions).toHaveLength(0);
  });

  it("the transient per-message override is owned here and consumed once", () => {
    const store = useConversationsStore.getState();
    store.setPendingOverride({ providerId: "openai", model: "gpt-4o" });
    expect(useConversationsStore.getState().pendingOverride).toEqual({
      providerId: "openai",
      model: "gpt-4o",
    });
    const consumed = useConversationsStore.getState().consumePendingOverride();
    expect(consumed).toEqual({ providerId: "openai", model: "gpt-4o" });
    // Cleared after a single consume so it applies to exactly one message.
    expect(useConversationsStore.getState().pendingOverride).toBeNull();
  });
});
