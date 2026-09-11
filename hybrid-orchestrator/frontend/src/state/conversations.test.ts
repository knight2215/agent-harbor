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
    routingMode: null,
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
      webSearchEnabled: false,
      thinkingEnabled: false,
      attachments: [],
      pendingPermissions: [],
      sendState: "idle",
      sendError: null,
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

  it("openConversation clears a stale failed send status on the switch", async () => {
    // A prior send failed and left the global sendState/sendError set. Switching
    // to another conversation must reset them so no stale "Failed to send" alert
    // shows against the newly opened conversation.
    useConversationsStore.setState({ sendState: "failed", sendError: "provider build failed" });
    invoke.mockResolvedValue([]);
    await useConversationsStore.getState().openConversation("c-2");
    const state = useConversationsStore.getState();
    expect(state.activeConversationId).toBe("c-2");
    expect(state.sendState).toBe("idle");
    expect(state.sendError).toBeNull();
  });

  it("resumeConversation clears a stale failed send status on the switch", async () => {
    useConversationsStore.setState({ sendState: "failed", sendError: "provider build failed" });
    invoke.mockResolvedValue({
      conversation: conversation("c-2"),
      messages: [],
    });
    await useConversationsStore.getState().resumeConversation("c-2");
    const state = useConversationsStore.getState();
    expect(state.activeConversationId).toBe("c-2");
    expect(state.sendState).toBe("idle");
    expect(state.sendError).toBeNull();
  });

  it("deleteConversation clears a stale failed send status when the active one is removed", async () => {
    useConversationsStore.setState({
      conversations: [conversation("c-1")],
      activeConversationId: "c-1",
      sendState: "failed",
      sendError: "provider build failed",
    });
    await useConversationsStore.getState().deleteConversation("c-1");
    const state = useConversationsStore.getState();
    expect(state.activeConversationId).toBeNull();
    expect(state.sendState).toBe("idle");
    expect(state.sendError).toBeNull();
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

  it("messageThinkingDelta accumulates reasoning without touching the answer content", () => {
    useConversationsStore.setState({
      activeConversationId: "c-1",
      messages: [streamingMessage("m-1", "c-1")],
    });
    const apply = useConversationsStore.getState().applyCoreEvent;
    // Interleave reasoning and answer deltas: reasoning accumulates onto the
    // FRONTEND-ONLY `thinking` field; the answer accumulates onto content.
    apply({
      type: "messageThinkingDelta",
      conversationId: "c-1",
      messageId: "m-1",
      delta: "Think",
    });
    apply({ type: "messageDelta", conversationId: "c-1", messageId: "m-1", delta: "Ans" });
    apply({ type: "messageThinkingDelta", conversationId: "c-1", messageId: "m-1", delta: "ing" });
    const message = useConversationsStore.getState().messages[0];
    // The answer content carries ONLY the content deltas (reasoning never leaks
    // into the answer).
    expect(message.content).toEqual({ type: "text", text: "Ans" });
    expect(message.thinking).toBe("Thinking");
    expect(message.status).toBe("streaming");
  });

  it("messageThinkingDelta for a non-active conversation is ignored", () => {
    useConversationsStore.setState({
      activeConversationId: "c-1",
      messages: [streamingMessage("m-1", "c-1")],
    });
    useConversationsStore.getState().applyCoreEvent({
      type: "messageThinkingDelta",
      conversationId: "c-OTHER",
      messageId: "m-1",
      delta: "should be dropped",
    });
    expect(useConversationsStore.getState().messages[0].thinking).toBeUndefined();
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

  it("messageError marks the message as errored and carries the reason into an empty placeholder", () => {
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
    const message = useConversationsStore.getState().messages[0];
    expect(message.status).toBe("error");
    // The empty streaming placeholder now shows the display-safe reason.
    expect(message.content).toEqual({ type: "text", text: "boom" });
  });

  it("messageError preserves an already-streamed reply's text and only flags the error", () => {
    useConversationsStore.setState({
      activeConversationId: "c-1",
      messages: [
        { ...streamingMessage("m-1", "c-1"), content: { type: "text", text: "partial reply" } },
      ],
    });
    useConversationsStore.getState().applyCoreEvent({
      type: "messageError",
      conversationId: "c-1",
      messageId: "m-1",
      message: "finalize failed",
    });
    const message = useConversationsStore.getState().messages[0];
    expect(message.status).toBe("error");
    // A non-empty streamed reply is NOT clobbered by the error reason.
    expect(message.content).toEqual({ type: "text", text: "partial reply" });
  });

  it("messageError appends a visible error bubble when the messageId was never seeded", () => {
    useConversationsStore.setState({ activeConversationId: "c-1", messages: [] });
    useConversationsStore.getState().applyCoreEvent({
      type: "messageError",
      conversationId: "c-1",
      messageId: "never-seeded",
      message: "no route available",
    });
    const messages = useConversationsStore.getState().messages;
    expect(messages).toHaveLength(1);
    expect(messages[0]).toMatchObject({
      id: "never-seeded",
      role: "assistant",
      status: "error",
      content: { type: "text", text: "no route available" },
    });
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

  it("attachment actions add (deduped), remove, and clear (FEAT-003)", () => {
    const store = useConversationsStore.getState();
    store.addAttachment({ kind: "text", name: "a.txt", path: "/tmp/a.txt", byteLen: 5, text: "x" });
    // Re-adding the same path + kind is idempotent (deduped).
    store.addAttachment({ kind: "text", name: "a.txt", path: "/tmp/a.txt", byteLen: 5, text: "x" });
    expect(useConversationsStore.getState().attachments).toHaveLength(1);
    // A different kind on the same path is a distinct attachment.
    store.addAttachment({ kind: "repo", name: "a.txt", path: "/tmp/a.txt", byteLen: 5, text: "x" });
    expect(useConversationsStore.getState().attachments).toHaveLength(2);
    // Remove by path + kind.
    store.removeAttachment("/tmp/a.txt", "text");
    expect(useConversationsStore.getState().attachments).toHaveLength(1);
    expect(useConversationsStore.getState().attachments[0].kind).toBe("repo");
    // Clear drops everything.
    store.clearAttachments();
    expect(useConversationsStore.getState().attachments).toHaveLength(0);
  });

  it("sendMessage clears attachments on success but keeps them on failure (FEAT-003)", async () => {
    useConversationsStore.setState({
      activeConversationId: "c-1",
      attachments: [{ kind: "text", name: "a.txt", path: "/tmp/a.txt", byteLen: 5, text: "x" }],
    });
    // Successful send clears the one-turn attachments.
    await useConversationsStore.getState().sendMessage("hello");
    expect(useConversationsStore.getState().attachments).toHaveLength(0);

    // A failed send leaves them in place so the user can retry.
    useConversationsStore.setState({
      attachments: [{ kind: "text", name: "b.txt", path: "/tmp/b.txt", byteLen: 5, text: "y" }],
    });
    invoke.mockImplementation((command: string) =>
      command === "send_message" ? Promise.reject(new Error("nope")) : Promise.resolve(undefined),
    );
    await useConversationsStore.getState().sendMessage("hello");
    expect(useConversationsStore.getState().attachments).toHaveLength(1);
  });

  it("sendMessage captures a rejected send_message into sendState/sendError", async () => {
    invoke.mockImplementation((command: string) =>
      command === "send_message"
        ? Promise.reject(new Error("provider build failed"))
        : Promise.resolve(undefined),
    );
    useConversationsStore.setState({ activeConversationId: "c-1" });
    // The rejection is captured, not thrown: the Composer fires this as
    // `void sendMessage(...)`, so an escaping rejection would be a silent no-op.
    await useConversationsStore.getState().sendMessage("hello");
    const state = useConversationsStore.getState();
    expect(state.sendState).toBe("failed");
    expect(state.sendError).toBe("provider build failed");
  });

  it("sendMessage resets sendState to idle on success and clears a prior error", async () => {
    // Seed a prior failure, then a successful send must clear it.
    useConversationsStore.setState({
      activeConversationId: "c-1",
      sendState: "failed",
      sendError: "old error",
    });
    await useConversationsStore.getState().sendMessage("hello");
    const state = useConversationsStore.getState();
    expect(state.sendState).toBe("idle");
    expect(state.sendError).toBeNull();
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

    // The persisted user message is announced first (complete placeholder),
    // carrying its text so the user's own words are visible immediately (Bug 2).
    apply({
      type: "messageStarted",
      conversationId: "c-1",
      messageId: "u-1",
      role: "user",
      text: "hello",
    });
    expect(useConversationsStore.getState().messages).toHaveLength(1);
    expect(useConversationsStore.getState().messages[0]).toMatchObject({
      id: "u-1",
      role: "user",
      status: "complete",
      content: { type: "text", text: "hello" },
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

    apply({
      type: "messageStarted",
      conversationId: "c-1",
      messageId: "u-1",
      role: "user",
      text: "hello",
    });
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

  it("createConversation then send invokes create_conversation exactly once (Bug 1)", async () => {
    // Bug 1: creating a conversation and then sending a message must create
    // exactly ONE conversation. The store's createConversation upserts the
    // returned row (rather than blindly appending), so a subsequent
    // loadConversations refetch - which the pipeline triggers via
    // conversationUpdated after the first message persist - reconciles to the
    // SAME single row instead of surfacing a duplicate "New Conversation".
    const created = conversation("c-1", "New Conversation");
    invoke.mockImplementation((command: string) => {
      if (command === "create_conversation") return Promise.resolve(created);
      if (command === "list_conversations") return Promise.resolve([created]);
      // send_message resolves with no value; get_messages is unused here.
      return Promise.resolve(undefined);
    });

    const store = useConversationsStore.getState();
    await store.createConversation();
    // The pipeline's post-persist refresh (conversationUpdated) refetches the
    // index; the row already present must not be duplicated.
    store.applyCoreEvent({ type: "conversationUpdated", conversationId: "c-1" });
    await Promise.resolve();
    await Promise.resolve();
    // The active send path itself never creates a conversation.
    useConversationsStore.setState({ activeConversationId: "c-1" });
    await useConversationsStore.getState().sendMessage("hello");

    const creates = invoke.mock.calls.filter((c) => c[0] === "create_conversation");
    expect(creates).toHaveLength(1);
    // Exactly one conversation row, no duplicate.
    expect(useConversationsStore.getState().conversations).toHaveLength(1);
    expect(useConversationsStore.getState().conversations[0].id).toBe("c-1");
  });

  it("createConversation upserts rather than duplicating an already-listed row (Bug 1)", async () => {
    // If a refetch has already surfaced the conversation (e.g. a fast
    // conversationCreated/updated event), createConversation must reconcile the
    // existing row in place, not append a second one.
    const created = conversation("c-1", "New Conversation");
    useConversationsStore.setState({ conversations: [created] });
    invoke.mockResolvedValue(created);
    await useConversationsStore.getState().createConversation();
    expect(useConversationsStore.getState().conversations).toHaveLength(1);
  });

  it("messageStarted for a user message seeds its text; a later reload does not blank it (Bug 2)", async () => {
    // Bug 2: the pipeline announces the persisted USER message via messageStarted
    // carrying its text (no user streaming). The placeholder must seed that text
    // so the user's own words are visible immediately on send, and a subsequent
    // get_messages reload (openConversation) that returns the persisted user row
    // must keep the text - never blank the bubble.
    useConversationsStore.setState({ activeConversationId: "c-1", messages: [] });
    const apply = useConversationsStore.getState().applyCoreEvent;
    apply({
      type: "messageStarted",
      conversationId: "c-1",
      messageId: "u-1",
      role: "user",
      text: "hello",
    });
    const seeded = useConversationsStore.getState().messages;
    expect(seeded).toHaveLength(1);
    expect(seeded[0]).toMatchObject({
      id: "u-1",
      role: "user",
      status: "complete",
      content: { type: "text", text: "hello" },
    });

    // A reload of the conversation returns the persisted user row with its text.
    invoke.mockResolvedValue([
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
    await useConversationsStore.getState().openConversation("c-1");
    const reloaded = useConversationsStore.getState().messages;
    expect(reloaded).toHaveLength(1);
    expect(reloaded[0].content).toEqual({ type: "text", text: "hello" });
  });

  it("messageStarted for an assistant message seeds empty and streams via deltas", () => {
    // The assistant announcement carries no text; it seeds an empty streaming
    // placeholder that subsequent deltas fill.
    useConversationsStore.setState({ activeConversationId: "c-1", messages: [] });
    const apply = useConversationsStore.getState().applyCoreEvent;
    apply({ type: "messageStarted", conversationId: "c-1", messageId: "a-1", role: "assistant" });
    const seeded = useConversationsStore.getState().messages[0];
    expect(seeded).toMatchObject({ id: "a-1", role: "assistant", status: "streaming" });
    expect(seeded.content).toEqual({ type: "text", text: "" });
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

  // --- FEAT-004 regenerate / edit-resend / continue ------------------------

  it("regenerateLastTurn re-sends the prior user message content as a fresh turn", async () => {
    // The prior user turn ("what is 2+2?") precedes the last assistant reply;
    // regenerate re-invokes send_message with that user content (a fresh turn -
    // it does not mutate the prior assistant row).
    useConversationsStore.setState({
      activeConversationId: "c-1",
      messages: [
        {
          id: "u-1",
          conversationId: "c-1",
          role: "user",
          content: { type: "text", text: "what is 2+2?" },
          createdAt: "2024-01-01T00:00:00Z",
          route: null,
          usage: null,
          status: "complete",
        },
        { ...streamingMessage("a-1", "c-1"), status: "complete" },
      ] satisfies Message[],
    });
    await useConversationsStore.getState().regenerateLastTurn();
    expect(invoke).toHaveBeenCalledWith("send_message", {
      conversationId: "c-1",
      content: "what is 2+2?",
      overrideRoute: null,
    });
  });

  it("regenerateLastTurn is a no-op when there is no preceding user message", async () => {
    useConversationsStore.setState({
      activeConversationId: "c-1",
      messages: [{ ...streamingMessage("a-1", "c-1"), status: "complete" }],
    });
    await useConversationsStore.getState().regenerateLastTurn();
    expect(invoke).not.toHaveBeenCalledWith("send_message", expect.anything());
  });

  it("editAndResend re-sends the edited content of the last user message", async () => {
    useConversationsStore.setState({
      activeConversationId: "c-1",
      messages: [
        {
          id: "u-1",
          conversationId: "c-1",
          role: "user",
          content: { type: "text", text: "what is 2+2?" },
          createdAt: "2024-01-01T00:00:00Z",
          route: null,
          usage: null,
          status: "complete",
        },
        { ...streamingMessage("a-1", "c-1"), status: "complete" },
      ] satisfies Message[],
    });
    await useConversationsStore.getState().editAndResend("u-1", "what is 3+3?");
    expect(invoke).toHaveBeenCalledWith("send_message", {
      conversationId: "c-1",
      content: "what is 3+3?",
      overrideRoute: null,
    });
  });

  it("editAndResend is a no-op when the id is not the last user message (branching deferred)", async () => {
    useConversationsStore.setState({
      activeConversationId: "c-1",
      messages: [
        {
          id: "u-1",
          conversationId: "c-1",
          role: "user",
          content: { type: "text", text: "first" },
          createdAt: "2024-01-01T00:00:00Z",
          route: null,
          usage: null,
          status: "complete",
        },
        {
          id: "u-2",
          conversationId: "c-1",
          role: "user",
          content: { type: "text", text: "second" },
          createdAt: "2024-01-01T00:00:00Z",
          route: null,
          usage: null,
          status: "complete",
        },
      ] satisfies Message[],
    });
    // Editing an EARLIER user message is deferred: only the last user message is
    // editable in this scope, so this is a no-op.
    await useConversationsStore.getState().editAndResend("u-1", "rewritten first");
    expect(invoke).not.toHaveBeenCalledWith("send_message", expect.anything());
  });

  it("continueTurn sends a follow-up 'continue' turn (not a resume)", async () => {
    useConversationsStore.setState({ activeConversationId: "c-1" });
    await useConversationsStore.getState().continueTurn();
    expect(invoke).toHaveBeenCalledWith("send_message", {
      conversationId: "c-1",
      content: "Please continue.",
      overrideRoute: null,
    });
  });
});
