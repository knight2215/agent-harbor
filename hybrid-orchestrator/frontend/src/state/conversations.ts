// Conversations store (architecture.md Section 8.1 / 8.2 / 8.5).
//
// Owns the conversation index, the active conversation, the active
// conversation's messages, its routing mode (Automatic vs a manual pin), the
// TRANSIENT per-message override (a single value owned HERE per Section 8.2:
// the model selector edits it, this store owns it, and the Composer send path
// consumes-and-clears it so it applies to exactly one message), and the queue
// of pending Ask-mode permission requests.
//
// The core is authoritative (Section 7.3): actions delegate to the IPC commands
// and CoreEvents keep the cache in sync. `applyCoreEvent` is the single
// reducer a top-level `onCoreEvent` subscription (wired in the app shell) feeds
// events into; it appends streaming deltas, finalizes completed messages,
// records errors, invalidates+refetches on conversation lifecycle events, and
// enqueues/dequeues permission requests.

import { create } from "zustand";
import {
  assignPersona as assignPersonaCmd,
  createConversation as createConversationCmd,
  deleteConversation as deleteConversationCmd,
  exportConversation as exportConversationCmd,
  getMessages,
  listConversations,
  openConversation as openConversationCmd,
  renameConversation as renameConversationCmd,
  resolvePermission as resolvePermissionCmd,
  sendMessage as sendMessageCmd,
  setConversationRoute as setConversationRouteCmd,
  setConversationRoutingMode as setConversationRoutingModeCmd,
  setConversationTags as setConversationTagsCmd,
  stopGeneration as stopGenerationCmd,
} from "../ipc/commands";
import type {
  Conversation,
  CoreEvent,
  ExportFormat,
  ManualRoute,
  Message,
  MessageContent,
  PermissionDecision,
  PermissionMode,
  PrivacyTag,
  Role,
  RoutingMode,
} from "../types";

/**
 * The lifecycle of the most recent {@link ConversationsState.sendMessage}. See
 * {@link ConversationsState.sendState} for why `failed` matters (a rejected
 * `send_message` emits no CoreEvents, so the send state is the only signal).
 */
export type SendState = "idle" | "sending" | "failed";

/**
 * One piece of context attached to the NEXT chat turn (FEAT-003). Held in the
 * conversations store (like the transient per-message override) so it is
 * cleared after a successful send. Three kinds:
 *   - `text`: an attached text file (its `text` contents are inlined).
 *   - `repo`: a repository file selected from the folder picker (also text
 *      contents, tagged separately so the UI can show it under "Repository").
 *   - `image`: an attached image (base64 + mime); only ever created when the
 *      selected model advertises `vision`, so its presence already passed the
 *      vision gate.
 * `byteLen` is the file's size, used to enforce the total-size caps.
 */
export interface Attachment {
  kind: "text" | "repo" | "image";
  name: string;
  path: string;
  byteLen: number;
  /** Present for `text` / `repo` kinds: the file's UTF-8 contents. */
  text?: string;
  /** Present for `image` kind: the base64-encoded file. */
  base64?: string;
  /** Present for `image` kind: the MIME type (e.g. `image/png`). */
  mimeType?: string;
}

/** A pending Ask-mode tool-permission request awaiting the user's decision. */
export interface PendingPermission {
  requestId: string;
  serverId: string;
  toolName: string;
  mode: PermissionMode;
  rationale: string;
}

