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

  it("permissionRequested enqueues; resolvePermission forwards the decision and dequeues", async () => {
    invoke.mockResolvedValue(true);
    useConversationsStore.getState().applyCoreEvent({
      type: "permissionRequested",
      requestId: "r-1",
      serverId: "s-1",
      toolName: "read_file",
      mode: "ask",
      rationale: "read a file",
    });
    expect(useConversationsStore.getState().pendingPermissions).toHaveLength(1);
    await useConversationsStore
      .getState()
      .resolvePermission("r-1", { allow: true, remember: false });
    // The decision is forwarded to the core to unblock the tool invocation.
    expect(invoke).toHaveBeenCalledWith("resolve_permission", {
      requestId: "r-1",
      decision: { allow: true, remember: false },
    });
    // And the prompt is dequeued from the local queue.
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

  it("sendMessage carries the current override then the store clears it", async () => {
    useConversationsStore.setState({ activeConversationId: "c-1" });
    useConversationsStore.getState().setPendingOverride({ providerId: "openai", model: "gpt-4o" });
    await useConversationsStore.getState().sendMessage("hello");
    expect(invoke).toHaveBeenCalledWith("send_message", {
      conversationId: "c-1",
      content: "hello",
      overrideRoute: { providerId: "openai", model: "gpt-4o" },
    });
    // The override applies to exactly one message.
    expect(useConversationsStore.getState().pendingOverride).toBeNull();

    // A subsequent send with no override passes null.
    await useConversationsStore.getState().sendMessage("again");
    expect(invoke).toHaveBeenLastCalledWith("send_message", {
      conversationId: "c-1",
      content: "again",
      overrideRoute: null,
    });
  });

  it("stopGeneration cancels the active conversation's turn", async () => {
    useConversationsStore.setState({ activeConversationId: "c-1" });
    await useConversationsStore.getState().stopGeneration();
    expect(invoke).toHaveBeenCalledWith("stop_generation", { conversationId: "c-1" });
  });

  it("drives the real send -> messageStarted -> delta -> complete path with no pre-seeding", async () => {
    // Exercise the PRODUCTION path where the assistant id first arrives via the
    // messageStarted event and NO message is pre-seeded (review issue #5): the
    // send path adds nothing to `messages`, the store learns the ids only from
    // the events, and the reply must still appear and accumulate.
    useConversationsStore.setState({ activeConversationId: "c-1", messages: [] });
    await useConversationsStore.getState().sendMessage("hello");
    expect(invoke).toHaveBeenCalledWith("send_message", {
      conversationId: "c-1",
      content: "hello",
      overrideRoute: null,
    });

    const apply = useConversationsStore.getState().applyCoreEvent;

    // The persisted user message is announced first (complete placeholder).
    apply({ type: "messageStarted", conversationId: "c-1", messageId: "u-1", role: "user" });
    expect(useConversationsStore.getState().messages).toHaveLength(1);
    expect(useConversationsStore.getState().messages[0]).toMatchObject({
      id: "u-1",
      role: "user",
      status: "complete",
    });

    // Then the streaming assistant placeholder, before any delta.
    apply({ type: "messageStarted", conversationId: "c-1", messageId: "a-1", role: "assistant" });
    const seeded = useConversationsStore.getState().messages;
    expect(seeded).toHaveLength(2);
    expect(seeded[1]).toMatchObject({ id: "a-1", role: "assistant", status: "streaming" });

    // Deltas now land on the seeded assistant message and accumulate.
    apply({ type: "messageDelta", conversationId: "c-1", messageId: "a-1", delta: "Hel" });
    apply({ type: "messageDelta", conversationId: "c-1", messageId: "a-1", delta: "lo" });
    apply({
      type: "messageComplete",
      conversationId: "c-1",
      messageId: "a-1",
      status: "complete",
      route: { providerId: "openai", model: "gpt-4o", rationale: "why", source: "automatic" },
      usage: { promptTokens: 1, completionTokens: 2, totalTokens: 3 },
    });

    const assistant = useConversationsStore.getState().messages[1];
    expect(assistant.content).toEqual({ type: "text", text: "Hello" });
    expect(assistant.status).toBe("complete");
    expect(assistant.route?.model).toBe("gpt-4o");
  });

  it("messageStarted for a non-active conversation is ignored", () => {
    useConversationsStore.setState({ activeConversationId: "c-1", messages: [] });
    useConversationsStore.getState().applyCoreEvent({
      type: "messageStarted",
      conversationId: "other",
      messageId: "a-1",
      role: "assistant",
    });
    expect(useConversationsStore.getState().messages).toHaveLength(0);
  });

  it("conversationUpdated refreshes the history index only, never the active messages", async () => {
    // The pipeline emits conversationUpdated after EACH message persist, so the
    // reducer must refresh the index (history recency) but must NOT refetch the
    // active conversation's messages: a mid-stream get_messages refetch would
    // resolve before the assistant row is persisted and clobber the live stream
    // (review issue #1). Assert list_conversations runs and get_messages does not.
    invoke.mockImplementation((command: string) => {
      if (command === "list_conversations") {
        return Promise.resolve([conversation("c-1"), conversation("c-2")]);
      }
      if (command === "get_messages") {
        return Promise.resolve([streamingMessage("a-1", "c-1")]);
      }
      return Promise.resolve(undefined);
    });
    useConversationsStore.setState({ activeConversationId: "c-1" });
    useConversationsStore.getState().applyCoreEvent({
      type: "conversationUpdated",
      conversationId: "c-1",
    });
    // Let any fire-and-forget work the reducer might have kicked off settle.
    await Promise.resolve();
    await Promise.resolve();
    expect(invoke).toHaveBeenCalledWith("list_conversations");
    expect(invoke).not.toHaveBeenCalledWith("get_messages", { conversationId: "c-1" });
  });

  it("a conversationUpdated mid-stream does not clobber the streaming placeholder or deltas", async () => {
    // Reproduce the exact pipeline ordering (review issue #1): the user message
    // is persisted and announced, conversationUpdated fires while only the user
    // row exists, then the assistant streams. If the event handler refetched the
    // active messages, get_messages (returning ONLY the user row) would resolve
    // mid-stream and wipe the assistant placeholder + accumulated deltas. Assert
    // the live stream survives.
    invoke.mockImplementation((command: string) => {
      if (command === "list_conversations") {
        return Promise.resolve([conversation("c-1")]);
      }
      if (command === "get_messages") {
        // The DB state at the moment the refetch would resolve: user row only.
        return Promise.resolve([
          {
            id: "u-1",
            conversationId: "c-1",
            role: "user",
            content: { type: "text", text: "hello" },
            createdAt: "2024-01-01T00:00:00Z",
            route: null,
            usage: null,
            status: "complete",
          } satisfies Message,
        ]);
      }
      return Promise.resolve(undefined);
    });
    useConversationsStore.setState({ activeConversationId: "c-1", messages: [] });
    const apply = useConversationsStore.getState().applyCoreEvent;

    apply({ type: "messageStarted", conversationId: "c-1", messageId: "u-1", role: "user" });
    // conversationUpdated after the user persist — must not schedule a refetch.
    apply({ type: "conversationUpdated", conversationId: "c-1" });
    apply({ type: "messageStarted", conversationId: "c-1", messageId: "a-1", role: "assistant" });
    apply({ type: "messageDelta", conversationId: "c-1", messageId: "a-1", delta: "Hel" });
    apply({ type: "messageDelta", conversationId: "c-1", messageId: "a-1", delta: "lo" });

    // Flush any pending microtasks so a stray refetch (if one existed) would
    // have resolved and clobbered the stream by now.
    await Promise.resolve();
    await Promise.resolve();

    const messages = useConversationsStore.getState().messages;
    expect(messages).toHaveLength(2);
    expect(messages[1]).toMatchObject({ id: "a-1", role: "assistant", status: "streaming" });
    expect(messages[1].content).toEqual({ type: "text", text: "Hello" });
    // The event handler never refetched the active conversation's messages.
    expect(invoke).not.toHaveBeenCalledWith("get_messages", { conversationId: "c-1" });
  });

  it("resumeConversation loads via open_conversation and restores the full record", async () => {
    const resumed: Conversation = {
      ...conversation("c-1"),
      personaId: "p-1",
      conversationPref: { providerId: "openai", model: "gpt-4o" },
      privacyTags: ["confidential"],
      enabledToolServers: ["srv-1"],
    };
    invoke.mockResolvedValue({
      conversation: resumed,
      messages: [streamingMessage("m-1", "c-1")],
    });
    useConversationsStore.setState({ conversations: [conversation("c-1")] });

    const returned = await useConversationsStore.getState().resumeConversation("c-1");
    expect(invoke).toHaveBeenCalledWith("open_conversation", { conversationId: "c-1" });
    expect(returned.personaId).toBe("p-1");
    const state = useConversationsStore.getState();
    expect(state.activeConversationId).toBe("c-1");
    expect(state.messages).toHaveLength(1);
    const active = state.conversations.find((c) => c.id === "c-1");
    expect(active?.conversationPref).toEqual({ providerId: "openai", model: "gpt-4o" });
    expect(active?.privacyTags).toEqual(["confidential"]);
    expect(active?.enabledToolServers).toEqual(["srv-1"]);
  });

  it("duplicateConversation seeds a new conversation from the source and carries the pin", async () => {
    const source: Conversation = {
      ...conversation("c-1", "Alpha"),
      personaId: "p-1",
      privacyTags: ["confidential"],
      conversationPref: { providerId: "openai", model: "gpt-4o" },
    };
    const created = conversation("c-2", "Alpha (copy)");
    useConversationsStore.setState({ conversations: [source] });
    invoke.mockImplementation((command: string) => {
      if (command === "create_conversation") return Promise.resolve(created);
      if (command === "set_conversation_route") {
        return Promise.resolve({ ...created, conversationPref: source.conversationPref });
      }
      return Promise.resolve(undefined);
    });

    const result = await useConversationsStore.getState().duplicateConversation("c-1");
    expect(invoke).toHaveBeenCalledWith("create_conversation", {
      args: { title: "Alpha (copy)", personaId: "p-1", privacyTags: ["confidential"] },
    });
    expect(invoke).toHaveBeenCalledWith("set_conversation_route", {
      conversationId: "c-2",
      route: { providerId: "openai", model: "gpt-4o" },
    });
    expect(result.conversationPref).toEqual({ providerId: "openai", model: "gpt-4o" });
    expect(useConversationsStore.getState().conversations.map((c) => c.id)).toContain("c-2");
  });

  it("exportConversation delegates to the export_conversation command", async () => {
    invoke.mockResolvedValue("# Alpha\n");
    const out = await useConversationsStore.getState().exportConversation("c-1", "markdown");
    expect(invoke).toHaveBeenCalledWith("export_conversation", {
      conversationId: "c-1",
      format: "markdown",
    });
    expect(out).toBe("# Alpha\n");
  });
});
