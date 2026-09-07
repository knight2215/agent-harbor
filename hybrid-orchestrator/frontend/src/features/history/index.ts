// UI surface 5: Conversation history / session management (architecture.md
// Section 8.5).
//
// Lists and searches the conversation index, offers per-conversation actions
// (rename / tag / duplicate / export / delete) via a context menu, shows a
// session summary and a new-conversation button, and RESUMES a session
// (restoring persona / route pin / privacy tags / enabled tool servers). The
// index stays in sync via the conversations store fed by the app shell's single
// top-level `onCoreEvent` fan-out.

export { ConversationContextMenu } from "./ConversationContextMenu";
export type { ConversationContextMenuProps } from "./ConversationContextMenu";
export { ConversationList } from "./ConversationList";
export type { ConversationListProps } from "./ConversationList";
export { History } from "./History";
export { NewConversationButton } from "./NewConversationButton";
export { SearchBar } from "./SearchBar";
export type { SearchBarProps } from "./SearchBar";
export { SessionSummary } from "./SessionSummary";
