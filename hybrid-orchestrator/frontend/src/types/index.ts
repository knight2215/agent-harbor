// Shared TypeScript types mirrored BY HAND from the Rust serde DTOs in the
// leaf `domain` crate (crates/domain/src/models.rs), which implement
// architecture.md Section 7.1 (data models), 4.2 (ProviderConfig), and 6.1
// (ManualRoute / PrivacyTag / RouteSource). `orchestrator-core` re-exports these
// DTOs, so the Rust side is also reachable as `orchestrator_core::Conversation`
// etc.; the serde representation (and therefore this mirror) is unchanged by the
// relocation.
//
// There is NO codegen: these interfaces must be kept in sync with the Rust DTOs
// manually. The Rust side uses `#[serde(rename_all = "camelCase")]`, so every
// field name here is camelCase to match the JSON that crosses the Tauri IPC
// boundary. When you change a model on either side, update the other in the
// same change.
//
// Conventions:
//   - Rust `Uuid`            -> string (UUID text form)
//   - Rust `DateTime<Utc>`   -> string (RFC 3339 / ISO 8601 timestamp)
//   - Rust `Option<T>`       -> `T | null` (serde serializes `None` as `null`)
//   - Rust `serde_json::Value` -> unknown
//   - internally-tagged enums (`#[serde(tag = "type", rename_all = "camelCase")]`)
//     -> discriminated unions on a `type` field
//   - unit enums (`#[serde(rename_all = "camelCase")]`) -> string literal unions
//   - `PrivacyTag::Custom(String)` (externally-tagged data variant)
//     -> `{ custom: string }`

/** Opaque reference into the OS keychain. Never contains secret material. */
export type SecretRef = string;

/** Message author role. Mirrors Rust `Role`. */
export type Role = "system" | "user" | "assistant" | "tool";

/** Lifecycle status of a message. Mirrors Rust `MessageStatus`. */
export type MessageStatus = "pending" | "streaming" | "complete" | "error";

/** How a route was decided. Mirrors Rust `RouteSource`. */
export type RouteSource = "manual" | "conversationPin" | "automatic";

/** Coarse routing preference expressed by a persona. Mirrors Rust `RoutingHint`. */
export type RoutingHint = "preferLocal" | "preferQuality" | "preferCheap" | "preferSpeed";

/** The supported provider families. Mirrors Rust `ProviderKind`. */
export type ProviderKind =
  "openAI" | "anthropic" | "bedrock" | "gemini" | "azure" | "lmStudio" | "genericOpenAI";

/** Permission gate applied to an MCP server's tool invocations. Mirrors Rust `PermissionMode`. */
export type PermissionMode = "ask" | "allow" | "deny";

/**
 * The user's answer to a pending `permissionRequested` prompt, passed to the
 * `resolve_permission` command. Mirrors Rust `orchestrator_core::Decision`
 * (architecture.md Section 5.6 / 9.4). `remember` (default `false`) asks to
 * remember the answer for the session.
 */
export interface PermissionDecision {
  allow: boolean;
  remember: boolean;
}

/**
 * Data-handling constraint tag. Mirrors Rust `PrivacyTag`.
 * `localOnly` / `confidential` are hard constraints that force local routing;
 * `Custom(String)` is serialized externally-tagged as `{ custom: string }`.
 */
export type PrivacyTag = "localOnly" | "confidential" | { custom: string };

/** A manual provider/model selection. Mirrors Rust `ManualRoute`. */
export interface ManualRoute {
  providerId: string;
  model: string;
}

/** A single tool/function call requested by the model. Mirrors Rust `ToolCall`. */
export interface ToolCall {
  id: string;
  name: string;
  arguments: unknown;
}

/** The result of invoking a tool. Mirrors Rust `ToolResult`. */
export interface ToolResult {
  callId: string;
  content: unknown;
  isError: boolean;
}

/** A non-text attachment carried with a message. Mirrors Rust `Attachment`. */
export interface Attachment {
  mimeType: string;
  name: string | null;
  uri: string;
}

