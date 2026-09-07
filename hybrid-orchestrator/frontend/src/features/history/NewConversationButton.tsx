// NewConversationButton (architecture.md Section 8.5 history surface).
//
// Creates a fresh conversation via the conversations store (-> the
// `create_conversation` command) and, once created, sets it active by resuming
// it so the chat surface opens on the new session.

import { useConversationsStore } from "../../state/conversations";

export function NewConversationButton() {
  const createConversation = useConversationsStore((s) => s.createConversation);
  const resumeConversation = useConversationsStore((s) => s.resumeConversation);

  const create = async () => {
    const created = await createConversation();
    await resumeConversation(created.id);
  };

  return (
    <button type="button" className="history__new" onClick={() => void create()}>
      New conversation
    </button>
  );
}
