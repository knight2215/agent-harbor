// Composer (architecture.md Section 8.1 chat surface; FEAT-002 Kiro redesign).
//
// A BOTTOM-ANCHORED message input with a compact control row wrapped around the
// textarea, replacing the old wall of controls stacked above the input. The
// control row hosts, inline:
//   - Attach (📎), Repository (📁), and a Web-search TOGGLE (🌐, aria-pressed)
//     affordances. Attach/Repository are minimal local-state stubs in THIS
//     feature (FEAT-003 wires them to the dialog + backend); the Web-search
//     toggle flips a REAL on/off flag in the conversations store that FEAT-004
//     consumes.
//   - the InlineModelControl (compact model dropdown reading/writing the shared
//     transient per-message override).
//   - the compact routing-mode control (RoutingModeToggle: Auto / Prefer Local /
//     Prefer Quality / Manual, radiogroup semantics preserved).
//   - the "Why this model?" info icon (AutoRationaleTooltip, icon-triggered).
//   - the warning icon -> modal for provider enumeration errors
//     (EnumerationErrorModal, shown ONLY when there are errors).
//   - a small "Refresh models" icon re-running the providers store load().
//
// Send delegates to the conversations store's `sendMessage`, which reads and
// CLEARS the shared transient override so it applies to exactly one message; the
// assistant reply arrives via streaming CoreEvents, not the send return value.
// Enter-to-send / Shift+Enter-newline (IME-safe) and the Stop button semantics
// are preserved EXACTLY, as is the `sendState === "failed"` role=alert
// affordance (a rejected send emits no CoreEvents, so it is the only signal).

import { useState } from "react";
import type { KeyboardEvent } from "react";
import { useConversationsStore } from "../../state/conversations";
import { useProvidersStore } from "../../state/providers";
import {
  AutoRationaleTooltip,
  EnumerationErrorModal,
  InlineModelControl,
  RoutingModeToggle,
} from "../model-selector";

export function Composer() {
  const activeConversationId = useConversationsStore((s) => s.activeConversationId);
  const sendMessage = useConversationsStore((s) => s.sendMessage);
  const stopGeneration = useConversationsStore((s) => s.stopGeneration);
  // A rejected `send_message` IPC call produces NO CoreEvents, so this store
  // state is the ONLY visible signal that a send failed (FEAT-002 / Issue 2).
  const sendState = useConversationsStore((s) => s.sendState);
  const sendError = useConversationsStore((s) => s.sendError);
  // The web-search toggle flips a REAL on/off flag owned by the conversations
  // store so FEAT-004 can consume it when assembling the send context.
  const webSearchEnabled = useConversationsStore((s) => s.webSearchEnabled);
  const setWebSearchEnabled = useConversationsStore((s) => s.setWebSearchEnabled);
  // Re-enumerate models on demand (e.g. after starting a local runtime or saving
  // a key) without reopening Settings. Fire-and-forget; failures surface via the
  // providers store (the warning-icon modal).
  const loadModels = useProvidersStore((s) => s.load);
  // A turn is in progress while any message is still streaming. The stop control
  // is only presented as live in that window; the underlying `stop_generation`
  // command is a validated no-op today (the Phase 4 pipeline has no cancel seam),
  // so the button stays disabled and is labelled unavailable rather than posing
  // as a working control (review issue #3).
  const streaming = useConversationsStore((s) =>
    (s.messages ?? []).some((m) => m.status === "streaming"),
  );
  const [draft, setDraft] = useState("");
  // Attach / Repository handlers are minimal local-state stubs in THIS feature;
  // FEAT-003 replaces them with the dialog + backend file-read wiring. Tracking
  // the counts locally keeps the affordances honest (they visibly do something)
  // without pulling FEAT-003 scope forward.
  const [attachmentCount, setAttachmentCount] = useState(0);
  const [repoCount, setRepoCount] = useState(0);

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
      {sendState === "failed" && (
        <p className="composer__send-error" role="alert">
          Failed to send: {sendError}
        </p>
      )}
      <div className="composer__box">
        <textarea
          className="composer__input"
          aria-label="Message"
          value={draft}
          disabled={disabled}
          onChange={(event) => setDraft(event.target.value)}
          onKeyDown={onKeyDown}
        />
        <div className="composer__controls">
          <div className="composer__controls-left">
            <button
              type="button"
              className="composer__icon"
              aria-label="Attach file"
              title="Attach file"
              disabled={disabled}
              onClick={() => setAttachmentCount((n) => n + 1)}
            >
              <span aria-hidden="true">📎</span>
              {attachmentCount > 0 && (
                <span className="composer__icon-badge" data-testid="attach-count">
                  {attachmentCount}
                </span>
              )}
            </button>
            <button
              type="button"
              className="composer__icon"
              aria-label="Add repository context"
              title="Add repository context"
              disabled={disabled}
              onClick={() => setRepoCount((n) => n + 1)}
            >
              <span aria-hidden="true">📁</span>
              {repoCount > 0 && (
                <span className="composer__icon-badge" data-testid="repo-count">
                  {repoCount}
                </span>
              )}
            </button>
            <button
              type="button"
              className="composer__icon"
              aria-label="Toggle web search"
              aria-pressed={webSearchEnabled}
              title={webSearchEnabled ? "Web search: on" : "Web search: off"}
              data-active={webSearchEnabled}
              onClick={() => setWebSearchEnabled(!webSearchEnabled)}
            >
              <span aria-hidden="true">🌐</span>
            </button>
            <InlineModelControl />
            <RoutingModeToggle />
            <AutoRationaleTooltip conversationId={activeConversationId} />
            <EnumerationErrorModal />
            <button
              type="button"
              className="composer__icon"
              aria-label="Refresh models"
              title="Refresh models"
              onClick={() => void loadModels()}
            >
              <span aria-hidden="true">↻</span>
            </button>
          </div>
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
        </div>
      </div>
    </form>
  );
}
