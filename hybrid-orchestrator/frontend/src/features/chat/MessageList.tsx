// MessageList (architecture.md Section 8.1 chat surface).
//
// Renders the active conversation's messages from the conversations store, in
// order, each as a MessageBubble. On mount and whenever the active conversation
// changes, it loads the message history via the store (which delegates to the
// `get_messages` command); streaming updates then arrive via CoreEvents.

import { useEffect } from "react";
import { useConversationsStore } from "../../state/conversations";
import { MessageBubble } from "./MessageBubble";

export function MessageList() {
  const activeConversationId = useConversationsStore((s) => s.activeConversationId);
  const messages = useConversationsStore((s) => s.messages);
  const openConversation = useConversationsStore((s) => s.openConversation);

  useEffect(() => {
    if (activeConversationId === null) return;
    void openConversation(activeConversationId);
  }, [activeConversationId, openConversation]);

  if (activeConversationId === null) {
    return <div className="message-list message-list--empty">No conversation selected.</div>;
  }

  return (
    <div className="message-list" role="log" aria-label="Conversation messages">
      {messages.map((message) => (
        <MessageBubble key={message.id} message={message} />
      ))}
    </div>
  );
}
