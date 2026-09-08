import { invoke } from "@tauri-apps/api/core";
import type {
  AgentPersona,
  AvailableModel,
  Conversation,
  EmbeddedModelStatus,
  EmbeddedModelView,
  ExportFormat,
  ManualRoute,
  McpServerConfig,
  McpServerInput,
  Message,
  ModelParameters,
  OpenedConversation,
  PermissionDecision,
  PermissionMode,
  PrivacyTag,
  RouteExplanation,
  RoutingHint,
  RoutingMode,
  SecretRef,
  ToolDescriptorView,
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

// --- Model selector data (P2.10 / Section 8.2) ------------------------------

/**
 * List every available model across all configured providers, with per-model
 * capabilities and price, for the model selector (architecture.md Section 8.2).
 * DISPLAY-SAFE: the rows never carry secret material. Backed by
 * `list_available_models`.
 */
export function listAvailableModels(): Promise<AvailableModel[]> {
  return invoke<AvailableModel[]>("list_available_models");
}

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

/**
 * Fetch the ordered message history for a conversation (architecture.md Section
 * 8.1 chat surface; also used on resume, Section 8.5). Backed by `get_messages`.
 */
export function getMessages(conversationId: string): Promise<Message[]> {
  return invoke<Message[]>("get_messages", { conversationId });
}

/**
 * Pin (or clear) the per-conversation route (architecture.md Section 6.3 / 8.2).
 * Pass `route = null` to return the conversation to Automatic routing. Backed by
 * `set_conversation_route`.
 *
 * NOTE: the per-MESSAGE override is NOT a command; per Section 8.2 it is a
 * transient value owned by the conversations store and passed to
 * {@link sendMessage}'s `overrideRoute` argument, then cleared after the send.
 */
export function setConversationRoute(
  conversationId: string,
  route: ManualRoute | null,
): Promise<Conversation> {
  return invoke<Conversation>("set_conversation_route", { conversationId, route });
}

/**
 * Set (or clear) the per-conversation routing mode (FEAT-002). Pass
 * `mode = null` (or `"auto"`) to return the conversation to automatic routing;
 * `"preferLocal"` / `"preferQuality"` bias the auto policy; `"manual"` pairs
 * with {@link setConversationRoute} to pin a specific provider/model. Backed by
 * `set_conversation_routing_mode`, mirroring {@link setConversationRoute}.
 */
export function setConversationRoutingMode(
  conversationId: string,
  mode: RoutingMode | null,
): Promise<Conversation> {
  return invoke<Conversation>("set_conversation_routing_mode", { conversationId, mode });
}

/**
 * Assign (or clear) a persona for a conversation (architecture.md Section 8.4 /
 * 8.5). Pass `personaId = null` to detach. Backed by `assign_persona`.
 */
export function assignPersona(
  conversationId: string,
  personaId: string | null,
): Promise<Conversation> {
  return invoke<Conversation>("assign_persona", { conversationId, personaId });
}

/**
 * Explain how the active conversation is routed, for the "why this model"
 * tooltip (architecture.md Section 8.2). Backed by `get_route_explanation`.
 */
export function getRouteExplanation(conversationId: string): Promise<RouteExplanation> {
  return invoke<RouteExplanation>("get_route_explanation", { conversationId });
}

// --- MCP server management (Section 8.3) ------------------------------------

/** List every configured MCP server. Backed by `list_mcp_servers`. */
export function listMcpServers(): Promise<McpServerConfig[]> {
  return invoke<McpServerConfig[]>("list_mcp_servers");
}

/** Add a new MCP server (connects in the background when enabled). Backed by `add_mcp_server`. */
export function addMcpServer(config: McpServerInput): Promise<McpServerConfig> {
  return invoke<McpServerConfig>("add_mcp_server", { config });
}

/** Update an existing MCP server (reconnects when enabled). Backed by `update_mcp_server`. */
export function updateMcpServer(id: string, config: McpServerInput): Promise<McpServerConfig> {
  return invoke<McpServerConfig>("update_mcp_server", { id, config });
}

/** Remove an MCP server (tears down its handle). Backed by `remove_mcp_server`. */
export function removeMcpServer(id: string): Promise<void> {
  return invoke<void>("remove_mcp_server", { id });
}

/** Enable or disable an MCP server (connect/teardown + persist). Backed by `set_mcp_enabled`. */
export function setMcpEnabled(id: string, enabled: boolean): Promise<McpServerConfig> {
  return invoke<McpServerConfig>("set_mcp_enabled", { id, enabled });
}

/** Refresh an MCP server's tool list (reconnect + re-list). Backed by `refresh_mcp_tools`. */
export function refreshMcpTools(id: string): Promise<ToolDescriptorView[]> {
  return invoke<ToolDescriptorView[]>("refresh_mcp_tools", { id });
}

/**
 * Set an MCP server's tool-invocation permission mode (architecture.md Section
 * 5.6 / 8.3). `toolName` is accepted for forward compatibility but the Phase 5
 * schema persists only the per-server mode. Backed by `set_tool_permission`.
 */
export function setToolPermission(
  serverId: string,
  toolName: string | null,
  mode: PermissionMode,
): Promise<McpServerConfig> {
  return invoke<McpServerConfig>("set_tool_permission", { serverId, toolName, mode });
}

// --- Conversation export / resume / cancellation (Section 8.1 / 8.5) --------

/** Export a conversation to a display-safe string. Backed by `export_conversation`. */
export function exportConversation(conversationId: string, format: ExportFormat): Promise<string> {
  return invoke<string>("export_conversation", { conversationId, format });
}

/** Load a conversation and its messages for resume. Backed by `open_conversation`. */
export function openConversation(conversationId: string): Promise<OpenedConversation> {
  return invoke<OpenedConversation>("open_conversation", { conversationId });
}

/**
 * Request cancellation of an in-flight generation (architecture.md Section 8.1
 * Composer stop button). The Phase 4 pipeline has no cancellation seam yet, so
 * this is a validated no-op on the backend; the command exists so the UI's stop
 * control has something to call. Backed by `stop_generation`.
 */
export function stopGeneration(conversationId: string): Promise<void> {
  return invoke<void>("stop_generation", { conversationId });
}

/**
 * Resolve a pending Ask-mode tool-permission request (architecture.md Section
 * 9.4). The core blocks the tool invocation until the user answers; this
 * forwards the request's `requestId` and the user's `{ allow, remember }`
 * {@link PermissionDecision} to unblock it. Returns `true` if a request with
 * `requestId` was awaiting a decision (now unblocked), `false` otherwise
 * (already resolved, timed out, or an unknown id). Backed by `resolve_permission`.
 */
export function resolvePermission(
  requestId: string,
  decision: PermissionDecision,
): Promise<boolean> {
  return invoke<boolean>("resolve_permission", { requestId, decision });
}

// --- Embedded local inference engine (Strategy B / FEAT-002) ----------------

/**
 * List the imported embedded (local `.gguf`) models (architecture.md Section 4,
 * Strategy B). DISPLAY-SAFE: the rows carry only id/path/loaded, never secret
 * material. Backed by `list_embedded_models`. The picker / LocalRuntimes UI that
 * calls this lands in FEAT-003.
 */
export function listEmbeddedModels(): Promise<EmbeddedModelView[]> {
  return invoke<EmbeddedModelView[]>("list_embedded_models");
}

/**
 * Import a local `.gguf` model by path, registering it with the embedded engine
 * (architecture.md Section 4, Strategy B). Returns the refreshed model list.
 * Backed by `import_embedded_model`.
 */
export function importEmbeddedModel(path: string): Promise<EmbeddedModelView[]> {
  return invoke<EmbeddedModelView[]>("import_embedded_model", { path });
}

/**
 * Select (import if needed, then mark active) a local `.gguf` model by path
 * (architecture.md Section 4, Strategy B). Returns the refreshed model list.
 * Backed by `select_embedded_model`.
 */
export function selectEmbeddedModel(path: string): Promise<EmbeddedModelView[]> {
  return invoke<EmbeddedModelView[]>("select_embedded_model", { path });
}

/**
 * Load (make active) an already-imported embedded model by id (architecture.md
 * Section 4, Strategy B). Returns the engine status. Backed by
 * `load_embedded_model`.
 */
export function loadEmbeddedModel(modelId: string): Promise<EmbeddedModelStatus> {
  return invoke<EmbeddedModelStatus>("load_embedded_model", { modelId });
}

/**
 * Unload the currently selected/loaded embedded model (architecture.md Section
 * 4, Strategy B), clearing the active-model state. Returns the engine status.
 * Backed by `unload_embedded_model`.
 */
export function unloadEmbeddedModel(): Promise<EmbeddedModelStatus> {
  return invoke<EmbeddedModelStatus>("unload_embedded_model");
}

/**
 * Report the embedded engine's lifecycle status (architecture.md Section 4,
 * Strategy B): which model (if any) is selected/loaded and how many are
 * imported. Backed by `embedded_model_status`.
 */
export function embeddedModelStatus(): Promise<EmbeddedModelStatus> {
  return invoke<EmbeddedModelStatus>("embedded_model_status");
}
