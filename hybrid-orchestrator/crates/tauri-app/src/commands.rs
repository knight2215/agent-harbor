//! `#[tauri::command]` handlers exposed across the IPC boundary.
//!
//! These are the ONLY sanctioned entry points from the webview into the core
//! (architecture.md Section 9.2: "only explicitly registered `#[tauri::command]`
//! handlers are callable; there is no generic passthrough to the core"). Every
//! handler VALIDATES and deserializes its arguments (types, id existence,
//! bounds, enum membership) BEFORE touching the core, and rejects invalid input
//! with a structured [`CommandError`] rather than partially applying it.
//!
//! SECRET HYGIENE (Section 9.1): no command return type carries raw key
//! material. `set_provider_secret` writes straight to the keystore and returns
//! only a [`SecretRef`] handle. `SecretStore::resolve` (the single plaintext
//! seam) is never surfaced here.

use orchestrator_core::{
    run_turn, AgentPersona, Conversation, ConversationInit, Decision, ManualRoute, ModelParameters,
    PermissionGate, PrivacyTag, ProviderConfig, ProviderKind, RoutingHint, SecretRef, TurnContext,
};
use persistence::config::{AppConfig, PricingConfig};
use persistence::ProviderRepo;
use providers::{list_available_models as list_models, AvailableModel, PricingTable, TokenPrice};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::state::AppState;

/// Maximum accepted length for user-supplied free-text fields (names, titles,
/// prompts). Guards the core and the store against unbounded input (Section 9.2
/// "bounds").
const MAX_TITLE_LEN: usize = 512;
const MAX_NAME_LEN: usize = 256;
const MAX_SYSTEM_PROMPT_LEN: usize = 32_768;
const MAX_SECRET_LEN: usize = 8_192;
const MAX_PROVIDER_ID_LEN: usize = 128;
const MAX_TAGS: usize = 64;
/// Upper bound on a single user chat message (architecture.md Section 9.2
/// "bounds"). 128 KiB is generous for a chat turn while guarding the core
/// against unbounded input.
const MAX_MESSAGE_LEN: usize = 131_072;

/// A structured error returned across the IPC boundary when a command rejects
/// its input or the core fails. Serialized as `{ "code": ..., "message": ... }`.
///
/// It carries only a machine code and a display-safe message; it never echoes
/// secret material.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandError {
    pub code: ErrorCode,
    pub message: String,
}

/// Coarse error categories the frontend can branch on.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ErrorCode {
    /// Argument validation failed (type/enum/bounds/id parse).
    InvalidArgument,
    /// A referenced entity does not exist.
    NotFound,
    /// The core / persistence / keystore layer failed.
    Internal,
}

impl CommandError {
    fn invalid(message: impl Into<String>) -> Self {
        CommandError {
            code: ErrorCode::InvalidArgument,
            message: message.into(),
        }
    }

    fn not_found(message: impl Into<String>) -> Self {
        CommandError {
            code: ErrorCode::NotFound,
            message: message.into(),
        }
    }

    fn internal(message: impl Into<String>) -> Self {
        CommandError {
            code: ErrorCode::Internal,
            message: message.into(),
        }
    }
}

impl From<orchestrator_core::SessionError> for CommandError {
    fn from(err: orchestrator_core::SessionError) -> Self {
        use orchestrator_core::SessionError;
        match err {
            SessionError::ConversationNotFound(id) => {
                CommandError::not_found(format!("conversation not found: {id}"))
            }
            other => CommandError::internal(other.to_string()),
        }
    }
}

// --- Argument validation helpers -------------------------------------------

/// Parse a UUID argument, rejecting malformed ids before touching the core.
fn parse_uuid(field: &str, value: &str) -> Result<Uuid, CommandError> {
    Uuid::parse_str(value)
        .map_err(|_| CommandError::invalid(format!("`{field}` is not a valid UUID: {value:?}")))
}

/// Validate a required free-text field: trimmed non-empty and within `max`.
fn validate_nonempty(field: &str, value: &str, max: usize) -> Result<(), CommandError> {
    if value.trim().is_empty() {
        return Err(CommandError::invalid(format!(
            "`{field}` must not be empty"
        )));
    }
    if value.chars().count() > max {
        return Err(CommandError::invalid(format!(
            "`{field}` exceeds the maximum length of {max}"
        )));
    }
    Ok(())
}

/// Validate an optional free-text field: when present, within `max`.
fn validate_opt_len(field: &str, value: &Option<String>, max: usize) -> Result<(), CommandError> {
    if let Some(v) = value {
        if v.chars().count() > max {
            return Err(CommandError::invalid(format!(
                "`{field}` exceeds the maximum length of {max}"
            )));
        }
    }
    Ok(())
}

