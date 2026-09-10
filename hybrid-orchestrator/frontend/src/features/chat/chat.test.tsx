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
import { useProvidersStore } from "../../state/providers";
import { Composer } from "./Composer";
import { MessageBubble } from "./MessageBubble";
import { MessageList } from "./MessageList";
import { PermissionPrompt } from "./PermissionPrompt";
import type { AvailableModel, Message } from "../../types";

function textMessage(id: string, text: string): Message {
  return {
    id,
    conversationId: "c-1",
    role: "assistant",
    content: { type: "text", text },
    createdAt: "2024-01-01T00:00:00Z",
    route: { providerId: "openai", model: "gpt-4o", rationale: "cheapest", source: "automatic" },
    usage: null,
    status: "complete",
  };
}

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

function resetStores() {
  useConversationsStore.setState({
    conversations: [],
    activeConversationId: null,
    messages: [],
    pendingOverride: null,
    pendingPermissions: [],
    sendState: "idle",
    sendError: null,
  });
  useProvidersStore.setState({ models: [] });
}

describe("chat surface", () => {
  beforeEach(() => {
    invoke.mockReset();
    invoke.mockResolvedValue(undefined);
    resetStores();
  });

  it("MessageBubble renders every MessageContent variant", () => {
    const { rerender } = render(<MessageBubble message={textMessage("m-1", "Hello world")} />);
    expect(screen.getByText("Hello world")).toBeInTheDocument();
    // The RouteBadge shows the message's route metadata.
    expect(screen.getByText("openai / gpt-4o")).toBeInTheDocument();

    rerender(
      <MessageBubble
        message={{
          ...textMessage("m-2", ""),
          content: {
            type: "toolCalls",
            calls: [{ id: "t1", name: "read_file", arguments: { path: "/x" } }],
          },
        }}
      />,
    );
    expect(screen.getByText("read_file")).toBeInTheDocument();
    expect(screen.getByText(/"path": "\/x"/)).toBeInTheDocument();

    rerender(
      <MessageBubble
        message={{
          ...textMessage("m-3", ""),
          content: {
            type: "toolResults",
            results: [{ callId: "t1", content: "done", isError: false }],
          },
        }}
      />,
    );
    expect(screen.getByText("ok")).toBeInTheDocument();
    expect(screen.getByText("done")).toBeInTheDocument();

    rerender(
      <MessageBubble
        message={{
          ...textMessage("m-4", ""),
          content: {
            type: "attachments",
            attachments: [{ mimeType: "image/png", name: "shot.png", uri: "file://shot.png" }],
          },
        }}
      />,
    );
    expect(screen.getByText("shot.png")).toBeInTheDocument();
    expect(screen.getByText("image/png")).toBeInTheDocument();
  });

  it("MessageList renders a threaded view with role-styled user and assistant bubbles", async () => {
    const userMsg: Message = { ...textMessage("m-user", "hello"), role: "user", route: null };
    const assistantMsg = textMessage("m-asst", "hi there");
    invoke.mockResolvedValue([userMsg, assistantMsg]);
    useConversationsStore.setState({ activeConversationId: "c-1" });
    render(<MessageList />);

    // Both turns render inside the role=log thread with their data-role tags.
    const log = await screen.findByRole("log", { name: "Conversation messages" });
    await waitFor(() => {
      expect(log.querySelector('[data-role="user"]')).not.toBeNull();
    });
    expect(log.querySelector('[data-role="assistant"]')).not.toBeNull();
    expect(screen.getByText("hello")).toBeInTheDocument();
    expect(screen.getByText("hi there")).toBeInTheDocument();
  });

  it("MessageList streams via delta then finalizes on complete, toggling StreamingIndicator", async () => {
    invoke.mockResolvedValue([{ ...textMessage("m-1", ""), status: "streaming", route: null }]);
    useConversationsStore.setState({ activeConversationId: "c-1" });
    render(<MessageList />);

    // Loaded the message history for the active conversation.
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("get_messages", { conversationId: "c-1" });
    });
    // Streaming -> indicator visible.
    await waitFor(() => {
      expect(screen.getByRole("status", { name: "Assistant is responding" })).toBeInTheDocument();
    });

    const apply = useConversationsStore.getState().applyCoreEvent;
    apply({ type: "messageDelta", conversationId: "c-1", messageId: "m-1", delta: "Hi there" });
    await waitFor(() => expect(screen.getByText("Hi there")).toBeInTheDocument());

    apply({
      type: "messageComplete",
      conversationId: "c-1",
      messageId: "m-1",
      status: "complete",
      route: { providerId: "openai", model: "gpt-4o", rationale: "auto", source: "automatic" },
      usage: null,
    });
    // Complete -> indicator gone.
    await waitFor(() => {
      expect(screen.queryByRole("status", { name: "Assistant is responding" })).toBeNull();
    });
  });

  it("MessageList surfaces a message_error", async () => {
    invoke.mockResolvedValue([{ ...textMessage("m-1", "partial"), status: "streaming" }]);
    useConversationsStore.setState({ activeConversationId: "c-1" });
    render(<MessageList />);
    await waitFor(() => expect(screen.getByText("partial")).toBeInTheDocument());

    useConversationsStore.getState().applyCoreEvent({
      type: "messageError",
      conversationId: "c-1",
      messageId: "m-1",
      message: "boom",
    });
    await waitFor(() => expect(screen.getByRole("alert")).toBeInTheDocument());
  });

  it("Composer send carries the current override then the store clears it", async () => {
    useConversationsStore.setState({ activeConversationId: "c-1" });
    useProvidersStore.setState({ models: [model("openai", "gpt-4o", false)] });
    useConversationsStore.getState().setPendingOverride({ providerId: "openai", model: "gpt-4o" });

    render(<Composer />);
    fireEvent.change(screen.getByLabelText("Message"), { target: { value: "hello" } });
    fireEvent.click(screen.getByRole("button", { name: "Send" }));

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("send_message", {
        conversationId: "c-1",
        content: "hello",
        overrideRoute: { providerId: "openai", model: "gpt-4o" },
      });
    });
    // Applied to exactly one message.
    expect(useConversationsStore.getState().pendingOverride).toBeNull();
  });

  it("Composer sends on plain Enter and clears the draft", async () => {
    useConversationsStore.setState({ activeConversationId: "c-1" });
    render(<Composer />);
    const input = screen.getByLabelText("Message") as HTMLTextAreaElement;
    fireEvent.change(input, { target: { value: "hello" } });
    fireEvent.keyDown(input, { key: "Enter" });

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("send_message", {
        conversationId: "c-1",
        content: "hello",
        overrideRoute: null,
      });
    });
    // The draft is cleared after a successful send.
    expect(input.value).toBe("");
  });

  it("Composer does NOT send on Shift+Enter and preserves the draft", () => {
    useConversationsStore.setState({ activeConversationId: "c-1" });
    render(<Composer />);
    const input = screen.getByLabelText("Message") as HTMLTextAreaElement;
    fireEvent.change(input, { target: { value: "line one" } });
    fireEvent.keyDown(input, { key: "Enter", shiftKey: true });

    // Shift+Enter inserts a newline (default) and never sends.
    expect(invoke).not.toHaveBeenCalledWith(
      "send_message",
      expect.objectContaining({ content: "line one" }),
    );
    expect(input.value).toBe("line one");
  });

  it("Composer does NOT send while an IME composition is active", () => {
    useConversationsStore.setState({ activeConversationId: "c-1" });
    render(<Composer />);
    const input = screen.getByLabelText("Message") as HTMLTextAreaElement;
    fireEvent.change(input, { target: { value: "こんにちは" } });

    // A candidate-commit Enter reports isComposing (modern) ...
    fireEvent.keyDown(input, { key: "Enter", isComposing: true });
    // ... or the legacy keyCode 229 on older IME stacks.
    fireEvent.keyDown(input, { key: "Enter", keyCode: 229 });

    expect(invoke).not.toHaveBeenCalledWith(
      "send_message",
      expect.objectContaining({ content: "こんにちは" }),
    );
    // The draft is untouched (no send, no clear).
    expect(input.value).toBe("こんにちは");
  });

  it("Composer surfaces a visible failed-to-send affordance when send_message rejects", async () => {
    invoke.mockImplementation((command: string) =>
      command === "send_message"
        ? Promise.reject(new Error("provider build failed"))
        : Promise.resolve(undefined),
    );
    useConversationsStore.setState({ activeConversationId: "c-1" });
    render(<Composer />);
    const input = screen.getByLabelText("Message") as HTMLTextAreaElement;
    fireEvent.change(input, { target: { value: "hello" } });
    fireEvent.keyDown(input, { key: "Enter" });

    // A rejected send emits NO CoreEvents, so this role=alert affordance is the
    // only visible signal that the send failed.
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("Failed to send: provider build failed");
    await waitFor(() => {
      expect(useConversationsStore.getState().sendState).toBe("failed");
    });
    expect(useConversationsStore.getState().sendError).toBe("provider build failed");
  });

  it("MessageBubble renders the stored error reason, not the generic string", () => {
    render(
      <MessageBubble
        message={{
          ...textMessage("m-err", "model unavailable: qwen3"),
          status: "error",
        }}
      />,
    );
    const alert = screen.getByRole("alert");
    expect(alert).toHaveTextContent("model unavailable: qwen3");
    expect(screen.queryByText("This message failed to generate.")).toBeNull();
  });

  it("MessageBubble falls back to the generic string when the error has no text", () => {
    render(<MessageBubble message={{ ...textMessage("m-err2", ""), status: "error" }} />);
    expect(screen.getByRole("alert")).toHaveTextContent("This message failed to generate.");
  });

  it("MessageList appends a visible error bubble for a messageError with no prior placeholder", async () => {
    invoke.mockResolvedValue([]);
    useConversationsStore.setState({ activeConversationId: "c-1", messages: [] });
    render(<MessageList />);
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("get_messages", { conversationId: "c-1" });
    });

    useConversationsStore.getState().applyCoreEvent({
      type: "messageError",
      conversationId: "c-1",
      messageId: "never-seeded",
      message: "no route available",
    });
    // The reason is never dropped: a brand-new error bubble appears.
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("no route available");
  });

  it("Composer stop button is disabled unless a turn is streaming", () => {
    // Review issue #3: the stop control is only live while a message streams,
    // so a dead no-op control is never presented as working.
    useConversationsStore.setState({ activeConversationId: "c-1", messages: [] });
    const { rerender } = render(<Composer />);
    expect(screen.getByRole("button", { name: "Stop" })).toBeDisabled();

    useConversationsStore.setState({
      messages: [{ ...textMessage("m-1", ""), status: "streaming" }],
    });
    rerender(<Composer />);
    expect(screen.getByRole("button", { name: "Stop" })).toBeEnabled();
  });

  it("Composer stop button calls stop_generation while streaming", async () => {
    useConversationsStore.setState({
      activeConversationId: "c-1",
      messages: [{ ...textMessage("m-1", ""), status: "streaming" }],
    });
    render(<Composer />);
    fireEvent.click(screen.getByRole("button", { name: "Stop" }));
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("stop_generation", { conversationId: "c-1" });
    });
  });

  it("PermissionPrompt renders on permission_requested and resolves via resolvePermission", async () => {
    render(<PermissionPrompt />);
    // Empty queue -> nothing rendered.
    expect(screen.queryByRole("dialog")).toBeNull();

    useConversationsStore.getState().applyCoreEvent({
      type: "permissionRequested",
      requestId: "r-1",
      serverId: "s-1",
      toolName: "read_file",
      mode: "ask",
      rationale: "read a file",
    });
    // The dialog now shows the queued request.
    expect(await screen.findByRole("dialog")).toBeInTheDocument();
    expect(await screen.findByText("read_file")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Allow" }));
    // The decision is forwarded to the core, then the prompt is dequeued.
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("resolve_permission", {
        requestId: "r-1",
        decision: { allow: true, remember: false },
      });
    });
    await waitFor(() => {
      expect(useConversationsStore.getState().pendingPermissions).toHaveLength(0);
    });
  });
});
