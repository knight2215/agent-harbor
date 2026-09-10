// Composer (architecture.md Section 8.1 chat surface).
//
// The message input: a textarea, a Send button, and a Stop button (wired to
// `stopGeneration`). It hosts the PerMessageOverrideControl (Section 8.2). Send
// delegates to the conversations store's `sendMessage`, which reads and CLEARS
// the shared transient override so it applies to exactly one message; the
// assistant reply arrives via streaming CoreEvents, not the send return value.

import { useState } from "react";
import type { KeyboardEvent } from "react";
import { useConversationsStore } from "../../state/conversations";
import { PerMessageOverrideControl } from "../model-selector";

export function Composer() {
  const activeConversationId = useConversationsStore((s) => s.activeConversationId);
  const sendMessage = useConversationsStore((s) => s.sendMessage);
  const stopGeneration = useConversationsStore((s) => s.stopGeneration);
  // A rejected `send_message` IPC call produces NO CoreEvents, so this store
  // state is the ONLY visible signal that a send failed (FEAT-002 / Issue 2).
  const sendState = useConversationsStore((s) => s.sendState);
  const sendError = useConversationsStore((s) => s.sendError);
  // A turn is in progress while any message is still streaming. The stop control
  // is only presented as live in that window; the underlying `stop_generation`
  // command is a validated no-op today (the Phase 4 pipeline has no cancel seam),
  // so the button stays disabled and is labelled unavailable rather than posing
  // as a working control (review issue #3).
  const streaming = useConversationsStore((s) =>
    (s.messages ?? []).some((m) => m.status === "streaming"),
  );
  const [draft, setDraft] = useState("");

  const disabled = activeConversationId === null;

  const submit = () => {
    const content = draft.trim();
    if (content.length === 0 || disabled) return;
    void sendMessage(content);
    setDraft("");
  };

  // Enter-to-send / Shift+Enter-newline (Issue 1). Guard IME composition so a
  // CJK candidate-commit Enter is not swallowed as a send: `isComposing` (and
  // the legacy `keyCode === 229`) mark a keydown that belongs to the input
  // method, not the user pressing Enter to send.
  const onKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (event.key !== "Enter") return;
    if (event.nativeEvent.isComposing || event.keyCode === 229) return;
    if (event.shiftKey) return;
    event.preventDefault();
    submit();
  };

  return (
    <form
      className="composer"
      onSubmit={(event) => {
        event.preventDefault();
        submit();
      }}
    >
      <PerMessageOverrideControl />
      <textarea
        className="composer__input"
        aria-label="Message"
        value={draft}
        disabled={disabled}
        onChange={(event) => setDraft(event.target.value)}
        onKeyDown={onKeyDown}
      />
      {sendState === "failed" && (
        <p className="composer__send-error" role="alert">
          Failed to send: {sendError}
        </p>
      )}
      <div className="composer__actions">
        <button
          type="button"
          className="composer__stop"
          onClick={() => void stopGeneration()}
          disabled={!streaming}
          title={streaming ? "Stop (cancellation is not yet available)" : "Nothing to stop"}
        >
          Stop
        </button>
        <button type="submit" className="composer__send" disabled={disabled}>
          Send
        </button>
      </div>
    </form>
  );
}
