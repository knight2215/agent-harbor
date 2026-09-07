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
    expect(screen.getByRole("dialog")).toBeInTheDocument();
    expect(screen.getByText("read_file")).toBeInTheDocument();

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