export interface ConversationsState {
  /** All conversations, newest activity first is left to the surface. */
  conversations: Conversation[];
  /** The active conversation id, or null when none is open. */
  activeConversationId: string | null;
  /** Messages for the active conversation, in order. */
  messages: Message[];
  /** The transient per-message override (Section 8.2), or null for automatic. */
  pendingOverride: ManualRoute | null;
  /**
   * Whether the composer's web-search toggle is ON (FEAT-002 lands the toggle
   * + flag; FEAT-004 consumes it when assembling the send context). A real
   * on/off flag rather than a stub so the affordance is meaningful today.
   */
  webSearchEnabled: boolean;
  /**
   * Whether the composer's "🧠 Thinking" toggle is ON (FEAT-003). OFF by
   * default. When ON, streamed reasoning ("thinking") is rendered in a
   * collapsible Reasoning section above the answer; when OFF, the reasoning
   * section is hidden even if the model emitted reasoning. A real on/off flag,
   * persisted like {@link webSearchEnabled}.
   */
  thinkingEnabled: boolean;
  /**
   * Files attached to the NEXT chat turn (FEAT-003): text/repo file contents
   * and vision-gated images. Assembled into a bounded, delimited context block
   * that is prepended to the user message `content` on send, then CLEARED after
   * a successful send so it applies to exactly one turn (mirroring the
   * transient per-message override).
   */
  attachments: Attachment[];
  /** Queue of pending Ask-mode permission requests (Section 9.4). */
  pendingPermissions: PendingPermission[];
  /**
   * The lifecycle of the most recent {@link ConversationsState.sendMessage}
   * attempt: `idle` before/after a settled send, `sending` while the
   * `send_message` IPC call is in flight, `failed` when that call REJECTED. The
   * `failed` state exists because a rejected `send_message` produces NO
   * CoreEvents at all, so without it a failed send would be a silent no-op (the
   * "said hello, got no reply and no error" bug).
   */
  sendState: SendState;
  /**
   * The display-safe message of the last send REJECTION (the thrown
   * CommandError / Error), or `null` when the last send did not reject. Set when
   * `sendState === "failed"` so the composer can show WHY the send failed.
   * Never carries secret material.
   */
  sendError: string | null;

  // --- Loading actions (core authoritative) --------------------------------
  /** Load the conversation index from the core. */
  loadConversations: () => Promise<void>;
  /** Open a conversation: set it active and load its messages. */
  openConversation: (conversationId: string) => Promise<void>;
  /**
   * Resume a full session (architecture.md Section 8.5): load the conversation
   * plus its messages in one round-trip via the `open_conversation` command,
   * set it active, and refresh the cached `Conversation` record so the restored
   * persona, route pin (`conversationPref`), privacy tags, and enabled tool
   * servers are reflected across the chat, model-selector, and tool surfaces.
   */
  resumeConversation: (conversationId: string) => Promise<Conversation>;

  // --- Mutations (delegate to the core, then reflect the result) -----------
  createConversation: (title?: string) => Promise<Conversation>;
  renameConversation: (conversationId: string, title: string) => Promise<void>;
  setConversationTags: (conversationId: string, tags: PrivacyTag[]) => Promise<void>;
  deleteConversation: (conversationId: string) => Promise<void>;
  setConversationRoute: (conversationId: string, route: ManualRoute | null) => Promise<void>;
  /**
   * Set (or clear) the per-conversation routing mode (FEAT-002). Delegates to
   * the `set_conversation_routing_mode` command and reflects the returned
   * record into the cache, mirroring {@link setConversationRoute}. The manual
   * pin (`conversationPref`) is managed separately via `setConversationRoute`.
   */
  setConversationRoutingMode: (conversationId: string, mode: RoutingMode | null) => Promise<void>;
  assignPersona: (conversationId: string, personaId: string | null) => Promise<void>;
  /**
   * Duplicate a conversation by creating a NEW one seeded from the source's
   * persona, route pin, and privacy tags (there is no dedicated duplicate
   * command in Phase 5, so the seed is applied client-side over
   * `create_conversation`). The new conversation starts with an empty message
   * history.
   */
  duplicateConversation: (conversationId: string) => Promise<Conversation>;
  /** Export a conversation to a display-safe string in the given format. */
  exportConversation: (conversationId: string, format: ExportFormat) => Promise<string>;

  // --- Transient per-message override (owned here, Section 8.2) ------------
  setPendingOverride: (route: ManualRoute | null) => void;
  /** Read and clear the transient override for a single send. */
  consumePendingOverride: () => ManualRoute | null;

