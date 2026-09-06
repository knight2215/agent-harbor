import { invoke } from "@tauri-apps/api/core";
import type {
  AgentPersona,
  Conversation,
  ManualRoute,
  ModelParameters,
  PrivacyTag,
  RoutingHint,
  SecretRef,
} from "../types";

/**
 * Typed wrappers over Tauri `invoke()` commands.
 *
 * Each function mirrors a `#[tauri::command]` handler in the Rust shell
 * (`crates/tauri-app/src/commands.rs`) and is the only sanctioned way the
 * frontend crosses the IPC boundary (architecture.md Section 9.2). Argument
 * names/shapes match the Rust handler signatures; the Rust side validates every
 * argument (types, enum membership, id parse, bounds) before touching the core
 * and rejects invalid input with a structured error.
 *
 * SECRET HYGIENE (Section 9.1): `setProviderSecret` sends the plaintext key IN
 * and receives back ONLY a `SecretRef` handle. No command returns a secret to
 * the webview.
 */

/**
 * Fetch the application version compiled into the Tauri binary.
 *
 * Backed by the `app_version` command.
 */
export function appVersion(): Promise<string> {
  return invoke<string>("app_version");
}

// --- Conversations ----------------------------------------------------------

/** Arguments for {@link createConversation}. Mirrors Rust `CreateConversationArgs`. */
export interface CreateConversationArgs {
  title?: string | null;
  personaId?: string | null;
  privacyTags?: PrivacyTag[];
}

/** List all conversations (history surface). */
export function listConversations(): Promise<Conversation[]> {
  return invoke<Conversation[]>("list_conversations");
}

/** Create a new conversation. */
export function createConversation(args: CreateConversationArgs = {}): Promise<Conversation> {
  return invoke<Conversation>("create_conversation", { args });
}

/** Rename a conversation. */
export function renameConversation(conversationId: string, title: string): Promise<Conversation> {
  return invoke<Conversation>("rename_conversation", { conversationId, title });
}

/** Set (replace) a conversation's privacy tags. */
export function setConversationTags(
  conversationId: string,
  tags: PrivacyTag[],
): Promise<Conversation> {
  return invoke<Conversation>("set_conversation_tags", { conversationId, tags });
}

/** Delete a conversation (and, via cascade, its messages). */
export function deleteConversation(conversationId: string): Promise<void> {
  return invoke<void>("delete_conversation", { conversationId });
}

// --- Personas ---------------------------------------------------------------

/** Input for {@link createPersona} / {@link updatePersona}. Mirrors Rust `PersonaInput`. */
export interface PersonaInput {
  name: string;
  systemPrompt: string;
  defaultRoute?: ManualRoute | null;
  routingHint?: RoutingHint | null;
  allowedToolServers?: string[];
  parameters?: ModelParameters;
}

/** List all saved personas. */
export function listPersonas(): Promise<AgentPersona[]> {
  return invoke<AgentPersona[]>("list_personas");
}

/** Create a new persona. */
export function createPersona(persona: PersonaInput): Promise<AgentPersona> {
  return invoke<AgentPersona>("create_persona", { persona });
}

/** Update an existing persona. */
export function updatePersona(personaId: string, persona: PersonaInput): Promise<AgentPersona> {
  return invoke<AgentPersona>("update_persona", { personaId, persona });
}

/** Delete a persona by id. */
export function deletePersona(personaId: string): Promise<void> {
  return invoke<void>("delete_persona", { personaId });
}

// --- Secret entry (P1.7) ----------------------------------------------------

/**
 * Store a provider API key in the OS keystore and receive back ONLY its opaque
 * {@link SecretRef} handle (architecture.md Section 9.1). The plaintext key
 * flows IN; the return value is never the secret. No other command returns key
 * material, and no event payload carries it.
 */
export function setProviderSecret(providerId: string, secret: string): Promise<SecretRef> {
  return invoke<SecretRef>("set_provider_secret", { providerId, secret });
}

// --- Message pipeline (P4.6 / Section 8.1) ----------------------------------

/**
 * Send a user message and drive the end-to-end pipeline (architecture.md
 * Section 8.1). The assistant response is delivered by STREAMING core events
 * (`messageDelta` / `messageComplete` / `messageError`) over the event bridge,
 * not through this call's return value: the command validates its arguments,
 * kicks off the pipeline turn, and resolves once the turn is under way.
 *
 * `overrideRoute` is the optional per-message manual override (Section 6.3,
 * highest precedence); omit it (or pass `null`) for automatic routing.
 */
export function sendMessage(
  conversationId: string,
  content: string,
  overrideRoute?: ManualRoute | null,
): Promise<void> {
  return invoke<void>("send_message", { conversationId, content, overrideRoute });
}
