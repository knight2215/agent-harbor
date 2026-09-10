// MessageBubble (architecture.md Section 8.1 chat surface).
//
// Renders a single message: its role, its content (every MessageContent
// variant — text, tool calls, tool results, attachments), a RouteBadge for the
// message's RouteMetadata, and a StreamingIndicator while it is streaming.

import type { Message, MessageContent } from "../../types";
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
}

export function MessageBubble({ message }: MessageBubbleProps) {
  const streaming = message.status === "streaming";
  const errored = message.status === "error";
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
  return (
    <article className="message-bubble" data-role={message.role} data-status={message.status}>
      <header className="message-bubble__header">
        <span className="message-bubble__role">{message.role}</span>
        {message.route !== null && <RouteBadge route={message.route} />}
      </header>
      {isTextError ? (
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
    </article>
  );
}