  // --- Web-search toggle (FEAT-002 flag; FEAT-004 consumer) ----------------
  /** Set the composer's web-search on/off flag. */
  setWebSearchEnabled: (enabled: boolean) => void;

  // --- Thinking toggle (FEAT-003) ------------------------------------------
  /** Set the composer's thinking/reasoning-trace on/off flag. */
  setThinkingEnabled: (enabled: boolean) => void;

  // --- Attachments / repository context (FEAT-003) -------------------------
  /** Add an attachment for the next turn (deduped by path + kind). */
  addAttachment: (attachment: Attachment) => void;
  /** Remove the attachment with the given path + kind. */
  removeAttachment: (path: string, kind: Attachment["kind"]) => void;
  /** Clear all attachments (used after a successful send). */
  clearAttachments: () => void;

  // --- Send / stop (Section 8.1) -------------------------------------------
  /**
   * Send a user message on the active conversation. Reads and CLEARS the
   * transient per-message override (Section 8.2) so it applies to exactly one
   * message, then delegates to the `send_message` command. The assistant reply
   * arrives via streaming CoreEvents, not this call's return value.
   */
  sendMessage: (content: string) => Promise<void>;
  /** Request cancellation of the active conversation's in-flight generation. */
  stopGeneration: () => Promise<void>;
  /**
   * Regenerate the last assistant turn (FEAT-004). Finds the most recent USER
   * message preceding the last assistant message and re-invokes
   * {@link sendMessage} with that user message's content, re-running the turn.
   *
   * MECHANISM: this APPENDS A FRESH TURN via the existing send path; it does
   * NOT mutate or delete the prior assistant row. There is no dedicated
   * regenerate command and the Phase-4 pipeline offers no in-place replace
   * seam, so an honest fresh turn is the correct affordance (a faked in-place
   * replace would misrepresent what the core actually did). A no-op when there
   * is no active conversation or no preceding user message.
   */
  regenerateLastTurn: () => Promise<void>;
  /**
   * Edit the last USER message and re-send (FEAT-004). Re-invokes
   * {@link sendMessage} with `newContent` so the edited prompt runs as a new
   * turn through the existing send path.
   *
   * SCOPE: the high-value case is editing the LAST user message + resend, which
   * is all this implements. `messageId` identifies which user message is being
   * edited; if it is not the last user message the call is a no-op. Deeper
   * branching (editing an arbitrary earlier message and truncating the
   * conversation tree from that point) is DEFERRED - the pipeline has no
   * truncate/branch seam, so like {@link regenerateLastTurn} this appends a
   * fresh turn rather than rewriting history.
   */
  editAndResend: (messageId: string, newContent: string) => Promise<void>;
  /**
   * Continue the last assistant turn (FEAT-004). Sends a follow-up turn with a
   * short "continue" prompt through {@link sendMessage}.
   *
   * HONESTY NOTE: this is a FOLLOW-UP TURN, not a true resume. The Phase-4
   * pipeline has NO cancel/resume seam (the same reason {@link stopGeneration}
   * is a validated no-op), so there is no partial generation to resume; asking
   * the model to continue in a fresh turn is the honest affordance. A no-op
   * when there is no active conversation.
   */
  continueTurn: () => Promise<void>;

  // --- Permission queue ----------------------------------------------------
  /**
   * Resolve a pending Ask-mode permission request (Section 9.4). Forwards the
   * user's `{ allow, remember }` `decision` to the core through the
   * `resolve_permission` command to unblock the tool invocation, then dequeues
   * the request from the local queue.
   */
  resolvePermission: (requestId: string, decision: PermissionDecision) => Promise<void>;

  // --- Event application (core authoritative, Section 7.3) -----------------
  applyCoreEvent: (event: CoreEvent) => void;
}

