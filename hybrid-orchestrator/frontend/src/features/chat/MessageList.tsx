// MessageList (architecture.md Section 8.1 chat surface).
//
// Renders the active conversation's messages from the conversations store, in
// order, each as a MessageBubble. On mount and whenever the active conversation
// changes, it loads the message history via the store (which delegates to the
// `get_messages` command); streaming updates then arrive via CoreEvents.

import { useEffect, useRef } from "react";
import { useConversationsStore } from "../../state/conversations";
import { MessageBubble } from "./MessageBubble";

/** Length of the last message's text, used as an auto-scroll trigger for streaming deltas. */
function lastTextLength(items: readonly { content: { type: string } }[]): number {
  const last = items[items.length - 1];
  if (last === undefined) return 0;
  const content = last.content as { type: string; text?: string };
  return content.type === "text" && typeof content.text === "string" ? content.text.length : 0;
}

export function MessageList() {
  const activeConversationId = useConversationsStore((s) => s.activeConversationId);
  const messages = useConversationsStore((s) => s.messages);
  const openConversation = useConversationsStore((s) => s.openConversation);
  const endRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (activeConversationId === null) return;
    void openConversation(activeConversationId);
  }, [activeConversationId, openConversation]);

  // Tolerate a not-yet-populated history: there is a real window between
  // selecting a conversation and `openConversation` resolving with its
  // messages, during which the store's `messages` may be empty (or, in
  // partially-seeded states, undefined). Guard so this surface never throws
  // and shows a sensible empty state instead.
  const items = messages ?? [];

  // Keep the thread pinned to the newest turn: re-run whenever a message is
  // added (items.length) or the streaming assistant message grows (last text
  // length). jsdom does not implement scrollIntoView, so only call it when it
  // is a real function; an unguarded call would throw under vitest.
  const itemCount = items.length;
  const tailLength = lastTextLength(items);
  useEffect(() => {
    endRef.current?.scrollIntoView?.({ block: "end" });
  }, [itemCount, tailLength]);

  if (activeConversationId === null) {
    return <div className="message-list message-list--empty">No conversation selected.</div>;
  }

  if (items.length === 0) {
    return <div className="message-list message-list--empty">No messages yet.</div>;
  }

  return (
    <div className="message-list" role="log" aria-label="Conversation messages">
      {items.map((message) => (
        <MessageBubble key={message.id} message={message} />
      ))}
      <div ref={endRef} className="message-list__end" aria-hidden="true" />
    </div>
  );
}