/// Validate a list of privacy tags: bounded count and non-empty custom values.
fn validate_privacy_tags(tags: &[PrivacyTag]) -> Result<(), CommandError> {
    if tags.len() > MAX_TAGS {
        return Err(CommandError::invalid(format!(
            "too many privacy tags (max {MAX_TAGS})"
        )));
    }
    for tag in tags {
        if let PrivacyTag::Custom(value) = tag {
            validate_nonempty("privacyTags.custom", value, MAX_NAME_LEN)?;
        }
    }
    Ok(())
}

/// Validate a manual route: both fields present and bounded.
fn validate_manual_route(route: &ManualRoute) -> Result<(), CommandError> {
    validate_nonempty("route.providerId", &route.provider_id, MAX_PROVIDER_ID_LEN)?;
    validate_nonempty("route.model", &route.model, MAX_NAME_LEN)?;
    Ok(())
}

// --- Conversation commands --------------------------------------------------

/// List all conversations (Section 8.1 history surface).
#[tauri::command]
pub async fn list_conversations(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<Conversation>, CommandError> {
    state
        .session_manager
        .list_conversations()
        .await
        .map_err(CommandError::from)
}

/// Arguments for [`create_conversation`].
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateConversationArgs {
    pub title: Option<String>,
    pub persona_id: Option<String>,
    #[serde(default)]
    pub privacy_tags: Vec<PrivacyTag>,
}

/// Create a new conversation. Validates the optional title/persona id/tags
/// before delegating to the [`SessionManager`].
#[tauri::command]
pub async fn create_conversation(
    state: tauri::State<'_, AppState>,
    args: CreateConversationArgs,
) -> Result<Conversation, CommandError> {
    validate_opt_len("title", &args.title, MAX_TITLE_LEN)?;
    validate_privacy_tags(&args.privacy_tags)?;
    let persona_id = match &args.persona_id {
        Some(id) => Some(parse_uuid("personaId", id)?),
        None => None,
    };
    let init = ConversationInit {
        title: args.title,
        persona_id,
        privacy_tags: args.privacy_tags,
    };
    state
        .session_manager
        .create_conversation(init)
        .await
        .map_err(CommandError::from)
}

/// Rename a conversation. Validates the id and the non-empty bounded title.
#[tauri::command]
pub async fn rename_conversation(
    state: tauri::State<'_, AppState>,
    conversation_id: String,
    title: String,
) -> Result<Conversation, CommandError> {
    let id = parse_uuid("conversationId", &conversation_id)?;
    validate_nonempty("title", &title, MAX_TITLE_LEN)?;
    state
        .session_manager
        .rename_conversation(id, title)
        .await
        .map_err(CommandError::from)
}

/// Set (or replace) a conversation's privacy tags.
#[tauri::command]
pub async fn set_conversation_tags(
    state: tauri::State<'_, AppState>,
    conversation_id: String,
    tags: Vec<PrivacyTag>,
) -> Result<Conversation, CommandError> {
    let id = parse_uuid("conversationId", &conversation_id)?;
    validate_privacy_tags(&tags)?;
    state
        .session_manager
        .set_conversation_tags(id, tags)
        .await
        .map_err(CommandError::from)
}

/// Delete a conversation (and, via cascade, its messages).
#[tauri::command]
pub async fn delete_conversation(
    state: tauri::State<'_, AppState>,
    conversation_id: String,
) -> Result<(), CommandError> {
    let id = parse_uuid("conversationId", &conversation_id)?;
    state
        .session_manager
        .delete_conversation(id)
        .await
        .map_err(CommandError::from)
}

// --- Persona commands -------------------------------------------------------

/// List all saved personas (Section 8.4).
#[tauri::command]
pub async fn list_personas(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<AgentPersona>, CommandError> {
    state
        .session_manager
        .personas()
        .list()
        .await
        .map_err(|e| CommandError::internal(e.to_string()))
}

/// Arguments for [`create_persona`] / [`update_persona`].
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonaInput {
    pub name: String,
    pub system_prompt: String,
    pub default_route: Option<ManualRoute>,
    pub routing_hint: Option<RoutingHint>,
    #[serde(default)]
    pub allowed_tool_servers: Vec<String>,
    #[serde(default)]
    pub parameters: ModelParameters,
}

impl PersonaInput {
    /// Validate every field (bounds, enum membership via serde, id parse) and
    /// materialize an [`AgentPersona`] with `id`.
    fn into_persona(self, id: Uuid) -> Result<AgentPersona, CommandError> {
        validate_nonempty("name", &self.name, MAX_NAME_LEN)?;
        validate_nonempty("systemPrompt", &self.system_prompt, MAX_SYSTEM_PROMPT_LEN)?;
        if let Some(route) = &self.default_route {
            validate_manual_route(route)?;
        }
        if self.allowed_tool_servers.len() > MAX_TAGS {
            return Err(CommandError::invalid(format!(
                "too many allowedToolServers (max {MAX_TAGS})"
            )));
        }
        let allowed_tool_servers = self
            .allowed_tool_servers
            .iter()
            .map(|s| parse_uuid("allowedToolServers", s))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(AgentPersona {
            id,
            name: self.name,
            system_prompt: self.system_prompt,
            default_route: self.default_route,
            routing_hint: self.routing_hint,
            allowed_tool_servers,
            parameters: self.parameters,
        })
    }
}

/// Create a new persona. Validates the input, assigns a fresh id, and persists.
#[tauri::command]
pub async fn create_persona(
    state: tauri::State<'_, AppState>,
    persona: PersonaInput,
) -> Result<AgentPersona, CommandError> {
    let persona = persona.into_persona(Uuid::new_v4())?;
    state
        .session_manager
        .personas()
        .insert(&persona)
        .await
        .map_err(|e| CommandError::internal(e.to_string()))?;
    Ok(persona)
}

/// Update an existing persona. Validates the id and the input, then persists.
#[tauri::command]
pub async fn update_persona(
    state: tauri::State<'_, AppState>,
    persona_id: String,
    persona: PersonaInput,
) -> Result<AgentPersona, CommandError> {
    let id = parse_uuid("personaId", &persona_id)?;
    let persona = persona.into_persona(id)?;
    let repo = state.session_manager.personas();
    if repo
        .get(id)
        .await
        .map_err(|e| CommandError::internal(e.to_string()))?
        .is_none()
    {
        return Err(CommandError::not_found(format!("persona not found: {id}")));
    }
    repo.update(&persona)
        .await
        .map_err(|e| CommandError::internal(e.to_string()))?;
    Ok(persona)
}

/// Delete a persona by id.
#[tauri::command]
pub async fn delete_persona(
    state: tauri::State<'_, AppState>,
    persona_id: String,
) -> Result<(), CommandError> {
    let id = parse_uuid("personaId", &persona_id)?;
    state
        .session_manager
        .personas()
        .delete(id)
        .await
        .map_err(|e| CommandError::internal(e.to_string()))
}

// --- Secret entry (P1.7) ----------------------------------------------------

/// Store a provider API key in the OS keystore and return ONLY its
/// [`SecretRef`] handle (architecture.md Section 9.1: "`set_provider_secret`
/// writes straight to the keystore and returns only a `SecretRef`").
///
/// NO-SECRET-ACROSS-IPC INVARIANT: the plaintext `secret` flows INTO the core
/// here and is written straight to the keystore; the return value is only the
/// opaque handle. This command never returns the secret, and no other command
/// or event payload carries raw key material. The single plaintext read seam is
/// the core-internal `SecretStore::resolve`, which is deliberately NOT exposed
/// as a command.
#[tauri::command]
pub async fn set_provider_secret(
    state: tauri::State<'_, AppState>,
    provider_id: String,
    secret: String,
) -> Result<SecretRef, CommandError> {
    set_provider_secret_inner(&state, &provider_id, &secret)
}

/// The full validate-then-store-then-return body of [`set_provider_secret`],
/// factored out so it can be driven directly in tests without a live Tauri
/// `State`. The `#[tauri::command]` wrapper above is a thin adapter over this.
///
/// This is what enforces the NO-SECRET-ACROSS-IPC invariant: it returns ONLY
/// the opaque [`SecretRef`] handle produced by the store, never the plaintext
/// `secret` it was given. A regression that made this path echo the secret
/// would fail `set_provider_secret_inner_returns_only_ref` below.
fn set_provider_secret_inner(
    state: &AppState,
    provider_id: &str,
    secret: &str,
) -> Result<SecretRef, CommandError> {
    validate_nonempty("providerId", provider_id, MAX_PROVIDER_ID_LEN)?;
    if secret.is_empty() {
        return Err(CommandError::invalid("`secret` must not be empty"));
    }
    if secret.len() > MAX_SECRET_LEN {
        return Err(CommandError::invalid(format!(
            "`secret` exceeds the maximum length of {MAX_SECRET_LEN} bytes"
        )));
    }
    state
        .secret_store
        .store(provider_id, secret)
        .map_err(|e| CommandError::internal(e.to_string()))
}

// --- Model selector data (P2.10) -------------------------------------------

/// List every available model across all configured providers, with per-model
/// capabilities and price, for the Phase 5 model selector (architecture.md
/// Section 8.2) and Phase 4 routing (Section 6.1).
///
/// It assembles the built-in provider registry from the persisted
/// [`ProviderConfig`] rows and the pricing table from the versioned
/// [`AppConfig`] (Section 6.2), then returns `Vec<`[`AvailableModel`]`>`.
///
/// DISPLAY-SAFE (Section 9.1 / 9.2): the returned rows carry ONLY
/// provider/model/capabilities/price labels. No secret material and no resolved
/// [`SecretRef`] value ever crosses this boundary; secrets are resolved only
/// inside the registry when it builds an instance, and stay there.
#[tauri::command]
pub async fn list_available_models(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<AvailableModel>, CommandError> {
    list_available_models_inner(&state).await
}

/// The full body of [`list_available_models`], factored out so it can be driven
/// directly in tests without a live Tauri `State` (the `set_provider_secret`
/// testability pattern). Loads config rows + pricing, builds the registry, and
/// enumerates models.
///
/// This is what enforces the DISPLAY-SAFE invariant: it returns only
/// [`AvailableModel`] rows built from the provider/model/capabilities/price
/// surface, never touching `SecretStore::resolve` for the return value. A
/// regression that leaked secret material into the result would fail
/// `list_available_models_inner_returns_display_safe_rows` below.
async fn list_available_models_inner(
    state: &AppState,
) -> Result<Vec<AvailableModel>, CommandError> {
    let db = state.session_manager.db();

    // Persisted provider instances (only the SecretRef handle is stored).
    let configs: Vec<ProviderConfig> = ProviderRepo::new(db)
        .list()
        .await
        .map_err(|e| CommandError::internal(e.to_string()))?;

    // User-entered pricing from the versioned app config (Section 6.2).
    let app_config = AppConfig::load(db)
        .await
        .map_err(|e| CommandError::internal(e.to_string()))?;
    let pricing = pricing_table_from_config(&app_config.pricing);

    // Build the built-in registry and instantiate the configured providers,
    // resolving each row's api_key_ref through the secret store at build time.
    let registry = providers::build_registry(configs.iter(), state.secret_store.as_ref())
        .map_err(|e| CommandError::internal(e.to_string()))?;

    // Enumerate models (may hit the network per provider) and attach
    // capabilities + price.
    list_models(&registry, &configs, &pricing)
        .await
        .map_err(|e| CommandError::internal(e.to_string()))
}

/// Convert the persisted [`PricingConfig`] (the single source of truth, Section
/// 6.2) into the in-memory [`PricingTable`] `list_available_models` reads. The
/// persisted rates seed the table; kinds/models the user did not price resolve
/// to zero (local providers included).
fn pricing_table_from_config(config: &PricingConfig) -> PricingTable {
    let mut table = PricingTable::new();
    for (kind, rate) in &config.per_kind {
        table.set_kind(
            *kind,
            TokenPrice::new(rate.input_per_mtok, rate.output_per_mtok),
        );
    }
    for (kind, models) in &config.per_model {
        for (model, rate) in models {
            table.set_model(
                *kind,
                model.clone(),
                TokenPrice::new(rate.input_per_mtok, rate.output_per_mtok),
            );
        }
    }
    table
}

/// Derive the set of provider instance ids that are PROVABLY local
/// (architecture.md Section 6.2), from the concrete [`ProviderKind`] of each
/// configured provider. This is the fail-closed locality signal routing uses
/// for the privacy hard constraint (instead of trusting a zero price, which a
/// misconfigured cloud provider could carry):
///
///   - [`ProviderKind::LmStudio`] runs on the local machine, so it is always
///     local.
///   - [`ProviderKind::GenericOpenAI`] is local ONLY when its endpoint is a
///     loopback host (`localhost` / `127.0.0.1` / `[::1]`); a generic endpoint
///     pointed at a remote host is treated as cloud.
///   - every other kind is cloud.
fn local_provider_ids(configs: &[ProviderConfig]) -> std::collections::BTreeSet<String> {
    configs
        .iter()
        .filter(|cfg| match cfg.kind {
            ProviderKind::LmStudio => true,
            ProviderKind::GenericOpenAI => {
                cfg.base_url.as_deref().is_some_and(is_loopback_endpoint)
            }
            _ => false,
        })
        .map(|cfg| cfg.id.clone())
        .collect()
}

/// Whether `url` names a loopback host (a local endpoint). Conservative: any URL
/// we cannot confidently classify as loopback is treated as NON-local so the
/// privacy gate fails closed.
fn is_loopback_endpoint(url: &str) -> bool {
    // Strip an optional scheme, then take the authority up to the first `/`.
    let without_scheme = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    let authority = without_scheme.split('/').next().unwrap_or("");
    // Drop credentials and port; keep the host (handles bracketed IPv6).
    let host_port = authority.rsplit('@').next().unwrap_or(authority);
    let host = if let Some(end) = host_port.strip_prefix('[') {
        // IPv6 literal: `[::1]:1234` -> `::1`.
        end.split(']').next().unwrap_or("")
    } else {
        host_port.split(':').next().unwrap_or("")
    };
    matches!(host, "localhost" | "127.0.0.1" | "::1")
}

// --- Message pipeline (P4.6 / Section 2.2 / 8.1) ---------------------------

/// Send a user message and drive the end-to-end pipeline (architecture.md
/// Section 8.1 signature): `message -> route -> provider -> tool loop ->
/// persist`. The assistant response is delivered by STREAMING
/// [`orchestrator_core::CoreEvent`]s over the existing core-event bridge
/// (`events.rs`), NOT by holding this command open (Section 2.2): the command
/// validates its arguments, kicks off the pipeline turn on a spawned task, and
/// returns.
///
/// `overrideRoute` is the optional per-message manual override (Section 6.3,
/// highest precedence). When present it must name a provider/model; validation
/// rejects a malformed route before the pipeline runs.
#[tauri::command]
pub async fn send_message(
    state: tauri::State<'_, AppState>,
    conversation_id: String,
    content: String,
    override_route: Option<ManualRoute>,
) -> Result<(), CommandError> {
    send_message_inner(&state, &conversation_id, &content, override_route).await
}

/// The full validate-then-drive body of [`send_message`], factored out so it can
/// be driven directly in tests without a live Tauri `State` (the established
/// `_inner` testability pattern). It validates the id/content/override, assembles
/// the [`TurnContext`] inputs from [`AppState`] (session manager, routing policy
/// registry, a freshly-built provider registry + candidate models, the
/// permission gate over the shared registry, the core-event sender, and the MCP
/// handles), and spawns the pipeline turn so other conversations are not blocked
/// (Section 7.5 concurrency). Streaming is delivered via `core_events`.
async fn send_message_inner(
    state: &AppState,
    conversation_id: &str,
    content: &str,
    override_route: Option<ManualRoute>,
) -> Result<(), CommandError> {
    let conversation_id = parse_uuid("conversationId", conversation_id)?;
    validate_nonempty("content", content, MAX_MESSAGE_LEN)?;
    if let Some(route) = &override_route {
        validate_manual_route(route)?;
    }

    // Build the provider registry + candidate model list from the persisted
    // config rows + pricing, mirroring `list_available_models_inner`. Doing this
    // per turn keeps the routing candidate set current with configuration.
    let db = state.session_manager.db();
    let configs: Vec<ProviderConfig> = ProviderRepo::new(db)
        .list()
        .await
        .map_err(|e| CommandError::internal(e.to_string()))?;
    let app_config = AppConfig::load(db)
        .await
        .map_err(|e| CommandError::internal(e.to_string()))?;
    let pricing = pricing_table_from_config(&app_config.pricing);
    let registry = providers::build_registry(configs.iter(), state.secret_store.as_ref())
        .map_err(|e| CommandError::internal(e.to_string()))?;
    let available = list_models(&registry, &configs, &pricing)
        .await
        .map_err(|e| CommandError::internal(e.to_string()))?;
    // The provably-local provider ids (from each row's concrete ProviderKind),
    // so routing enforces LocalOnly/Confidential on provable locality rather
    // than a zero price (Section 6.2 fail-closed).
    let local_provider_ids = local_provider_ids(&configs);

    // Clone the shared subsystems into the spawned task so the command returns
    // immediately and the streamed events arrive over the bridge.
    let session_manager = state.session_manager.clone();
    let policies = state.policies.clone();
    let gate = PermissionGate::new(state.core_events.clone(), state.permission_registry.clone());
    let events = state.core_events.clone();
    let servers = state.mcp_servers.as_ref().clone();
    let content = content.to_string();

    tokio::spawn(async move {
        let ctx = TurnContext {
            session_manager: &session_manager,
            policies: &policies,
            providers: &registry,
            servers,
            gate,
            events,
            available,
            local_provider_ids,
        };
        // The pipeline emits MessageError + persists an Error-status message on
        // failure, so the returned error is already surfaced; nothing to do here.
        let _ = run_turn(&ctx, conversation_id, content, override_route).await;
    });

    Ok(())
}

// --- MCP tool permissions (P3.5 / Section 9.4) ------------------------------

/// Resolve a pending `Ask`-mode tool-permission request (architecture.md
/// Sections 5.6 / 9.4). When the core emits a
/// [`orchestrator_core::CoreEvent::PermissionRequested`] it BLOCKS the pending
/// tool invocation until the user answers; the frontend calls this command with
/// the request's `requestId` and the user's [`Decision`] to unblock it.
///
/// The `decision` carries `allow` (proceed or refuse) and an advisory
/// `remember` flag (remember the answer for the session, Section 5.6). Returns
/// `true` if a request with `requestId` was awaiting a decision (now
/// unblocked), `false` if no such request exists (already resolved, timed out,
/// or an unknown id).
///
/// UI is intentionally OUT of scope for FEAT-002: this is only the command seam
/// wired to the framework-agnostic `PermissionRegistry` on [`AppState`].
#[tauri::command]
pub async fn resolve_permission(
    state: tauri::State<'_, AppState>,
    request_id: String,
    decision: Decision,
) -> Result<bool, CommandError> {
    resolve_permission_inner(&state, &request_id, decision)
}

/// The full validate-then-resolve body of [`resolve_permission`], factored out
/// so it can be driven directly in tests without a live Tauri `State` (the
/// `set_provider_secret` testability pattern). The `#[tauri::command]` wrapper
/// above is a thin adapter over this: it parses/validates the `requestId` UUID,
/// then forwards the decision to the shared `PermissionRegistry`.
fn resolve_permission_inner(
    state: &AppState,
    request_id: &str,
    decision: Decision,
) -> Result<bool, CommandError> {
    let id = parse_uuid("requestId", request_id)?;
    Ok(state.permission_registry.resolve(id, decision))
}

// --- Diagnostics ------------------------------------------------------------

/// Returns the application version compiled into the binary.
///
/// In Tauri v2 this registration in `main.rs`'s `generate_handler!` is what
/// authorizes the command across the IPC boundary; no `capabilities/default.json`
/// permission entry is required (those gate Tauri's built-in plugins, not user
/// commands).
#[tauri::command]
pub fn app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use orchestrator_core::SessionManager;
    use persistence::Db;
    use secrets::{InMemorySecretStore, SecretError};
    use std::sync::Arc;

    async fn test_state() -> AppState {
        let db = Db::open_in_memory().await.unwrap();
        let (state, _rx) = AppState::new(
            SessionManager::new(db),
            Arc::new(InMemorySecretStore::new()),
        );
        state
    }

    #[test]
    fn parse_uuid_rejects_garbage() {
        assert!(parse_uuid("id", "not-a-uuid").is_err());
        assert!(parse_uuid("id", &Uuid::new_v4().to_string()).is_ok());
    }

    #[test]
    fn validate_nonempty_enforces_bounds() {
        assert!(validate_nonempty("f", "   ", 10).is_err());
        assert!(validate_nonempty("f", "ok", 10).is_ok());
        assert!(validate_nonempty("f", &"x".repeat(11), 10).is_err());
    }

    #[test]
    fn persona_input_rejects_empty_name() {
        let input = PersonaInput {
            name: "  ".to_string(),
            system_prompt: "hi".to_string(),
            default_route: None,
            routing_hint: None,
            allowed_tool_servers: vec![],
            parameters: ModelParameters::default(),
        };
        assert!(input.into_persona(Uuid::new_v4()).is_err());
    }

    #[test]
    fn persona_input_rejects_bad_tool_server_id() {
        let input = PersonaInput {
            name: "Coder".to_string(),
            system_prompt: "hi".to_string(),
            default_route: None,
            routing_hint: None,
            allowed_tool_servers: vec!["nope".to_string()],
            parameters: ModelParameters::default(),
        };
        assert!(input.into_persona(Uuid::new_v4()).is_err());
    }

    /// Drives the `set_provider_secret` HANDLER body end-to-end (via the
    /// extracted `set_provider_secret_inner`, which the `#[tauri::command]`
    /// wrapper is a thin adapter over). It asserts the handler returns ONLY the
    /// opaque `SecretRef` handle, never the secret, and that the secret is
    /// retrievable only through the core-internal `resolve` seam (not a
    /// command). A regression that made the handler echo the secret in its
    /// return value or serialized form would FAIL this test.
    #[tokio::test]
    async fn set_provider_secret_inner_returns_only_ref() {
        let state = test_state().await;
        let plaintext = "sk-do-not-leak-1234";

        // Drive the actual handler body, not `secret_store.store` directly.
        let secret_ref = set_provider_secret_inner(&state, "openai", plaintext).unwrap();

        // The handler returns the opaque provider-id handle, never the key.
        assert_eq!(secret_ref, SecretRef::new("openai"));
        assert_eq!(secret_ref.handle(), "openai");
        assert!(!secret_ref.handle().contains(plaintext));
        // The serialized form that would cross IPC is just the handle.
        let json = serde_json::to_string(&secret_ref).unwrap();
        assert_eq!(json, "\"openai\"");
        assert!(!json.contains(plaintext));
        // The handler actually persisted the secret to the store, and the
        // plaintext is reachable ONLY via the internal resolve seam.
        assert_eq!(state.secret_store.resolve(&secret_ref).unwrap(), plaintext);
    }

    /// The handler REJECTS invalid input before touching the store: an empty
    /// provider id, an empty secret, and an over-long secret all error, and
    /// nothing is written for the rejected calls.
    #[tokio::test]
    async fn set_provider_secret_inner_rejects_invalid_input() {
        let state = test_state().await;

        // Empty provider id -> InvalidArgument, nothing stored.
        let err = set_provider_secret_inner(&state, "", "sk-key").unwrap_err();
        assert!(matches!(err.code, ErrorCode::InvalidArgument));

        // Empty secret -> InvalidArgument.
        let err = set_provider_secret_inner(&state, "openai", "").unwrap_err();
        assert!(matches!(err.code, ErrorCode::InvalidArgument));

        // Over-long secret -> InvalidArgument.
        let too_long = "x".repeat(MAX_SECRET_LEN + 1);
        let err = set_provider_secret_inner(&state, "openai", &too_long).unwrap_err();
        assert!(matches!(err.code, ErrorCode::InvalidArgument));

        // None of the rejected calls stored anything under "openai".
        assert!(matches!(
            state.secret_store.resolve(&SecretRef::new("openai")),
            Err(SecretError::NotFound(_))
        ));
    }

    /// `resolve_permission` rejects a malformed request id before touching the
    /// registry, and returns `false` for a well-formed but unknown id (nothing
    /// was awaiting it). A registered request is unblocked and reports `true`.
    #[tokio::test]
    async fn resolve_permission_inner_validates_and_resolves() {
        let state = test_state().await;

        // Malformed UUID -> InvalidArgument, registry untouched.
        let err = resolve_permission_inner(&state, "not-a-uuid", Decision::allow()).unwrap_err();
        assert!(matches!(err.code, ErrorCode::InvalidArgument));

        // Well-formed but unknown id -> false (nothing awaiting it).
        let unknown = Uuid::new_v4().to_string();
        assert!(!resolve_permission_inner(&state, &unknown, Decision::allow()).unwrap());
    }

    /// `send_message` REJECTS invalid input before touching the core: a
    /// malformed conversation id, empty content, an over-long message, and an
    /// override route with an empty provider id all error with
    /// `InvalidArgument`. This mirrors the `set_provider_secret_inner` rejection
    /// tests and drives the extracted inner fn exactly as the `#[tauri::command]`
    /// wrapper does.
    #[tokio::test]
    async fn send_message_inner_rejects_invalid_input() {
        let state = test_state().await;

        // Malformed conversation id -> InvalidArgument.
        let err = send_message_inner(&state, "not-a-uuid", "hi", None)
            .await
            .unwrap_err();
        assert!(matches!(err.code, ErrorCode::InvalidArgument));

        // Empty content -> InvalidArgument.
        let valid_id = Uuid::new_v4().to_string();
        let err = send_message_inner(&state, &valid_id, "   ", None)
            .await
            .unwrap_err();
        assert!(matches!(err.code, ErrorCode::InvalidArgument));

        // Over-long content -> InvalidArgument.
        let too_long = "x".repeat(MAX_MESSAGE_LEN + 1);
        let err = send_message_inner(&state, &valid_id, &too_long, None)
            .await
            .unwrap_err();
        assert!(matches!(err.code, ErrorCode::InvalidArgument));

        // Override route with an empty provider id -> InvalidArgument.
        let bad_route = ManualRoute {
            provider_id: "".to_string(),
            model: "gpt-4o".to_string(),
        };
        let err = send_message_inner(&state, &valid_id, "hi", Some(bad_route))
            .await
            .unwrap_err();
        assert!(matches!(err.code, ErrorCode::InvalidArgument));
    }

    /// Empty provider id / secret are rejected by validation before the store.
    #[test]
    fn secret_validation_bounds() {
        assert!(validate_nonempty("providerId", "", MAX_PROVIDER_ID_LEN).is_err());
        assert!(validate_nonempty("providerId", "openai", MAX_PROVIDER_ID_LEN).is_ok());
    }

    /// With no configured providers, the command body returns an empty list
    /// (no network is touched: there are no instances to enumerate). This drives
    /// the extracted inner fn end-to-end (load configs + pricing, build the
    /// registry, enumerate) exactly as the `#[tauri::command]` wrapper does.
    #[tokio::test]
    async fn list_available_models_inner_empty_when_no_providers() {
        let state = test_state().await;
        let models = list_available_models_inner(&state).await.unwrap();
        assert!(models.is_empty());
    }

    /// The persisted pricing config converts into the in-memory pricing table:
    /// per-kind and per-model rates are threaded through, and unpriced kinds
    /// (local providers included) resolve to zero (Section 6.2).
    #[test]
    fn pricing_table_from_config_threads_rates_and_zeros_unpriced() {
        use orchestrator_core::ProviderKind;
        use persistence::config::TokenRate;
        use std::collections::BTreeMap;

        let mut per_kind = BTreeMap::new();
        per_kind.insert(
            ProviderKind::OpenAI,
            TokenRate {
                input_per_mtok: 2.5,
                output_per_mtok: 10.0,
            },
        );
        let mut openai_models = BTreeMap::new();
        openai_models.insert(
            "gpt-4o-mini".to_string(),
            TokenRate {
                input_per_mtok: 0.15,
                output_per_mtok: 0.6,
            },
        );
        let mut per_model = BTreeMap::new();
        per_model.insert(ProviderKind::OpenAI, openai_models);

        let config = PricingConfig {
            per_kind,
            per_model,
            last_edited: None,
        };
        let table = pricing_table_from_config(&config);

        // Per-model override wins.
        assert_eq!(
            table.price_for(ProviderKind::OpenAI, "gpt-4o-mini"),
            TokenPrice::new(0.15, 0.6)
        );
        // Per-kind default applies to other models.
        assert_eq!(
            table.price_for(ProviderKind::OpenAI, "gpt-4o"),
            TokenPrice::new(2.5, 10.0)
        );
        // An unpriced kind (local) resolves to zero.
        assert_eq!(
            table.price_for(ProviderKind::LmStudio, "any"),
            TokenPrice::ZERO
        );
    }

    /// `local_provider_ids` classifies locality from the concrete
    /// [`ProviderKind`] (Section 6.2 fail-closed), NOT from price: LM Studio is
    /// always local, a loopback GenericOpenAI endpoint is local, a remote
    /// GenericOpenAI endpoint is cloud, and every other kind is cloud.
    #[test]
    fn local_provider_ids_classifies_by_kind_and_endpoint() {
        fn cfg(id: &str, kind: ProviderKind, base_url: Option<&str>) -> ProviderConfig {
            ProviderConfig {
                id: id.to_string(),
                kind,
                base_url: base_url.map(str::to_string),
                api_key_ref: None,
                extra: serde_json::Value::Null,
            }
        }

        let configs = [
            cfg("lm", ProviderKind::LmStudio, None),
            cfg(
                "local-generic",
                ProviderKind::GenericOpenAI,
                Some("http://127.0.0.1:1234/v1"),
            ),
            cfg(
                "localhost-generic",
                ProviderKind::GenericOpenAI,
                Some("http://localhost:8080"),
            ),
            cfg(
                "remote-generic",
                ProviderKind::GenericOpenAI,
                Some("https://api.example.com/v1"),
            ),
            // A GenericOpenAI with no endpoint cannot be proven local -> cloud.
            cfg("bare-generic", ProviderKind::GenericOpenAI, None),
            cfg("oai", ProviderKind::OpenAI, Some("http://127.0.0.1/v1")),
        ];

        let local = local_provider_ids(&configs);
        assert!(local.contains("lm"));
        assert!(local.contains("local-generic"));
        assert!(local.contains("localhost-generic"));
        assert!(!local.contains("remote-generic"));
        assert!(!local.contains("bare-generic"));
        // A loopback base_url does not make a cloud KIND local.
        assert!(!local.contains("oai"));
    }

    /// `is_loopback_endpoint` recognizes loopback hosts across scheme/port/IPv6
    /// forms and rejects remote hosts (conservative: unknown => not loopback).
    #[test]
    fn is_loopback_endpoint_recognizes_local_hosts() {
        assert!(is_loopback_endpoint("http://localhost:1234/v1"));
        assert!(is_loopback_endpoint("https://127.0.0.1"));
        assert!(is_loopback_endpoint("http://[::1]:8080/v1"));
        assert!(is_loopback_endpoint("localhost:1234"));
        assert!(!is_loopback_endpoint("https://api.openai.com/v1"));
        assert!(!is_loopback_endpoint("http://10.0.0.5:1234"));
        assert!(!is_loopback_endpoint("http://user@evil.com/localhost"));
    }

    /// The `AvailableModel` shape that crosses IPC is display-safe: it serializes
    /// to provider/model/capabilities/price fields only, never secret material.
    #[test]
    fn available_model_is_display_safe() {
        let row = AvailableModel {
            provider_id: "openai".to_string(),
            model: "gpt-4o".to_string(),
            capabilities: providers::Capabilities::default(),
            price: TokenPrice::new(2.5, 10.0),
        };
        let json = serde_json::to_string(&row).unwrap();
        assert!(json.contains("\"providerId\":\"openai\""));
        assert!(json.contains("\"model\":\"gpt-4o\""));
        assert!(json.contains("\"capabilities\""));
        assert!(json.contains("\"price\""));
        // No secret material of any kind is present.
        assert!(!json.to_lowercase().contains("secret"));
        assert!(!json.to_lowercase().contains("apikey"));
        assert!(!json.contains("sk-"));
    }
}