/** Append a text delta to the streaming message identified by `messageId`. */
function appendDelta(messages: Message[], messageId: string, delta: string): Message[] {
  const index = messages.findIndex((m) => m.id === messageId);
  if (index === -1) {
    return messages;
  }
  const message = messages[index];
  const text = message.content.type === "text" ? message.content.text : "";
  const content: MessageContent = { type: "text", text: text + delta };
  const next = messages.slice();
  next[index] = { ...message, content, status: "streaming" };
  return next;
}

/**
 * Append a reasoning ("thinking") delta to the streaming message identified by
 * `messageId` (FEAT-003). Accumulates onto the FRONTEND-ONLY `thinking` field,
 * leaving `content` (the answer) untouched so the answer path is unaffected.
 */
function appendThinking(messages: Message[], messageId: string, delta: string): Message[] {
  const index = messages.findIndex((m) => m.id === messageId);
  if (index === -1) {
    return messages;
  }
  const message = messages[index];
  const next = messages.slice();
  next[index] = {
    ...message,
    thinking: (message.thinking ?? "") + delta,
    status: "streaming",
  };
  return next;
}

/**
 * Build a placeholder [`Message`] for a `messageStarted` event so the chat
 * surface shows the message before any delta arrives (architecture.md Section
 * 7.4). The user message lands `complete`; the assistant reply lands
 * `streaming` and accumulates deltas.
 *
 * `text` seeds the message content when the event carries it. The pipeline
 * announces the persisted USER message WITH its full text (there is no user
 * streaming), so the user's own words are visible immediately on send instead
 * of an empty bubble that no delta ever fills (Bug 2). The assistant reply
 * announces no text and seeds empty, then accumulates via `messageDelta`.
 */
function placeholderMessage(
  conversationId: string,
  messageId: string,
  role: Role,
  text?: string,
): Message {
  return {
    id: messageId,
    conversationId,
    role,
    content: { type: "text", text: text ?? "" },
    createdAt: new Date().toISOString(),
    route: null,
    usage: null,
    status: role === "assistant" ? "streaming" : "complete",
  };
}