/**
 * The body of a message. Mirrors Rust `MessageContent`, an internally-tagged
 * enum discriminated on `type`.
 */
export type MessageContent =
  | { type: "text"; text: string }
  | { type: "toolCalls"; calls: ToolCall[] }
  | { type: "toolResults"; results: ToolResult[] }
  | { type: "attachments"; attachments: Attachment[] };

/** Which provider/model answered a message and why. Mirrors Rust `RouteMetadata`. */
export interface RouteMetadata {
  providerId: string;
  model: string;
  rationale: string;
  source: RouteSource;
}

/** Token accounting for a completed message. Mirrors Rust `TokenUsage`. */
export interface TokenUsage {
  promptTokens: number;
  completionTokens: number;
  totalTokens: number;
}

/** Model sampling / generation parameters. Mirrors Rust `ModelParameters`. */
export interface ModelParameters {
  temperature: number | null;
  maxTokens: number | null;
  topP: number | null;
  frequencyPenalty: number | null;
  presencePenalty: number | null;
  stop: string[] | null;
}

/** A conversation and its per-conversation settings. Mirrors Rust `Conversation`. */
export interface Conversation {
  id: string;
  title: string;
  createdAt: string;
  updatedAt: string;
  personaId: string | null;
  conversationPref: ManualRoute | null;
  privacyTags: PrivacyTag[];
  enabledToolServers: string[];
}

/** A single message within a conversation. Mirrors Rust `Message`. */
export interface Message {
  id: string;
  conversationId: string;
  role: Role;
  content: MessageContent;
  createdAt: string;
  route: RouteMetadata | null;
  usage: TokenUsage | null;
  status: MessageStatus;
}

/** A saved agent persona. Mirrors Rust `AgentPersona`. */
export interface AgentPersona {
  id: string;
  name: string;
  systemPrompt: string;
  defaultRoute: ManualRoute | null;
  routingHint: RoutingHint | null;
  allowedToolServers: string[];
  parameters: ModelParameters;
}

/**
 * How the core connects to an MCP server. Mirrors Rust `McpTransport`, an
 * internally-tagged enum discriminated on `type`. `env` and `headers` are
 * serialized from Rust `Vec<(String, String)>` as arrays of `[key, value]`
 * tuples.
 */
export type McpTransport =
  | { type: "stdio"; command: string; args: string[]; env: Array<[string, string]> }
  | { type: "httpSse"; url: string; headers: Array<[string, string]> };

/** Configuration for a single MCP tool server. Mirrors Rust `McpServerConfig`. */
export interface McpServerConfig {
  id: string;
  name: string;
  transport: McpTransport;
  permissionMode: PermissionMode;
  enabled: boolean;
}

/** Provider adapter construction config. Mirrors Rust `ProviderConfig` (Section 4.2). */
export interface ProviderConfig {
  id: string;
  kind: ProviderKind;
  baseUrl: string | null;
  apiKeyRef: SecretRef | null;
  extra: unknown;
}

/**
 * Per-model capability descriptor. Mirrors Rust `providers::Capabilities`
 * (architecture.md Section 4.1). Rust uses `#[serde(rename_all = "camelCase")]`,
 * so `json_mode` -> `jsonMode` and `max_context` -> `maxContext`.
 */
export interface Capabilities {
  streaming: boolean;
  tools: boolean;
  vision: boolean;
  jsonMode: boolean;
  maxContext: number | null;
}

/** Metadata about a model a provider offers. Mirrors Rust `providers::ModelInfo` (Section 4.1). */
export interface ModelInfo {
  id: string;
  displayName: string | null;
  contextLength: number | null;
}

/**
 * Per-model token pricing in currency units per one million tokens. Mirrors
 * Rust `providers::TokenPrice` / `persistence::TokenRate` (architecture.md
 * Section 6.2). Local providers default to zero.
 */
export interface TokenPrice {
  inputPerMtok: number;
  outputPerMtok: number;
}

/**
 * One selectable model surfaced by the `list_available_models` command. Mirrors
 * Rust `providers::AvailableModel` (architecture.md Section 6.1 / 8.2).
 *
 * DISPLAY-SAFE: carries only provider/model/capabilities/price labels, never
 * secret material (Section 9.1 / 9.2).
 */
