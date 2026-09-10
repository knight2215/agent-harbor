// WelcomeScreen (architecture.md Section 8.1 chat surface; FEAT-002 landing).
//
// The default landing view of the Chat destination. Before any conversation is
// active the main pane shows THIS screen (a large Agent Harbor logo, a short
// intro, and a prominent primary "New conversation" button) instead of an empty
// chat surface, so a fresh launch never gives the illusion that the user is
// already inside a conversation. The button creates a conversation via the
// conversations store and opens it, which flips `activeConversationId` non-null
// and makes the app shell swap this screen for the chat surface.

import logoUrl from "../../assets/logo.svg";
import { useConversationsStore } from "../../state/conversations";

export function WelcomeScreen() {
  const createConversation = useConversationsStore((s) => s.createConversation);
  const openConversation = useConversationsStore((s) => s.openConversation);

  // Create a fresh conversation and open it so the chat surface appears. Opening
  // (rather than only creating) is what flips `activeConversationId` non-null.
  const start = async () => {
    const created = await createConversation();
    await openConversation(created.id);
  };

  return (
    <section className="welcome" aria-label="Welcome">
      <img
        className="welcome__logo"
        src={logoUrl}
        alt="Agent Harbor"
        data-testid="welcome-logo"
        width={96}
        height={96}
      />
      <h2 className="welcome__title">Welcome to Agent Harbor</h2>
      <p className="welcome__intro">
        Route your chats across local and cloud models from one place. Start a new conversation to
        pick up a thread, attach files, add repository context, or search the web.
      </p>
      <button type="button" className="welcome__cta" onClick={() => void start()}>
        New conversation
      </button>
    </section>
  );
}