export const useConversationsStore = create<ConversationsState>((set, get) => ({
  conversations: [],
  activeConversationId: null,
  messages: [],
  pendingOverride: null,
  webSearchEnabled: false,
  thinkingEnabled: false,
  attachments: [],
  pendingPermissions: [],
  sendState: "idle",
  sendError: null,

  loadConversations: async () => {
    const conversations = await listConversations();
    set({ conversations });
  },

  openConversation: async (conversationId) => {
    const messages = await getMessages(conversationId);
    // Reset the global send status on a conversation switch so a stale
    // "Failed to send" alert from a prior conversation never shows against the
    // newly opened one (sendState/sendError are store-global, not per-row).
    set({ activeConversationId: conversationId, messages, sendState: "idle", sendError: null });
  },

  resumeConversation: async (conversationId) => {
    const opened = await openConversationCmd(conversationId);
    const { conversation, messages } = opened;
    set((state) => {
      const present = state.conversations.some((c) => c.id === conversation.id);
      return {
        // Refresh the cached record so the restored persona / route pin /
        // privacy tags / enabled tool servers are current for every surface.
        conversations: present
          ? state.conversations.map((c) => (c.id === conversation.id ? conversation : c))
          : [...state.conversations, conversation],
        activeConversationId: conversation.id,
        messages,
        // Clear the global send status on the switch (see openConversation).
        sendState: "idle",
        sendError: null,
      };
    });
    return conversation;
  },

  createConversation: async (title) => {
    const conversation = await createConversationCmd(title ? { title } : {});
    // UPSERT by id rather than a blind append (Bug 1). The core is
    // authoritative (Section 7.3): the very same conversation also arrives
    // through `loadConversations` when the pipeline emits `conversationUpdated`
    // on the first message persist. A blind append plus that refetch could
    // otherwise surface the freshly created "New Conversation" TWICE (once from
    // the local append, once from a list that has not yet converged). Replacing
    // the row when present - and only appending when genuinely new - keeps
    // exactly one row for the created conversation regardless of ordering.
    set((state) => {
      const present = state.conversations.some((c) => c.id === conversation.id);
      return {
        conversations: present
          ? state.conversations.map((c) => (c.id === conversation.id ? conversation : c))
          : [...state.conversations, conversation],
      };
    });
    return conversation;
  },

  renameConversation: async (conversationId, title) => {
    const updated = await renameConversationCmd(conversationId, title);
    set((state) => ({
      conversations: state.conversations.map((c) => (c.id === updated.id ? updated : c)),
    }));
  },

  setConversationTags: async (conversationId, tags) => {
    const updated = await setConversationTagsCmd(conversationId, tags);
    set((state) => ({
      conversations: state.conversations.map((c) => (c.id === updated.id ? updated : c)),
    }));
  },

  deleteConversation: async (conversationId) => {
    await deleteConversationCmd(conversationId);
    set((state) => {
      const wasActive = state.activeConversationId === conversationId;
      return {
        conversations: state.conversations.filter((c) => c.id !== conversationId),
        activeConversationId: wasActive ? null : state.activeConversationId,
        messages: wasActive ? [] : state.messages,
        // Clearing the active conversation is a switch too: drop any stale send
        // status so it cannot surface against a different conversation.
        sendState: wasActive ? "idle" : state.sendState,
        sendError: wasActive ? null : state.sendError,
      };
    });
  },

  setConversationRoute: async (conversationId, route) => {
    const updated = await setConversationRouteCmd(conversationId, route);
    set((state) => ({
      conversations: state.conversations.map((c) => (c.id === updated.id ? updated : c)),
    }));
  },

  setConversationRoutingMode: async (conversationId, mode) => {
    const updated = await setConversationRoutingModeCmd(conversationId, mode);
    set((state) => ({
      conversations: state.conversations.map((c) => (c.id === updated.id ? updated : c)),
    }));
  },

  assignPersona: async (conversationId, personaId) => {
    const updated = await assignPersonaCmd(conversationId, personaId);
    set((state) => ({
      conversations: state.conversations.map((c) => (c.id === updated.id ? updated : c)),
    }));
  },

  duplicateConversation: async (conversationId) => {
    const source = get().conversations.find((c) => c.id === conversationId);
    if (source === undefined) {
      throw new Error(`unknown conversation: ${conversationId}`);
    }
    // Seed the new conversation from the source's persona / tags (Section 8.5).
    const created = await createConversationCmd({
      title: `${source.title} (copy)`,
      personaId: source.personaId,
      privacyTags: source.privacyTags,
    });
    // Carry over the route pin, which create_conversation does not accept.
    let seeded = created;
    if (source.conversationPref !== null) {
      seeded = await setConversationRouteCmd(created.id, source.conversationPref);
    }
    // Upsert by id (see createConversation): keep exactly one row for the new
    // conversation even if a refetch has already surfaced it.
    set((state) => {
      const present = state.conversations.some((c) => c.id === seeded.id);
      return {
        conversations: present
          ? state.conversations.map((c) => (c.id === seeded.id ? seeded : c))
          : [...state.conversations, seeded],
      };
    });
    return seeded;
  },

  exportConversation: (conversationId, format) => {
    return exportConversationCmd(conversationId, format);
  },

  setPendingOverride: (route) => set({ pendingOverride: route }),

  setWebSearchEnabled: (enabled) => set({ webSearchEnabled: enabled }),

  setThinkingEnabled: (enabled) => set({ thinkingEnabled: enabled }),

  addAttachment: (attachment) =>
    set((state) => {
      // Dedupe by path + kind so re-picking the same file is idempotent.
      const exists = state.attachments.some(
        (a) => a.path === attachment.path && a.kind === attachment.kind,
      );
      if (exists) return {};
      return { attachments: [...state.attachments, attachment] };
    }),

  removeAttachment: (path, kind) =>
    set((state) => ({
      attachments: state.attachments.filter((a) => !(a.path === path && a.kind === kind)),
    })),

  clearAttachments: () => set({ attachments: [] }),

  consumePendingOverride: () => {
    const { pendingOverride } = get();
    set({ pendingOverride: null });
    return pendingOverride;
  },

  sendMessage: async (content) => {
    const { activeConversationId } = get();
    if (activeConversationId === null) return;
    // Consume-and-clear the transient override so it applies to one message.
    const override = get().consumePendingOverride();
    // Mark the attempt in flight and clear any prior failure so a retry starts
    // clean. Capture a REJECTION into `sendState`/`sendError` instead of letting
    // it escape: the Composer fires this as `void sendMessage(...)`, so a thrown
    // error would otherwise be swallowed and leave NO signal (the silent no-op
    // bug). Mirror providers.ts load() error extraction. On success reset to
    // idle; the assistant reply then arrives via streaming CoreEvents.
    set({ sendState: "sending", sendError: null });
    try {
      await sendMessageCmd(activeConversationId, content, override);
      // Clear the one-turn attachments on a SUCCESSFUL send, mirroring how the
      // transient override is consumed once (FEAT-003). On failure they are
      // left in place so the user can retry without re-attaching.
      set({ sendState: "idle", sendError: null, attachments: [] });
    } catch (e) {
      set({ sendState: "failed", sendError: e instanceof Error ? e.message : String(e) });
    }
  },

  stopGeneration: async () => {
    const { activeConversationId } = get();
    if (activeConversationId === null) return;
    await stopGenerationCmd(activeConversationId);
  },

  regenerateLastTurn: async () => {
    const { activeConversationId, messages } = get();
    if (activeConversationId === null) return;
    // Find the last assistant message, then the most recent USER message that
    // precedes it - that user turn is what we re-run. Reuse its text content
    // through the ordinary send path so the turn is re-generated as a fresh
    // turn (see the action doc: this appends, it does not replace in place).
    const lastAssistantIndex = messages.map((m) => m.role).lastIndexOf("assistant");
    if (lastAssistantIndex === -1) return;
    let priorUser: Message | undefined;
    for (let i = lastAssistantIndex - 1; i >= 0; i -= 1) {
      if (messages[i].role === "user") {
        priorUser = messages[i];
        break;
      }
    }
    if (priorUser === undefined || priorUser.content.type !== "text") return;
    await get().sendMessage(priorUser.content.text);
  },

  editAndResend: async (messageId, newContent) => {
    const { activeConversationId, messages } = get();
    if (activeConversationId === null) return;
    // Only the LAST user message is editable in this scope (deeper branching is
    // deferred - see the action doc). Ignore an edit targeting anything else so
    // the affordance cannot silently rewrite an arbitrary earlier turn.
    const lastUserIndex = messages.map((m) => m.role).lastIndexOf("user");
    if (lastUserIndex === -1 || messages[lastUserIndex].id !== messageId) return;
    const content = newContent.trim();
    if (content.length === 0) return;
    await get().sendMessage(content);
  },

  continueTurn: async () => {
    const { activeConversationId } = get();
    if (activeConversationId === null) return;
    // Honest follow-up, NOT a resume: the pipeline has no cancel/resume seam, so
    // ask the model to continue in a fresh turn via the ordinary send path.
    await get().sendMessage("Please continue.");
  },

  resolvePermission: async (requestId, decision) => {
    // Forward the Ask-mode decision to the core so its blocked tool invocation
    // can proceed (architecture.md Section 9.4), then dequeue the prompt.
    await resolvePermissionCmd(requestId, decision);
    set((state) => ({
      pendingPermissions: state.pendingPermissions.filter((p) => p.requestId !== requestId),
    }));
  },

  applyCoreEvent: (event) => {
    switch (event.type) {
      case "messageStarted": {
        const { activeConversationId } = get();
        if (event.conversationId !== activeConversationId) return;
        // Seed a placeholder keyed on the event's messageId so the following
        // messageDelta/messageComplete events (which reference an id the store
        // has never seen) land on a real message instead of being dropped.
        set((state) => {
          if (state.messages.some((m) => m.id === event.messageId)) {
            return {};
          }
          return {
            messages: [
              ...state.messages,
              placeholderMessage(event.conversationId, event.messageId, event.role, event.text),
            ],
          };
        });
        return;
      }
      case "messageDelta": {
        const { activeConversationId } = get();
        if (event.conversationId !== activeConversationId) return;
        set((state) => ({
          messages: appendDelta(state.messages, event.messageId, event.delta),
        }));
        return;
      }
      case "messageThinkingDelta": {
        const { activeConversationId } = get();
        if (event.conversationId !== activeConversationId) return;
        set((state) => ({
          messages: appendThinking(state.messages, event.messageId, event.delta),
        }));
        return;
      }
      case "messageComplete": {
        const { activeConversationId } = get();
        if (event.conversationId !== activeConversationId) return;
        set((state) => ({
          messages: state.messages.map((m) =>
            m.id === event.messageId
              ? { ...m, status: event.status, route: event.route, usage: event.usage }
              : m,
          ),
        }));
        return;
      }
      case "messageError": {
        const { activeConversationId } = get();
        if (event.conversationId !== activeConversationId) return;
        set((state) => {
          const index = state.messages.findIndex((m) => m.id === event.messageId);
          // The errored message was never seeded (no placeholder): append a new
          // assistant bubble carrying the reason so the failure is never
          // dropped (a send that fails before any messageStarted arrives).
          if (index === -1) {
            const errored: Message = {
              id: event.messageId,
              conversationId: event.conversationId,
              role: "assistant",
              content: { type: "text", text: event.message },
              createdAt: new Date().toISOString(),
              route: null,
              usage: null,
              status: "error",
            };
            return { messages: [...state.messages, errored] };
          }
          const message = state.messages[index];
          // Preserve a fully streamed reply that then errored on finalize (keep
          // its text, just flag the error); only surface the reason text when
          // the content is an empty streaming placeholder that would otherwise
          // show nothing.
          const isEmptyText = message.content.type === "text" && message.content.text.length === 0;
          const content: MessageContent = isEmptyText
            ? { type: "text", text: event.message }
            : message.content;
          const next = state.messages.slice();
          next[index] = { ...message, content, status: "error" };
          return { messages: next };
        });
        return;
      }
      case "conversationCreated":
      case "conversationUpdated": {
        // Core is authoritative: invalidate + refetch the index (recency,
        // titles, persona / route / tag changes surface here).
        void get().loadConversations();
        // Deliberately do NOT refetch the active conversation's messages here.
        // `run_turn` emits `conversationUpdated` after EACH message persist, so
        // a message refetch would resolve mid-stream — while only the user row
        // is persisted and the assistant row is not yet written — and its
        // `set({ messages })` would overwrite the seeded streaming placeholder
        // and every accumulated delta, blanking the assistant bubble. The live
        // streaming events (messageStarted/messageDelta/messageComplete) keep
        // the active message list current, so an event-driven refetch is both
        // unnecessary and harmful. User-initiated opens (openConversation /
        // resumeConversation from the history surface) still refetch on demand.
        return;
      }
      case "conversationDeleted": {
        set((state) => ({
          conversations: state.conversations.filter((c) => c.id !== event.conversationId),
          activeConversationId:
            state.activeConversationId === event.conversationId ? null : state.activeConversationId,
          messages: state.activeConversationId === event.conversationId ? [] : state.messages,
        }));
        return;
      }
      case "permissionRequested": {
        set((state) => ({
          pendingPermissions: [
            ...state.pendingPermissions,
            {
              requestId: event.requestId,
              serverId: event.serverId,
              toolName: event.toolName,
              mode: event.mode,
              rationale: event.rationale,
            },
          ],
        }));
        return;
      }
      // Events handled by other stores (providers/tools/personas) or with no
      // conversation-store effect are ignored here.
      case "mcpStateChanged":
      case "mcpError":
      case "providersChanged":
      case "personasChanged":
        return;
    }
  },
}));
