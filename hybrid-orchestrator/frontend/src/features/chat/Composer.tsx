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
        <button type="button" className="composer__stop" onClick={() => void stopGeneration()}>
          Stop
        </button>
        <button type="submit" className="composer__send" disabled={disabled}>
          Send
        </button>
      </div>
    </form>
  );
}
