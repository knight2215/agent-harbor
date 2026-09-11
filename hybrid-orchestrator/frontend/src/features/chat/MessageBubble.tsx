// MessageBubble (architecture.md Section 8.1 chat surface).
//
// Renders a single message: its role, its content (every MessageContent
// variant — text, tool calls, tool results, attachments), a RouteBadge for the
// message's RouteMetadata, and a StreamingIndicator while it is streaming.
//
// It also hosts the FEAT-004 per-message affordances in an action row below the
// content: Copy (on EVERY bubble), Edit (only on the LAST user message), and
// Regenerate + Continue (only on the LAST assistant message). All of these are
// display-safe and honest about what the Phase-4 pipeline supports:
//   - Regenerate / Edit re-run the turn as a FRESH turn via the existing send
//     path (there is no in-place replace / branch seam in the core).
//   - Continue sends a follow-up "continue" turn - NOT a true resume (the same
//     reason Stop is a validated no-op), matching the honest-affordance
//     convention already used for Stop in the composer.
// None of the turn-affecting actions are shown while a turn is streaming, so a
// mid-stream regenerate/continue can never be triggered incoherently.

import { useState } from "react";
import type { Message, MessageContent } from "../../types";
import { useConversationsStore } from "../../state/conversations";
import { RouteBadge } from "./RouteBadge";
import { StreamingIndicator } from "./StreamingIndicator";

/** Render an arbitrary tool payload (arguments / result content) as pretty JSON. */
function formatPayload(value: unknown): string {
  if (typeof value === "string") return value;
  try {
    return JSON.stringify(value, null, 2);
  } catch {
    return String(value);
  }
}

/**
 * Produce a sensible plain-text representation of a message's content for the
 * clipboard. Text messages copy their text verbatim; tool/attachment content
 * reuses {@link formatPayload} so a Copy always yields something useful.
 */
function contentToText(content: MessageContent): string {
  switch (content.type) {
    case "text":
      return content.text;
    case "toolCalls":
      return content.calls
        .map((call) => `${call.name}(${formatPayload(call.arguments)})`)
        .join("\n");
    case "toolResults":
      return content.results
        .map((result) => `${result.isError ? "error" : "ok"}: ${formatPayload(result.content)}`)
        .join("\n");
    case "attachments":
      return content.attachments
        .map((attachment) => attachment.name ?? attachment.uri)
        .join("\n");
  }
}

function ContentBody({ content }: { content: MessageContent }) {
  switch (content.type) {
    case "text":
      return <p className="message-bubble__text">{content.text}</p>;
    case "toolCalls":
      return (
        <ul className="message-bubble__tool-calls">
          {content.calls.map((call) => (
            <li key={call.id} className="tool-call">
              <span className="tool-call__name">{call.name}</span>
              <pre className="tool-call__args">{formatPayload(call.arguments)}</pre>
            </li>
          ))}
        </ul>
      );
    case "toolResults":
      return (
        <ul className="message-bubble__tool-results">
          {content.results.map((result) => (
            <li key={result.callId} className="tool-result" data-error={result.isError}>
              <span className="tool-result__status">{result.isError ? "error" : "ok"}</span>
              <pre className="tool-result__content">{formatPayload(result.content)}</pre>
            </li>
          ))}
        </ul>
      );
    case "attachments":
      return (
        <ul className="message-bubble__attachments">
          {content.attachments.map((attachment) => (
            <li key={attachment.uri} className="attachment">
              <a href={attachment.uri}>{attachment.name ?? attachment.uri}</a>
              <span className="attachment__mime">{attachment.mimeType}</span>
            </li>
          ))}
        </ul>
      );
  }
}

export interface MessageBubbleProps {
  message: Message;
  /**
   * Whether this is the LAST user message in the thread (FEAT-004). When true
   * an Edit affordance is shown, scoped to editing + re-sending this turn.
   */
  isLastUser?: boolean;
  /**
   * Whether this is the LAST assistant message in the thread (FEAT-004). When
   * true the Regenerate + Continue affordances are shown.
   */
  isLastAssistant?: boolean;
}

