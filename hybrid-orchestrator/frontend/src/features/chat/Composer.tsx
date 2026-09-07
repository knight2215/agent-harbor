// Composer (architecture.md Section 8.1 chat surface).
//
// The message input: a textarea, a Send button, and a Stop button (wired to
// `stopGeneration`). It hosts the PerMessageOverrideControl (Section 8.2). Send
// delegates to the conversations store's `sendMessage`, which reads and CLEARS
// the shared transient override so it applies to exactly one message; the
// assistant reply arrives via streaming CoreEvents, not the send return value.

import { useState } from "react";
import { useConversationsStore } from "../../state/conversations";
import { PerMessageOverrideControl } from "../model-selector";

export function Composer() {
  const activeConversationId = useConversationsStore((s) => s.activeConversationId);
  const sendMessage = useConversationsStore((s) => s.sendMessage);
  const stopGeneration = useConversationsStore((s) => s.stopGeneration);
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
      />
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
