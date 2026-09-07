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
} from "../types";

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
  /** Queue of pending Ask-mode permission requests (Section 9.4). */
  pendingPermissions: PendingPermission[];

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
 * Build a placeholder [`Message`] for a `messageStarted` event so the chat
 * surface shows the message before any delta arrives (architecture.md Section
 * 7.4). The user message lands `complete`; the assistant reply lands
 * `streaming` and accumulates deltas.
 */
function placeholderMessage(conversationId: string, messageId: string, role: Role): Message {
  return {
    id: messageId,
    conversationId,
    role,
    content: { type: "text", text: "" },
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
  pendingPermissions: [],

  loadConversations: async () => {
    const conversations = await listConversations();
    set({ conversations });
  },

  openConversation: async (conversationId) => {
    const messages = await getMessages(conversationId);
    set({ activeConversationId: conversationId, messages });
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
      };
    });
    return conversation;
  },

  createConversation: async (title) => {
    const conversation = await createConversationCmd(title ? { title } : {});
    set((state) => ({ conversations: [...state.conversations, conversation] }));
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
    set((state) => ({
      conversations: state.conversations.filter((c) => c.id !== conversationId),
      activeConversationId:
        state.activeConversationId === conversationId ? null : state.activeConversationId,
      messages: state.activeConversationId === conversationId ? [] : state.messages,
    }));
  },

  setConversationRoute: async (conversationId, route) => {
    const updated = await setConversationRouteCmd(conversationId, route);
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
    set((state) => ({ conversations: [...state.conversations, seeded] }));
    return seeded;
  },

  exportConversation: (conversationId, format) => {
    return exportConversationCmd(conversationId, format);
  },

  setPendingOverride: (route) => set({ pendingOverride: route }),

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
    await sendMessageCmd(activeConversationId, content, override);
  },

  stopGeneration: async () => {
    const { activeConversationId } = get();
    if (activeConversationId === null) return;
    await stopGenerationCmd(activeConversationId);
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
              placeholderMessage(event.conversationId, event.messageId, event.role),
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
        set((state) => ({
          messages: state.messages.map((m) =>
            m.id === event.messageId ? { ...m, status: "error" } : m,
          ),
        }));
        return;
      }
      case "conversationCreated":
      case "conversationUpdated": {
        // Core is authoritative: invalidate + refetch the index.
        void get().loadConversations();
        // If the active conversation changed, refetch its messages too.
        if (get().activeConversationId === event.conversationId) {
          void get().openConversation(event.conversationId);
        }
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
