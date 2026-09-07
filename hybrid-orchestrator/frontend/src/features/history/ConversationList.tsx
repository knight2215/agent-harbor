// ConversationList (architecture.md Section 8.5 history surface).
//
// Renders the conversation index from the conversations store, sorted by most
// recent activity, filtered client-side by the `query` (matched against the
// title and the privacy tags). Each row shows the title, its updated timestamp,
// its tags, and its pinned route (the "last route"), and clicking a row RESUMES
// that session (restoring persona / route pin / tags / enabled tools via the
// store's `resumeConversation`). Each row also hosts the per-conversation
// context menu.

import { useConversationsStore } from "../../state/conversations";
import type { Conversation, PrivacyTag } from "../../types";
import { ConversationContextMenu } from "./ConversationContextMenu";

/** Render a privacy tag as a short label. */
function tagLabel(tag: PrivacyTag): string {
  if (typeof tag === "string") return tag;
  return `custom:${tag.custom}`;
}

/** True when the conversation matches the (lower-cased) query on title or tags. */
function matchesQuery(conversation: Conversation, query: string): boolean {
  if (query.length === 0) return true;
  const needle = query.toLowerCase();
  if (conversation.title.toLowerCase().includes(needle)) return true;
  return conversation.privacyTags.some((tag) => tagLabel(tag).toLowerCase().includes(needle));
}

export interface ConversationListProps {
  /** Client-side filter text applied to the title and tags. */
  query: string;
}

export function ConversationList({ query }: ConversationListProps) {
  const conversations = useConversationsStore((s) => s.conversations);
  const activeConversationId = useConversationsStore((s) => s.activeConversationId);
  const resumeConversation = useConversationsStore((s) => s.resumeConversation);

  const visible = conversations
    .filter((c) => matchesQuery(c, query))
    .slice()
    .sort((a, b) => b.updatedAt.localeCompare(a.updatedAt));

  if (visible.length === 0) {
    return <p className="conversation-list conversation-list--empty">No conversations.</p>;
  }

  return (
    <ul className="conversation-list" aria-label="Conversations">
      {visible.map((conversation) => (
        <li key={conversation.id} className="conversation-list__item">
          <button
            type="button"
            className="conversation-list__open"
            aria-pressed={conversation.id === activeConversationId}
            data-selected={conversation.id === activeConversationId}
            onClick={() => void resumeConversation(conversation.id)}
          >
            <span className="conversation-list__title">{conversation.title}</span>
            <span className="conversation-list__timestamp">{conversation.updatedAt}</span>
            {conversation.privacyTags.length > 0 && (
              <span className="conversation-list__tags">
                {conversation.privacyTags.map(tagLabel).join(", ")}
              </span>
            )}
            {conversation.conversationPref !== null && (
              <span className="conversation-list__route">
                {conversation.conversationPref.providerId} / {conversation.conversationPref.model}
              </span>
            )}
          </button>
          <ConversationContextMenu conversation={conversation} />
        </li>
      ))}
    </ul>
  );
}