export function MessageBubble({ message, isLastUser, isLastAssistant }: MessageBubbleProps) {
  const streaming = message.status === "streaming";
  const errored = message.status === "error";
  // Reasoning ("thinking") trace (FEAT-003): rendered ABOVE the answer as a
  // collapsible, visually-distinct section, but ONLY when the "🧠 Thinking"
  // toggle is ON (OFF by default) AND the model actually emitted reasoning.
  // When the toggle is off or the reasoning is absent/empty, nothing renders
  // (no empty section, no crash) and the content path is unchanged.
  const thinkingEnabled = useConversationsStore((s) => s.thinkingEnabled);
  const regenerateLastTurn = useConversationsStore((s) => s.regenerateLastTurn);
  const editAndResend = useConversationsStore((s) => s.editAndResend);
  const continueTurn = useConversationsStore((s) => s.continueTurn);
  const thinking = message.thinking ?? "";
  const hasThinking = thinkingEnabled && thinking.length > 0;

  // Copied-confirmation state, local to this bubble so it never affects the
  // streaming/error render. Reset by a short timer after a successful copy.
  const [copied, setCopied] = useState(false);
  // Inline edit state for the last user message (textarea + save/cancel).
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState("");

  const onCopy = async () => {
    try {
      await navigator.clipboard.writeText(contentToText(message.content));
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      // Clipboard access can be denied; fail silently rather than crash the
      // bubble (the copy affordance is best-effort).
    }
  };

  const startEdit = () => {
    setDraft(message.content.type === "text" ? message.content.text : "");
    setEditing(true);
  };

  const saveEdit = () => {
    setEditing(false);
    void editAndResend(message.id, draft);
  };

  // On error, a non-empty text message IS its own reason (the messageError
  // reducer puts the display-safe reason into the text), so it is rendered once
  // as a role="alert" paragraph. A NON-text errored message (tool calls / tool
  // results / attachments) keeps its real content visible via ContentBody, and
  // an ADDITIONAL role="alert" line carries the generic reason so the failure
  // is announced without hiding the content. An empty text error shows the
  // generic string. Exactly one role="alert" element is rendered.
  const isTextError = errored && message.content.type === "text";
  const errorText =
    isTextError && message.content.type === "text" && message.content.text.length > 0
      ? message.content.text
      : "This message failed to generate.";

  // Turn-affecting affordances (Regenerate / Continue / Edit) are only coherent
  // when the message is settled - never mid-stream (review-safe: no regenerate
  // mid-stream). Copy is always available (nothing to break).
  const showEdit = isLastUser === true && !streaming;
  const showRegenerate = isLastAssistant === true && !streaming;
  return (
    <article className="message-bubble" data-role={message.role} data-status={message.status}>
      <header className="message-bubble__header">
        <span className="message-bubble__role">{message.role}</span>
        {message.route !== null && <RouteBadge route={message.route} />}
      </header>
      {hasThinking && (
        <details className="message-bubble__reasoning" data-testid="message-reasoning">
          <summary className="message-bubble__reasoning-summary">🧠 Reasoning</summary>
          <p className="message-bubble__reasoning-text">{thinking}</p>
        </details>
      )}
      {editing ? (
        // Inline edit UI for the last user message (FEAT-004). Deeper branching
        // (editing an arbitrary earlier message + truncating the tree) is
        // deferred; this edits + re-sends the last user turn as a fresh turn.
        <div className="message-bubble__edit">
          <textarea
            className="message-bubble__edit-input"
            aria-label="Edit message"
            value={draft}
            onChange={(event) => setDraft(event.target.value)}
          />
          <div className="message-bubble__edit-actions">
            <button
              type="button"
              className="message-bubble__action"
              aria-label="Save edited message"
              onClick={saveEdit}
            >
              Save &amp; resend
            </button>
            <button
              type="button"
              className="message-bubble__action"
              aria-label="Cancel edit"
              onClick={() => setEditing(false)}
            >
              Cancel
            </button>
          </div>
        </div>
      ) : isTextError ? (
        // A text error: render the reason (or generic fallback) once, as the alert.
        <p className="message-bubble__error" role="alert">
          {errorText}
        </p>
      ) : (
        <>
          {/* Non-error, or a non-text error: always show the real content ... */}
          <ContentBody content={message.content} />
          {/* ... and, for a non-text error, announce the failure alongside it. */}
          {errored && (
            <p className="message-bubble__error" role="alert">
              This message failed to generate.
            </p>
          )}
        </>
      )}
      {streaming && <StreamingIndicator />}
      {!editing && (
        <div className="message-bubble__actions">
          <button
            type="button"
            className="message-bubble__action"
            aria-label="Copy message"
            onClick={() => void onCopy()}
          >
            {copied ? "Copied" : "Copy"}
          </button>
          {showEdit && (
            <button
              type="button"
              className="message-bubble__action"
              aria-label="Edit message"
              onClick={startEdit}
            >
              Edit
            </button>
          )}
          {showRegenerate && (
            <>
              <button
                type="button"
                className="message-bubble__action"
                aria-label="Regenerate response"
                onClick={() => void regenerateLastTurn()}
              >
                Regenerate
              </button>
              {/* Continue is a follow-up turn, NOT a resume (the pipeline has no
                  resume seam - same reason Stop is a no-op). */}
              <button
                type="button"
                className="message-bubble__action"
                aria-label="Continue response"
                title="Sends a follow-up 'continue' turn (not a true resume)"
                onClick={() => void continueTurn()}
              >
                Continue
              </button>
            </>
          )}
        </div>
      )}
    </article>
  );
}
