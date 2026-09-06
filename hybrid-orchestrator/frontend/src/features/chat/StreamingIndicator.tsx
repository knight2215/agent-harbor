// StreamingIndicator (architecture.md Section 8.1 chat surface).
//
// A small "assistant is typing" indicator shown while a message is streaming.

export function StreamingIndicator() {
  return (
    <span className="streaming-indicator" role="status" aria-label="Assistant is responding">
      <span className="streaming-indicator__dot" />
      <span className="streaming-indicator__dot" />
      <span className="streaming-indicator__dot" />
    </span>
  );
}
