// SessionSummary (architecture.md Section 8.5 history surface).
//
// A display-safe rollup of the active conversation: the models that answered
// (from each message's RouteMetadata), aggregate token usage (from each
// message's TokenUsage), and the conversation's privacy tags. It is presented
// from the conversations store's active conversation + messages; it owns no
// store state and issues no IPC.

import { useConversationsStore } from "../../state/conversations";
import type { PrivacyTag } from "../../types";

/** Render a privacy tag as a short label. */
function tagLabel(tag: PrivacyTag): string {
  if (typeof tag === "string") return tag;
  return `custom:${tag.custom}`;
}

export function SessionSummary() {
  const activeConversationId = useConversationsStore((s) => s.activeConversationId);
  const conversations = useConversationsStore((s) => s.conversations);
  const messages = useConversationsStore((s) => s.messages);

  const conversation = conversations.find((c) => c.id === activeConversationId) ?? null;
  if (conversation === null) {
    return <div className="session-summary session-summary--empty">No active session.</div>;
  }

  // Distinct provider/model pairs that answered in this session.
  const models = Array.from(
    new Set(
      messages
        .map((m) => m.route)
        .filter((route): route is NonNullable<typeof route> => route !== null)
        .map((route) => `${route.providerId} / ${route.model}`),
    ),
  );

  // Aggregate token usage across completed messages.
  const totalTokens = messages.reduce((sum, m) => sum + (m.usage?.totalTokens ?? 0), 0);

  return (
    <section className="session-summary" aria-label="Session summary">
      <h3 className="session-summary__title">{conversation.title}</h3>
      <dl className="session-summary__stats">
        <dt>Models</dt>
        <dd>{models.length === 0 ? "none yet" : models.join(", ")}</dd>
        <dt>Total tokens</dt>
        <dd>{totalTokens}</dd>
        <dt>Tags</dt>
        <dd>
          {conversation.privacyTags.length === 0
            ? "none"
            : conversation.privacyTags.map(tagLabel).join(", ")}
        </dd>
      </dl>
    </section>
  );
}
