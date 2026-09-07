// UI surface 1: Chat (architecture.md Section 8.1).
//
// Exports the chat components. The app shell (a later feature) composes these
// into the chat pane and wires the single top-level `onCoreEvent` fan-out.

export { Composer } from "./Composer";
export { MessageBubble } from "./MessageBubble";
export type { MessageBubbleProps } from "./MessageBubble";
export { MessageList } from "./MessageList";
export { PermissionPrompt } from "./PermissionPrompt";
export { RouteBadge } from "./RouteBadge";
export type { RouteBadgeProps } from "./RouteBadge";
export { StreamingIndicator } from "./StreamingIndicator";
