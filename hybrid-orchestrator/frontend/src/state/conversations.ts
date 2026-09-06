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
  getMessages,
  listConversations,
  renameConversation as renameConversationCmd,
  sendMessage as sendMessageCmd,
  setConversationRoute as setConversationRouteCmd,
  setConversationTags as setConversationTagsCmd,
  stopGeneration as stopGenerationCmd,
} from "../ipc/commands";
import type {
  Conversation,
  CoreEvent,
  ManualRoute,
  Message,
  MessageContent,
  PermissionDecision,
  PermissionMode,
  PrivacyTag,
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

  // --- Mutations (delegate to the core, then reflect the result) -----------
  createConversation: (title?: string) => Promise<Conversation>;
  renameConversation: (conversationId: string, title: string) => Promise<void>;
  setConversationTags: (conversationId: string, tags: PrivacyTag[]) => Promise<void>;
  deleteConversation: (conversationId: string) => Promise<void>;
  setConversationRoute: (conversationId: string, route: ManualRoute | null) => Promise<void>;
  assignPersona: (conversationId: string, personaId: string | null) => Promise<void>;

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
   * Resolve a pending Ask-mode permission request (Section 9.4). Dequeues the
   * request; `decision` carries the user's `{ allow, remember }` answer.
   */
  resolvePermission: (requestId: string, decision: PermissionDecision) => void;

  // --- Event application (core authoritative, Section 7.3) -----------------
  applyCoreEvent: (event: CoreEvent) => void;
}

/** Append a text delta to a streaming message, creating it if it is not present. */
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

  resolvePermission: (requestId, decision) => {
    // The Ask-mode decision is surfaced to the core through the pipeline's
    // permission channel; there is no dedicated `resolve_permission` IPC command
    // in this phase, so the store dequeues locally. Reference `decision` so the
    // full { allow, remember } contract is part of the public store API that the
    // PermissionPrompt buttons call (architecture.md Section 9.4).
    void decision;
    set((state) => ({
      pendingPermissions: state.pendingPermissions.filter((p) => p.requestId !== requestId),
    }));
  },

  applyCoreEvent: (event) => {
    switch (event.type) {
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