export interface AvailableModel {
  providerId: string;
  model: string;
  capabilities: Capabilities;
  price: TokenPrice;
}

/**
 * User-entered per-provider/per-model token-rate table stored in the versioned
 * app config. Mirrors Rust `persistence::PricingConfig` (architecture.md Section
 * 6.2). Keys are the camelCase `ProviderKind` serialization; empty maps are
 * omitted from the persisted JSON. This is the single source both the cost
 * signal and `list_available_models` read.
 */
export interface PricingConfig {
  perKind: Partial<Record<ProviderKind, TokenPrice>>;
  perModel: Partial<Record<ProviderKind, Record<string, TokenPrice>>>;
  lastEdited: string | null;
}

/** Connection state of an MCP server. Mirrors Rust `McpConnectionState`. */
export type McpConnectionState = "connecting" | "connected" | "disconnected";

/**
 * Input for the `add_mcp_server` / `update_mcp_server` commands. Mirrors Rust
 * `McpServerInput` (the persisted `McpServerConfig` minus the id, which the
 * command assigns on add or takes from the path on update). `transport` uses the
 * exact `McpTransport` serde representation (`env`/`headers` as `[key, value]`
 * tuples).
 */
export interface McpServerInput {
  name: string;
  transport: McpTransport;
  permissionMode: PermissionMode;
  enabled: boolean;
}

/**
 * Display-safe view of an MCP tool descriptor returned by `refresh_mcp_tools`.
 * Mirrors Rust `ToolDescriptorView` (architecture.md Section 5.4 / 8.3). Carries
 * only the tool name, description, and its JSON Schema; never secret material.
 */
export interface ToolDescriptorView {
  name: string;
  description: string;
  inputSchema: unknown;
}

/**
 * A display-safe preview of how the active conversation is routed, returned by
 * `get_route_explanation` (architecture.md Section 8.2). Mirrors Rust
 * `RouteExplanation`. `providerId` / `model` are omitted (undefined) when the
 * route is automatic and no prior message has recorded a decision yet.
 */
export interface RouteExplanation {
  rationale: string;
  providerId?: string;
  model?: string;
  source: RouteSource;
}

/** Serialization format for `export_conversation`. Mirrors Rust `ExportFormat`. */
export type ExportFormat = "markdown" | "json";

/**
 * The resume payload returned by `open_conversation` (architecture.md Section
 * 8.5). Mirrors Rust `OpenedConversation`: the conversation plus its message
 * history in one round-trip so the chat surface can restore the full session.
 */
export interface OpenedConversation {
  conversation: Conversation;
  messages: Message[];
}

/**
 * Events emitted by the core to the frontend (architecture.md Section 8).
 * Mirrors Rust `CoreEvent`, an internally-tagged enum discriminated on `type`.
 *
 * Event hygiene (Section 9.1 / 9.2): payloads carry only display-safe data
 * (ids, deltas, statuses, rationales), never secrets or credentials.
 */
export type CoreEvent =
  | { type: "messageStarted"; conversationId: string; messageId: string; role: Role }
  | { type: "messageDelta"; conversationId: string; messageId: string; delta: string }
  | {
      type: "messageComplete";
      conversationId: string;
      messageId: string;
      status: MessageStatus;
      route: RouteMetadata | null;
      usage: TokenUsage | null;
    }
  | { type: "messageError"; conversationId: string; messageId: string; message: string }
  | { type: "conversationUpdated"; conversationId: string }
  | { type: "conversationCreated"; conversationId: string }
  | { type: "conversationDeleted"; conversationId: string }
  | { type: "mcpStateChanged"; serverId: string; state: McpConnectionState }
  | { type: "mcpError"; serverId: string; message: string }
  | {
      type: "permissionRequested";
      requestId: string;
      serverId: string;
      toolName: string;
      mode: PermissionMode;
      rationale: string;
    }
  | { type: "providersChanged" }
  | { type: "personasChanged" };
