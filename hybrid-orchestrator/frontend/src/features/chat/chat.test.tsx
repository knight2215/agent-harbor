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

// The Composer's Attach / Repository controls open the native dialog via
// `@tauri-apps/plugin-dialog`; mock `open()` so tests drive it without a live
// Tauri runtime (the settings.test.tsx pattern). Each test overrides the return
// value for the path(s) or folder it exercises.
const dialogOpen = vi.fn();
vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: (...args: unknown[]) => dialogOpen(...args),
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

/** A model with the `vision` capability, for the image-attach vision gate. */
function visionModel(providerId: string, id: string): AvailableModel {
  const base = model(providerId, id, false);
  return { ...base, capabilities: { ...base.capabilities, vision: true } };
}

// The providers store is a module-level singleton; capture its REAL load() once
// before any test swaps in a spy so resetStores() can restore it (a leaked spy
// would otherwise persist across tests, since the Composer now calls load()).
const realProvidersLoad = useProvidersStore.getState().load;

function resetStores() {
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
  useProvidersStore.setState({
    models: [],
    errors: [],
    loadState: "idle",
    lastError: null,
    load: realProvidersLoad,
  });
}

describe("chat surface", () => {
  beforeEach(() => {
    invoke.mockReset();
    invoke.mockResolvedValue(undefined);
    dialogOpen.mockReset();
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

  it("renders the user's own 'hello' in a data-role='user' bubble after send (Bug 2)", async () => {
    // Bug 2: the user's message text is announced via messageStarted{role:user,
    // text} and must render in the user bubble immediately - not as an empty
    // bubble. get_messages returns [] (the mid-turn DB state) so the ONLY source
    // of the text is the seeded placeholder; the reload must not blank it.
    invoke.mockResolvedValue([]);
    useConversationsStore.setState({ activeConversationId: "c-1", messages: [] });
    render(<MessageList />);
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("get_messages", { conversationId: "c-1" });
    });

    useConversationsStore.getState().applyCoreEvent({
      type: "messageStarted",
      conversationId: "c-1",
      messageId: "u-1",
      role: "user",
      text: "hello",
    });

    const log = await screen.findByRole("log", { name: "Conversation messages" });
    await waitFor(() => {
      const userBubble = log.querySelector('[data-role="user"]');
      expect(userBubble).not.toBeNull();
      expect(userBubble?.textContent).toContain("hello");
    });
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

  it("MessageBubble keeps a non-text errored message's content visible alongside the alert", () => {
    // A tool-call message that errors must still show its actual content (the
    // tool name) rather than hiding it behind the generic string, and it also
    // renders exactly ONE role=alert announcing the failure.
    render(
      <MessageBubble
        message={{
          ...textMessage("m-err3", ""),
          status: "error",
          content: {
            type: "toolCalls",
            calls: [{ id: "t1", name: "read_file", arguments: { path: "/x" } }],
          },
        }}
      />,
    );
    // The real content is preserved (ContentBody rendered).
    expect(screen.getByText("read_file")).toBeInTheDocument();
    // And a single alert announces the failure (getByRole throws on multiples).
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

  it("Composer exposes the Attach / Repository / Web-search icon affordances", () => {
    useConversationsStore.setState({ activeConversationId: "c-1" });
    render(<Composer />);
    expect(screen.getByRole("button", { name: "Attach file" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Add repository context" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Toggle web search" })).toBeInTheDocument();
    // A small Refresh-models icon lives in the composer control row.
    expect(screen.getByRole("button", { name: "Refresh models" })).toBeInTheDocument();
  });

  it("Composer web-search toggle flips the shared store flag (aria-pressed)", () => {
    useConversationsStore.setState({ activeConversationId: "c-1", webSearchEnabled: false });
    render(<Composer />);
    const toggle = screen.getByRole("button", { name: "Toggle web search" });
    // Off by default.
    expect(toggle).toHaveAttribute("aria-pressed", "false");
    expect(useConversationsStore.getState().webSearchEnabled).toBe(false);

    fireEvent.click(toggle);
    // The real on/off flag in the conversations store is flipped (FEAT-004
    // consumes it); aria-pressed reflects the new state.
    expect(useConversationsStore.getState().webSearchEnabled).toBe(true);
    expect(toggle).toHaveAttribute("aria-pressed", "true");
  });

  // --- FEAT-004 web search: inject-on-success / notice-and-still-send -------

  it("Composer with the web-search toggle ON injects results as context before sending", async () => {
    useConversationsStore.setState({ activeConversationId: "c-1", webSearchEnabled: true });
    invoke.mockImplementation((command: string) => {
      if (command === "run_web_search") {
        return Promise.resolve([
          { title: "Async Rust", url: "https://ex.com/async", snippet: "a guide" },
        ]);
      }
      return Promise.resolve(undefined);
    });
    render(<Composer />);

    const input = screen.getByLabelText("Message") as HTMLTextAreaElement;
    fireEvent.change(input, { target: { value: "how does async work" } });
    fireEvent.keyDown(input, { key: "Enter" });

    // The search runs for the raw draft ...
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("run_web_search", { query: "how does async work" });
    });
    // ... and the results are prepended as a delimited context block before the
    // user's message, which is preserved in full.
    await waitFor(() => {
      const call = invoke.mock.calls.find((c) => c[0] === "send_message");
      expect(call).toBeTruthy();
      const content = (call?.[1] as { content: string }).content;
      expect(content).toContain("Web search results");
      expect(content).toContain("Async Rust");
      expect(content).toContain("https://ex.com/async");
      expect(content).toContain("how does async work");
    });
    // No failure notice is shown on the success path.
    expect(screen.queryByTestId("composer-notice")).toBeNull();
  });

  it("Composer shows a non-fatal notice and STILL sends when web search fails/unconfigured", async () => {
    useConversationsStore.setState({ activeConversationId: "c-1", webSearchEnabled: true });
    invoke.mockImplementation((command: string) =>
      command === "run_web_search"
        ? Promise.reject(new Error("web search is not configured"))
        : Promise.resolve(undefined),
    );
    render(<Composer />);

    const input = screen.getByLabelText("Message") as HTMLTextAreaElement;
    fireEvent.change(input, { target: { value: "latest rust news" } });
    fireEvent.keyDown(input, { key: "Enter" });

    // A VISIBLE non-fatal notice (role=status) explains the degradation.
    const notice = await screen.findByTestId("composer-notice");
    expect(notice).toHaveAttribute("role", "status");
    expect(notice).toHaveTextContent(/Web search unavailable/i);
    expect(notice).toHaveTextContent(/Sending without web results/i);

    // The PLAIN message is still sent (no web results block, never dropped).
    await waitFor(() => {
      const call = invoke.mock.calls.find((c) => c[0] === "send_message");
      expect(call).toBeTruthy();
      const content = (call?.[1] as { content: string }).content;
      expect(content).toBe("latest rust news");
      expect(content).not.toContain("Web search results");
    });
  });

  it("Composer Refresh models icon re-enumerates via the providers store load()", () => {
    useConversationsStore.setState({ activeConversationId: "c-1" });
    const load = vi.fn();
    useProvidersStore.setState({ load });
    render(<Composer />);
    fireEvent.click(screen.getByRole("button", { name: "Refresh models" }));
    expect(load).toHaveBeenCalled();
    // The next beforeEach -> resetStores() restores the real load(), so the
    // seeded spy does not leak into sibling tests.
  });

  // --- FEAT-003 attach / repository context --------------------------------

  it("Composer attaches a text file (chip) and prepends its content on send", async () => {
    useConversationsStore.setState({ activeConversationId: "c-1" });
    dialogOpen.mockResolvedValue("/tmp/notes.txt");
    invoke.mockImplementation((command: string) => {
      if (command === "read_text_file") {
        return Promise.resolve({
          path: "/tmp/notes.txt",
          name: "notes.txt",
          byteLen: 11,
          text: "hello world",
        });
      }
      return Promise.resolve(undefined);
    });
    render(<Composer />);

    fireEvent.click(screen.getByRole("button", { name: "Attach file" }));
    // A removable chip appears once the read resolves.
    await screen.findByRole("button", { name: "Remove attachment notes.txt" });
    await waitFor(() => {
      expect(useConversationsStore.getState().attachments).toHaveLength(1);
    });

    // Sending prepends the file's content (fenced block) to the trimmed draft.
    const input = screen.getByLabelText("Message") as HTMLTextAreaElement;
    fireEvent.change(input, { target: { value: "summarize this" } });
    fireEvent.keyDown(input, { key: "Enter" });
    await waitFor(() => {
      const call = invoke.mock.calls.find((c) => c[0] === "send_message");
      expect(call).toBeTruthy();
      const content = (call?.[1] as { content: string }).content;
      expect(content).toContain("### Attached file: notes.txt");
      expect(content).toContain("hello world");
      expect(content).toContain("summarize this");
    });
  });

  it("Composer attaches an image when the selected model supports vision", async () => {
    useConversationsStore.setState({ activeConversationId: "c-1" });
    useProvidersStore.setState({ models: [visionModel("openai", "gpt-4o")] });
    dialogOpen.mockResolvedValue("/tmp/pic.png");
    invoke.mockImplementation((command: string) => {
      if (command === "read_file_base64") {
        return Promise.resolve({
          path: "/tmp/pic.png",
          name: "pic.png",
          mimeType: "image/png",
          base64: "Zm9v",
          byteLen: 3,
        });
      }
      return Promise.resolve(undefined);
    });
    render(<Composer />);

    fireEvent.click(screen.getByRole("button", { name: "Attach file" }));
    await screen.findByRole("button", { name: "Remove attachment pic.png" });
    await waitFor(() => {
      const atts = useConversationsStore.getState().attachments;
      expect(atts).toHaveLength(1);
      expect(atts[0].kind).toBe("image");
    });
  });

  it("Composer shows a notice and does NOT attach an image for a non-vision model", async () => {
    useConversationsStore.setState({ activeConversationId: "c-1" });
    // A model WITHOUT vision (the default `model()` helper sets vision: false).
    useProvidersStore.setState({ models: [model("openai", "gpt-3.5", false)] });
    dialogOpen.mockResolvedValue("/tmp/pic.png");
    const readSpy = vi.fn();
    invoke.mockImplementation((command: string) => {
      if (command === "read_file_base64") {
        readSpy();
      }
      return Promise.resolve(undefined);
    });
    render(<Composer />);

    fireEvent.click(screen.getByRole("button", { name: "Attach file" }));
    // The visible non-fatal notice appears and no image is attached.
    const notice = await screen.findByTestId("composer-notice");
    expect(notice).toHaveTextContent("This model doesn't support images");
    expect(useConversationsStore.getState().attachments).toHaveLength(0);
    // The image read command is never even called for a non-vision model.
    expect(readSpy).not.toHaveBeenCalled();
  });

  it("Composer removes a chip when its remove button is clicked", async () => {
    useConversationsStore.setState({
      activeConversationId: "c-1",
      attachments: [{ kind: "text", name: "a.txt", path: "/tmp/a.txt", byteLen: 5, text: "hello" }],
    });
    render(<Composer />);
    const remove = screen.getByRole("button", { name: "Remove attachment a.txt" });
    fireEvent.click(remove);
    await waitFor(() => {
      expect(useConversationsStore.getState().attachments).toHaveLength(0);
    });
  });

  it("Composer clears attachments after a successful send", async () => {
    useConversationsStore.setState({
      activeConversationId: "c-1",
      attachments: [{ kind: "text", name: "a.txt", path: "/tmp/a.txt", byteLen: 5, text: "hello" }],
    });
    invoke.mockResolvedValue(undefined);
    render(<Composer />);
    const input = screen.getByLabelText("Message") as HTMLTextAreaElement;
    fireEvent.change(input, { target: { value: "go" } });
    fireEvent.keyDown(input, { key: "Enter" });
    await waitFor(() => {
      expect(useConversationsStore.getState().attachments).toHaveLength(0);
    });
  });

  it("Composer repository picker respects the cap and includes selected content on send", async () => {
    useConversationsStore.setState({ activeConversationId: "c-1" });
    dialogOpen.mockResolvedValue("/tmp/repo");
    invoke.mockImplementation((command: string) => {
      if (command === "list_repo_files") {
        return Promise.resolve({
          dir: "/tmp/repo",
          files: [
            { relPath: "src/main.rs", byteLen: 12 },
            { relPath: "README.md", byteLen: 4 },
          ],
          truncated: false,
        });
      }
      if (command === "read_text_file") {
        return Promise.resolve({
          path: "/tmp/repo/src/main.rs",
          name: "src/main.rs",
          byteLen: 12,
          text: "fn main() {}",
        });
      }
      return Promise.resolve(undefined);
    });
    render(<Composer />);

    fireEvent.click(screen.getByRole("button", { name: "Add repository context" }));
    // The picker lists the files with a running budget signal.
    await screen.findByTestId("repo-picker-budget");
    fireEvent.click(screen.getByRole("checkbox", { name: "Include src/main.rs" }));
    fireEvent.click(screen.getByRole("button", { name: /Include 1 file/ }));

    await waitFor(() => {
      const atts = useConversationsStore.getState().attachments;
      expect(atts).toHaveLength(1);
      expect(atts[0].kind).toBe("repo");
    });

    const input = screen.getByLabelText("Message") as HTMLTextAreaElement;
    fireEvent.change(input, { target: { value: "review" } });
    fireEvent.keyDown(input, { key: "Enter" });
    await waitFor(() => {
      const call = invoke.mock.calls.find((c) => c[0] === "send_message");
      const content = (call?.[1] as { content: string }).content;
      expect(content).toContain("### Repository file: src/main.rs");
      expect(content).toContain("fn main() {}");
      expect(content).toContain("review");
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
