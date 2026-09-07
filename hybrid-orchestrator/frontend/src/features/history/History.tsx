// History (architecture.md Section 8.5 conversation history / session management).
//
// The history surface and natural session entry point: it loads the
// conversation index on mount, owns the client-side search query, and assembles
// the SearchBar, NewConversationButton, ConversationList, and SessionSummary.
// The index stays in sync with the core through the conversations store, whose
// `applyCoreEvent` refetches on conversation_created / conversation_updated and
// drops rows on conversation_deleted; those events arrive from the app shell's
// single top-level `onCoreEvent` fan-out. Resuming a row restores the full
// session (persona / route pin / tags / enabled tool servers) via the store's
// `resumeConversation`.

import { useEffect, useState } from "react";
import { useConversationsStore } from "../../state/conversations";
import { ConversationList } from "./ConversationList";
import { NewConversationButton } from "./NewConversationButton";
import { SearchBar } from "./SearchBar";
import { SessionSummary } from "./SessionSummary";

export function History() {
  const loadConversations = useConversationsStore((s) => s.loadConversations);
  const [query, setQuery] = useState("");

  useEffect(() => {
    void loadConversations();
  }, [loadConversations]);

  return (
    <section className="history" aria-label="Conversation history">
      <header className="history__header">
        <h2>Conversations</h2>
        <NewConversationButton />
      </header>
      <SearchBar value={query} onChange={setQuery} />
      <ConversationList query={query} />
      <SessionSummary />
    </section>
  );
}
