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

use std::sync::Arc;

use mcp_client::McpServerHandle;
use orchestrator_core::{
    run_turn, AgentPersona, Conversation, ConversationInit, Decision, ManualRoute, McpServerConfig,
    McpTransport, Message, ModelParameters, PermissionGate, PermissionMode, PrivacyTag,
    ProviderConfig, ProviderKind, RouteSource, RoutingHint, RoutingMode, SecretRef, TurnContext,
};
use persistence::config::{AppConfig, PricingConfig};
use persistence::{McpServerRepo, ProviderRepo};
use providers::{
    list_available_models as list_models,
    list_available_models_with_build_errors as list_models_with_build_errors,
    AvailableModelsResult, PricingTable, TokenPrice,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::state::{spawn_connect, AppState};

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
/// Bounds for MCP server configuration fields (Section 9.2 "bounds").
const MAX_URL_LEN: usize = 2_048;
const MAX_COMMAND_LEN: usize = 4_096;
/// Upper bound on the count of stdio args / env / http headers on one server.
const MAX_MCP_LIST_ITEMS: usize = 256;
const MAX_ENV_KEY_LEN: usize = 256;
const MAX_ENV_VALUE_LEN: usize = 8_192;

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
/// [`AppConfig`] (Section 6.2), then returns an [`AvailableModelsResult`]
/// carrying both the successful [`AvailableModel`] rows and a display-safe list
/// of per-provider enumeration errors so the UI can show why a misconfigured or
/// unreachable provider contributed nothing (Section 8.2). A single failing
/// provider never blanks the successful results.
///
/// DISPLAY-SAFE (Section 9.1 / 9.2): both the model rows and the enumeration
/// error messages carry ONLY provider/model/capabilities/price/error labels. No
/// secret material and no resolved [`SecretRef`] value ever crosses this
/// boundary; secrets are resolved only inside the registry when it builds an
/// instance, and stay there.
#[tauri::command]
pub async fn list_available_models(
    state: tauri::State<'_, AppState>,
) -> Result<AvailableModelsResult, CommandError> {
    list_available_models_inner(&state).await
}

/// The full body of [`list_available_models`], factored out so it can be driven
/// directly in tests without a live Tauri `State` (the `set_provider_secret`
/// testability pattern). Loads config rows + pricing, builds the registry, and
/// enumerates models.
///
/// This is what enforces the DISPLAY-SAFE invariant: it returns only
/// [`AvailableModel`] rows and display-safe enumeration error messages built
/// from the provider/model/capabilities/price surface, never touching
/// `SecretStore::resolve` for the return value. A regression that leaked secret
/// material into the result would fail
/// `list_available_models_inner_returns_display_safe_rows` below.
async fn list_available_models_inner(
    state: &AppState,
) -> Result<AvailableModelsResult, CommandError> {
    let db = state.session_manager.db();

    // Auto-seed the Ollama provider-config row (idempotent) BEFORE reading the
    // configs, so a locally-running Ollama's installed models are discovered via
    // the adapter's native `GET /api/tags` and surface under Local WITHOUT any
    // user configuration. Safe when Ollama is absent/offline: the enumeration
    // loop skips a provider whose `list_models` errors, so the seeded row simply
    // contributes no models instead of breaking the picker.
    ensure_ollama_provider_config(state).await?;

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
    //
    // This deliberately does NOT use `providers::build_registry`, which delegates
    // to `ProviderRegistry::build_all` and short-circuits on the FIRST build
    // failure via `?`. That fail-fast behavior turned a single un-buildable
    // config row into a thrown CommandError that aborted the WHOLE enumeration,
    // so the frontend saw a rejected promise (a clean-empty cause with no visible
    // error). Instead we build per-row and, on a per-row build error, skip that
    // row (do NOT insert an instance) rather than aborting. Because
    // `providers::list_available_models` now records any configured row lacking a
    // built instance as a display-safe enumeration error, skipping the failed
    // build surfaces it as a per-provider error instead of a whole-list abort,
    // preserving per-provider isolation.
    let mut registry = providers::builtin_registry();
    // Capture each per-row build failure (keyed by cfg.id) instead of discarding
    // it. A per-row build failure (missing factory, secret resolution error, bad
    // config) is STILL isolated: the row is simply left un-built so the other
    // rows still build (preserving the v0.7.3 non-fatal behavior), but the real
    // ProviderError Display is threaded into the shared enumeration below so the
    // picker's enumeration error carries the REAL cause instead of the generic
    // "no instance was built" message. DISPLAY-SAFE: err.to_string() is a
    // ProviderError Display, which carries only a reason (never key material).
    let mut build_errors: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();
    for cfg in &configs {
        if let Err(err) = registry.build_from_config(cfg, state.secret_store.as_ref()) {
            build_errors.insert(cfg.id.clone(), err.to_string());
        }
    }

    // Bridge the embedded engine seam (FEAT-002): the lifecycle commands import
    // `.gguf` models onto the SHARED `AppState.embedded_engine`, but
    // `build_registry` builds a FRESH, empty `EmbeddedEngine` for the persisted
    // `ProviderKind::Embedded` row. Replace that throwaway instance with an
    // `EmbeddedProvider` wrapping the SHARED engine (via `from_shared`, so the
    // provider and the lifecycle commands observe the SAME `Arc<EmbeddedEngine>`)
    // so the models the user imported are the ones enumerated here (and
    // therefore surface under Local in the picker across routing modes). Keyed
    // by `ChatProvider::id()` == `EMBEDDED_ENGINE_ID`, which is exactly the id
    // the seeded embedded config row carries, so it matches `list_models`'s
    // per-row `registry.get(&cfg.id)` lookup. No-op when no embedded row is
    // persisted (nothing imported yet): the shared engine is registered but has
    // no config row, so `list_models` skips it.
    registry.insert_instance(std::sync::Arc::new(
        providers::EmbeddedProvider::from_shared(state.embedded_engine.clone()),
    ));

    // Enumerate models (may hit the network per provider) and attach
    // capabilities + price. Feed the captured per-row build errors so an unbuilt
    // row surfaces its REAL build-failure cause instead of the generic message.
    list_models_with_build_errors(&registry, &configs, &pricing, &build_errors)
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

// --- Provider diagnostics (model-picker self-report) -----------------------

/// A display-safe, per-provider diagnostic row for the model-picker diagnostics
/// panel (architecture.md Section 8.2). It reports exactly what happened to one
/// configured provider row during the same enumeration the picker runs: its
/// kind, its display-safe base_url (or None), whether an instance was built,
/// how many models it enumerated, and the display-safe error (if any) explaining
/// why it contributed nothing.
///
/// DISPLAY-SAFE (Section 9.1 / 9.2): this crosses the Tauri IPC boundary and
/// carries ONLY id/kind/base_url/counts and a [`providers::ProviderError`]
/// Display string. It NEVER carries `api_key_ref`, a resolved [`SecretRef`], or
/// any key material. `base_url` is the display-safe persisted string; keys are
/// not.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderDiagnostic {
    /// The configured provider instance id (`ProviderConfig::id`).
    pub id: String,
    /// The provider kind of this configured row.
    pub kind: ProviderKind,
    /// The display-safe persisted base URL, or None when unset.
    pub base_url: Option<String>,
    /// Whether a live instance was built for this row (its factory build
    /// succeeded). False means the build failed or was skipped.
    pub instance_built: bool,
    /// How many models this provider enumerated (0 when it errored or is
    /// unbuilt).
    pub model_count: usize,
    /// The display-safe reason this provider contributed nothing, or None when
    /// it enumerated successfully. Never carries secret material.
    pub error: Option<String>,
}

/// The full display-safe report returned by [`provider_diagnostics`]: a summary
/// (how many rows are configured, the total model count across all providers,
/// and how many providers contributed at least one model) plus the per-provider
/// [`ProviderDiagnostic`] rows (architecture.md Section 8.2).
///
/// DISPLAY-SAFE (Section 9.1 / 9.2): every field is a count or a display-safe
/// per-provider row; no secret material crosses this boundary.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderDiagnosticsReport {
    /// Number of configured provider rows (including auto-seeded rows).
    pub configured_count: usize,
    /// Total number of models enumerated across all providers.
    pub total_model_count: usize,
    /// Number of providers that contributed at least one model.
    pub provider_count_with_models: usize,
    /// The per-provider diagnostic rows, one per configured row.
    pub providers: Vec<ProviderDiagnostic>,
}

/// Report, per configured provider row, exactly what happened during model
/// enumeration: the backend data source for the visible model-picker diagnostics
/// panel (architecture.md Section 8.2). This runs the SAME seeding + registry
/// build + enumeration the picker ([`list_available_models`]) runs, so it reports
/// the same reality the user sees, but with a richer per-provider breakdown that
/// makes the two invisible clean-empty causes (a configured row with no built
/// instance, or a provider whose enumeration errors) diagnosable.
///
/// DISPLAY-SAFE (Section 9.1 / 9.2): the report carries only id/kind/base_url/
/// counts and display-safe [`providers::ProviderError`] messages. It never
/// touches `SecretStore::resolve` for the return value and never surfaces
/// `api_key_ref` or key material.
#[tauri::command]
pub async fn provider_diagnostics(
    state: tauri::State<'_, AppState>,
) -> Result<ProviderDiagnosticsReport, CommandError> {
    provider_diagnostics_inner(&state).await
}

/// The full body of [`provider_diagnostics`], factored out so it can be driven
/// directly in tests without a live Tauri `State` (the `list_available_models`
/// testability pattern). It mirrors `list_available_models_inner`'s seeding,
/// resilient per-row registry build, and shared-embedded-instance setup EXACTLY,
/// so the diagnostics reflect the same enumeration the picker performs.
async fn provider_diagnostics_inner(
    state: &AppState,
) -> Result<ProviderDiagnosticsReport, CommandError> {
    let db = state.session_manager.db();

    // Mirror list_available_models_inner's seeding EXACTLY: it auto-seeds only
    // the Ollama provider-config row (idempotent) before reading the configs. It
    // does NOT call ensure_embedded_provider_config; the shared embedded instance
    // is inserted below and only enumerated when an embedded row is persisted.
    ensure_ollama_provider_config(state).await?;

    let configs: Vec<ProviderConfig> = ProviderRepo::new(db)
        .list()
        .await
        .map_err(|e| CommandError::internal(e.to_string()))?;

    // Load pricing to mirror list_available_models_inner's setup exactly and to
    // feed the SAME shared enumeration the picker runs (below). The diagnostics
    // only surface counts, so the resolved prices are not read per model here,
    // but the shared `list_available_models` signature takes `&pricing`, and
    // keeping the seeding + build + enumeration sequence identical to the picker
    // is exactly what guarantees the two cannot diverge.
    let app_config = AppConfig::load(db)
        .await
        .map_err(|e| CommandError::internal(e.to_string()))?;
    let pricing = pricing_table_from_config(&app_config.pricing);

    // Build the registry resiliently, EXACTLY as list_available_models_inner
    // does: start from the built-in factories and build each configured row
    // in isolation. A per-row build failure is NOT fatal here either; the row is
    // simply left un-built so the diagnostic reports instance_built == false with
    // a display-safe error.
    let mut registry = providers::builtin_registry();
    // Capture each per-row build failure (keyed by cfg.id) EXACTLY as
    // list_available_models_inner does, rather than discarding it. The per-row
    // failure is still non-fatal (the row is left un-built, instance_built ==
    // false), but the real ProviderError Display is threaded into the shared
    // enumeration below so the ProviderDiagnostic.error field carries the REAL
    // cause instead of the generic "no instance was built" message. DISPLAY-SAFE:
    // err.to_string() is a ProviderError Display (a reason, never key material).
    let mut build_errors: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();
    for cfg in &configs {
        if let Err(err) = registry.build_from_config(cfg, state.secret_store.as_ref()) {
            build_errors.insert(cfg.id.clone(), err.to_string());
        }
    }

    // Bridge the embedded engine seam exactly as list_available_models_inner
    // does: replace the throwaway embedded instance with one wrapping the SHARED
    // engine so the diagnostics observe the same imported models the picker does.
    registry.insert_instance(std::sync::Arc::new(
        providers::EmbeddedProvider::from_shared(state.embedded_engine.clone()),
    ));

    // Run the SAME shared enumeration the picker runs (the
    // `list_models_with_build_errors` alias for
    // `providers::list_available_models_with_build_errors`, fed the same captured
    // per-row build errors) exactly ONCE, then DERIVE the per-provider report
    // from its result. This is deliberately NOT a second
    // inline get/list_models/count-or-error loop: re-implementing the picker's
    // per-row logic here (including its "no instance was built" message) would
    // let the two silently drift, which would make the diagnostics misleading
    // (the exact failure this diagnostics surface exists to prevent). Consuming
    // the shared result makes the mirror structural: the model_count and error
    // of each row are, by construction, whatever the picker saw.
    let AvailableModelsResult { models, errors } =
        list_models_with_build_errors(&registry, &configs, &pricing, &build_errors)
            .await
            .map_err(|e| CommandError::internal(e.to_string()))?;

    // Derive each row's diagnostic from the shared enumeration result plus the
    // configs. `model_count` is how many of the enumerated models the shared fn
    // attributed to this row's id; `error` is the display-safe message of any
    // enumeration error the shared fn recorded for this row (an unreachable
    // endpoint OR the unbuilt-row case, which now carries the REAL captured
    // build-failure cause when one was captured, falling back to the generic
    // "configured but no instance built" string otherwise, under the same
    // provider_id).
    let mut diagnostics = Vec::with_capacity(configs.len());
    let mut provider_count_with_models = 0usize;
    for cfg in &configs {
        let model_count = models.iter().filter(|m| m.provider_id == cfg.id).count();
        let error = errors
            .iter()
            .find(|e| e.provider_id == cfg.id)
            .map(|e| e.message.clone());

        if model_count > 0 {
            provider_count_with_models += 1;
        }

        diagnostics.push(ProviderDiagnostic {
            id: cfg.id.clone(),
            kind: cfg.kind,
            base_url: cfg.base_url.clone(),
            // Whether a live instance was built for this row (its factory build
            // succeeded), read directly from the SAME registry the shared
            // enumeration consulted.
            instance_built: registry.get(&cfg.id).is_some(),
            model_count,
            error,
        });
    }

    Ok(ProviderDiagnosticsReport {
        configured_count: configs.len(),
        total_model_count: models.len(),
        provider_count_with_models,
        providers: diagnostics,
    })
}

/// Derive the set of provider instance ids that are PROVABLY local
/// (architecture.md Section 6.2), from the concrete [`ProviderKind`] of each
/// configured provider. This is the fail-closed locality signal routing uses
/// for the privacy hard constraint (instead of trusting a zero price, which a
/// misconfigured cloud provider could carry):
///
///   - [`ProviderKind::LmStudio`], [`ProviderKind::Ollama`], and
///     [`ProviderKind::Embedded`] run on the local machine (the embedded engine
///     runs in-process), so they are always local.
///   - [`ProviderKind::GenericOpenAI`] is local ONLY when its endpoint is a
///     loopback host (`localhost` / `127.0.0.1` / `[::1]`); a generic endpoint
///     pointed at a remote host is treated as cloud.
///   - every other kind is cloud.
fn local_provider_ids(configs: &[ProviderConfig]) -> std::collections::BTreeSet<String> {
    configs
        .iter()
        .filter(|cfg| match cfg.kind {
            ProviderKind::LmStudio => true,
            ProviderKind::Ollama => true,
            ProviderKind::Embedded => true,
            ProviderKind::GenericOpenAI => {
                cfg.base_url.as_deref().is_some_and(is_loopback_endpoint)
            }
            _ => false,
        })
        .map(|cfg| cfg.id.clone())
        .collect()
}

/// Extract the bare host from a URL/endpoint string. This is the SINGLE shared
/// host parser used by both the loopback classifier ([`is_loopback_endpoint`],
/// the routing privacy gate) and the base-URL validator ([`validate_base_url`],
/// Section 9.3), so there is exactly one place that understands the
/// scheme/credentials/port/bracketed-IPv6 shapes.
///
/// It strips an optional `scheme://`, takes the authority up to the first `/`,
/// drops any `user:pass@` credentials and the `:port`, and unwraps a bracketed
/// IPv6 literal (`[::1]:1234` -> `::1`). The returned host is lowercased so
/// callers can match case-insensitively. An empty string means the input had no
/// recognizable host.
fn extract_host(url: &str) -> String {
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
    host.to_ascii_lowercase()
}

/// Whether `url` names a loopback host (a local endpoint). Conservative: any URL
/// we cannot confidently classify as loopback is treated as NON-local so the
/// privacy gate fails closed. Uses the shared [`extract_host`] parser.
fn is_loopback_endpoint(url: &str) -> bool {
    matches!(
        extract_host(url).as_str(),
        "localhost" | "127.0.0.1" | "::1"
    )
}

/// Whether `url` uses TLS (an `https://` or `wss://` scheme). Absent or
/// plaintext schemes (`http://`, none) are treated as NOT TLS.
///
/// Reached through `check_provider_base_url`, the provider-config write seam
/// that `set_local_runtime` now exercises on a non-test path.
fn is_tls_scheme(url: &str) -> bool {
    let scheme = url.split_once("://").map(|(s, _)| s).unwrap_or("");
    scheme.eq_ignore_ascii_case("https") || scheme.eq_ignore_ascii_case("wss")
}

/// Whether `host` is an obviously-dangerous internal target that a
/// user-supplied provider base_url must never reach (Section 9.3). Kept
/// deliberately NARROW so legitimate LAN LM Studio setups (private RFC-1918
/// ranges) are only WARNED on, not blocked:
///
///   - the cloud metadata IP `169.254.169.254` (AWS/GCP/Azure IMDS), and
///   - the whole IPv4 link-local block `169.254.0.0/16`, and
///   - the IPv6 link-local block `fe80::/10` (`fe80:`..`febf:` prefixes).
///
/// These are never a real inference endpoint; reaching them is almost always an
/// SSRF-shaped mistake, so we block them conservatively.
///
/// Reached through `check_provider_base_url`, the provider-config write seam
/// that `set_local_runtime` now exercises on a non-test path.
fn is_blocked_internal_host(host: &str) -> bool {
    // IPv4 link-local 169.254.0.0/16 (covers the 169.254.169.254 metadata IP).
    if let Some(rest) = host.strip_prefix("169.254.") {
        // Only treat it as link-local when the remaining octets are numeric,
        // so a hostname like "169.254.example.com" is not misclassified.
        let looks_numeric = rest
            .split('.')
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()));
        if looks_numeric {
            return true;
        }
    }
    // IPv6 link-local fe80::/10 => first hextet in fe80..=febf.
    if let Some(first) = host.split(':').next() {
        if first.len() == 4 {
            if let Ok(value) = u16::from_str_radix(first, 16) {
                if (0xfe80..=0xfebf).contains(&value) {
                    return true;
                }
            }
        }
    }
    false
}

/// The outcome of validating a user-supplied provider `base_url` against the
/// Section 9.3 local-network posture.
///
/// Produced through `check_provider_base_url`, the provider-config write seam
/// that `set_local_runtime` now exercises on a non-test path.
#[derive(Debug, PartialEq, Eq)]
enum BaseUrlVerdict {
    /// A loopback endpoint (`localhost` / `127.0.0.1` / `[::1]`). Accept
    /// silently: plaintext is fine on the local machine and this is the default
    /// LM Studio posture.
    AcceptLoopback,
    /// A non-loopback endpoint reached over TLS. Accept silently.
    AcceptTls,
    /// A non-loopback endpoint reached over plaintext `http://` (or no scheme).
    /// Accept, but carry a display-safe warning recommending TLS.
    AcceptWithWarning(String),
    /// An obviously-dangerous internal target (link-local / cloud metadata) or
    /// an unparseable endpoint. Reject with a display-safe reason.
    Blocked(String),
}

/// Validate a user-supplied provider `base_url` against architecture.md Section
/// 9.3 ("LM Studio and local-network endpoints"). This is the ENFORCEMENT point
/// for the base-URL posture; it is shared by both [`ProviderKind::LmStudio`] and
/// [`ProviderKind::GenericOpenAI`] inputs (Section 9.3 covers LM Studio and any
/// generic OpenAI-compatible provider). It reuses the single [`extract_host`]
/// parser (no duplicated host parsing with [`is_loopback_endpoint`]).
///
/// Policy (fail-safe, but deliberately not over-blocking private LANs):
///
///   - loopback host (`localhost` / `127.0.0.1` / `[::1]`) => accept silently.
///     The default LM Studio endpoint `http://localhost:1234/v1` lands here.
///   - obviously-dangerous internal target (IPv4 link-local `169.254.0.0/16`
///     incl. the `169.254.169.254` metadata IP, or IPv6 link-local `fe80::/10`)
///     => BLOCK. These are never a real endpoint and reaching them is an
///     SSRF-shaped mistake.
///   - unparseable / hostless input => BLOCK conservatively.
///   - non-loopback host over TLS (`https://`) => accept silently.
///   - non-loopback host over plaintext `http://` (or no scheme) => accept, but
///     WARN and recommend TLS (private RFC-1918 LAN endpoints legitimate LM
///     Studio setups use land here: warned, never blocked).
///
/// Reached through `check_provider_base_url`, the provider-config write seam
/// that `set_local_runtime` now exercises on a non-test path.
fn validate_base_url(url: &str) -> BaseUrlVerdict {
    let host = extract_host(url);
    if host.is_empty() {
        return BaseUrlVerdict::Blocked(format!(
            "base_url has no recognizable host and cannot be validated: {url:?}"
        ));
    }
    if matches!(host.as_str(), "localhost" | "127.0.0.1" | "::1") {
        return BaseUrlVerdict::AcceptLoopback;
    }
    if is_blocked_internal_host(&host) {
        return BaseUrlVerdict::Blocked(format!(
            "base_url points at a blocked internal/link-local address ({host}); \
             configure a loopback or reachable inference endpoint instead"
        ));
    }
    if is_tls_scheme(url) {
        return BaseUrlVerdict::AcceptTls;
    }
    BaseUrlVerdict::AcceptWithWarning(format!(
        "base_url {url:?} is a non-loopback endpoint served over plaintext HTTP; \
         prefer https:// so provider traffic is encrypted in transit"
    ))
}

/// Validate a user-supplied provider `base_url` for the provider-config write
/// path, mapping the [`BaseUrlVerdict`] onto the command conventions: a blocked
/// target becomes an [`CommandError::invalid`] BEFORE the value reaches the
/// core; an accepted target returns an optional display-safe warning (present
/// only for the non-loopback plaintext case) that the caller may surface.
///
/// # Enforcement seam (Section 9.3)
///
/// [`set_local_runtime`] is the `#[tauri::command]` that accepts a raw provider
/// `base_url` from the webview, so this is the enforcement point on that write
/// path: it is called before `ProviderRepo::insert` / `ProviderRepo::update` so
/// a blocked target is rejected before the value reaches the core. It is also
/// covered by the unit tests below.
fn check_provider_base_url(url: &str) -> Result<Option<String>, CommandError> {
    match validate_base_url(url) {
        BaseUrlVerdict::AcceptLoopback | BaseUrlVerdict::AcceptTls => Ok(None),
        BaseUrlVerdict::AcceptWithWarning(warning) => Ok(Some(warning)),
        BaseUrlVerdict::Blocked(reason) => Err(CommandError::invalid(reason)),
    }
}

// --- Local runtime configuration (Section 9.3 write path) ------------------

/// Upper bound on a user-supplied provider `base_url` (Section 9.2 "bounds").
const MAX_BASE_URL_LEN: usize = 2_048;

/// The stable persisted [`ProviderConfig::id`] for a user-configurable local
/// runtime kind. Keying on a fixed id PER KIND makes [`set_local_runtime`] an
/// idempotent upsert: re-saving the same kind UPDATES the one row instead of
/// inserting a duplicate, and [`list_local_runtimes`] / [`clear_local_runtime`]
/// can address the row without tracking a generated id. Only the two
/// user-configurable local kinds have an id; every other kind returns `None`
/// (Ollama ships with its own default, Embedded is owned by the embedded
/// lifecycle commands under [`EMBEDDED_PROVIDER_ID`], and cloud kinds are not
/// configured here). The ids are distinct from [`EMBEDDED_PROVIDER_ID`].
fn local_runtime_config_id(kind: ProviderKind) -> Option<&'static str> {
    match kind {
        ProviderKind::LmStudio => Some("lmstudio-local"),
        ProviderKind::GenericOpenAI => Some("generic-openai-local"),
        _ => None,
    }
}

/// A display-safe view of a configured local runtime for the webview. Carries
/// only non-secret fields (Section 9.1): the plaintext API key and any resolved
/// secret NEVER cross this boundary. `has_api_key` reports only WHETHER a key is
/// stored (as an opaque [`SecretRef`]), never the key itself; `warning` carries
/// the optional display-safe base_url advisory from [`check_provider_base_url`].
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalRuntimeConfigView {
    /// The stable per-kind config id (see [`local_runtime_config_id`]).
    pub id: String,
    /// The provider kind (LmStudio or GenericOpenAI).
    pub kind: ProviderKind,
    /// The persisted, validated base_url.
    pub base_url: String,
    /// Whether an API key is stored (as an opaque `SecretRef`), never the key.
    pub has_api_key: bool,
    /// Optional display-safe advisory (e.g. non-loopback plaintext HTTP), only
    /// surfaced on the write path; `None` when rehydrating existing rows.
    pub warning: Option<String>,
}

/// Notify the frontend that the set/availability of providers or models changed
/// so the model-selector store (`state/providers.ts`) refetches via
/// `list_available_models` (architecture.md Sections 7.3 / 8.2). Fire-and-forget
/// through [`AppState::core_events`], mirroring the `McpStateChanged` sends: a
/// dropped UI event must never fail the mutation that triggered it, so the send
/// result is intentionally ignored. Callers emit this only AFTER a provider-config
/// mutation has succeeded, so a validation/persist error emits nothing.
fn emit_providers_changed(state: &AppState) {
    let _ = state
        .core_events
        .send(orchestrator_core::CoreEvent::ProvidersChanged);
}

/// Persist (upsert) a user-configured local runtime: an OpenAI-compatible
/// server the user runs themselves (LM Studio or a generic OpenAI-compatible
/// endpoint). The entered `base_url` is validated through the Section 9.3
/// posture ([`check_provider_base_url`]) and the config is written through
/// [`ProviderRepo`], so the existing adapters, locality classifier, and routing
/// pick it up with no further changes.
///
/// The optional `api_key` is stored straight into the keystore and referenced
/// only as an opaque [`SecretRef`]; it never comes back across IPC. Tauri maps
/// the snake_case params to camelCase over the wire (`baseUrl`, `apiKey`).
#[tauri::command]
pub async fn set_local_runtime(
    state: tauri::State<'_, AppState>,
    kind: ProviderKind,
    base_url: String,
    api_key: Option<String>,
) -> Result<LocalRuntimeConfigView, CommandError> {
    set_local_runtime_and_notify(&state, kind, &base_url, api_key.as_deref()).await
}

/// The mutation-plus-notify seam behind [`set_local_runtime`]: run the
/// [`set_local_runtime_inner`] body and, only if it succeeds (the `?`
/// short-circuits on a validation/persist error before the emit), ask the
/// frontend providers store to refetch by emitting `ProvidersChanged` (bug B).
/// The `#[tauri::command]` wrapper is a one-line delegation to this seam so the
/// emit is exercised by the same call the wrapper makes; tests drive this seam
/// directly (a live `tauri::State` cannot be built offline). Fire-and-forget: a
/// dropped UI event must never fail the command (mirrors the McpStateChanged
/// sends).
async fn set_local_runtime_and_notify(
    state: &AppState,
    kind: ProviderKind,
    base_url: &str,
    api_key: Option<&str>,
) -> Result<LocalRuntimeConfigView, CommandError> {
    let view = set_local_runtime_inner(state, kind, base_url, api_key).await?;
    emit_providers_changed(state);
    Ok(view)
}

/// The full validate-then-store-then-upsert body of [`set_local_runtime`],
/// factored out so it can be driven directly in tests without a live Tauri
/// `State` (the `set_provider_secret_inner` testability pattern).
///
/// It (1) accepts ONLY the user-configurable local kinds (LmStudio,
/// GenericOpenAI), rejecting anything else with [`CommandError::invalid`]; (2)
/// trims and validates `base_url` via [`check_provider_base_url`], propagating a
/// Blocked target as an error before anything is persisted and capturing the
/// optional display-safe warning; (3) stores a non-empty `api_key` through the
/// same secret store as [`set_provider_secret`], keeping only the opaque
/// [`SecretRef`]; and (4) upserts the [`ProviderConfig`] by its stable per-kind
/// id (get -> update | insert), so re-saving the same kind never duplicates the
/// row. It returns a display-safe [`LocalRuntimeConfigView`] and NEVER the key.
async fn set_local_runtime_inner(
    state: &AppState,
    kind: ProviderKind,
    base_url: &str,
    api_key: Option<&str>,
) -> Result<LocalRuntimeConfigView, CommandError> {
    // (1) Only LM Studio and generic OpenAI-compatible runtimes are
    // user-configurable here.
    let id = local_runtime_config_id(kind).ok_or_else(|| {
        CommandError::invalid(
            "only LmStudio and GenericOpenAI local runtimes can be configured here",
        )
    })?;

    // (2) Validate the base_url against the Section 9.3 posture before it
    // reaches the core; a Blocked target errors and persists nothing.
    let base_url = base_url.trim();
    validate_nonempty("baseUrl", base_url, MAX_BASE_URL_LEN)?;
    let warning = check_provider_base_url(base_url)?;

    // Load any existing row up front: it decides insert-vs-update below and,
    // when this save carries no key, tells us which prior secret to delete so
    // nothing is orphaned under the stable per-kind handle (keychain hygiene,
    // Section 9.1).
    let repo = ProviderRepo::new(state.session_manager.db());
    let existing = repo
        .get(id)
        .await
        .map_err(|e| CommandError::internal(e.to_string()))?;

    // (3) Store the optional API key as an opaque SecretRef; keyless local
    // endpoints leave api_key_ref as None. On a keyless save, delete any secret
    // the previous row stored so it does not linger in the keychain unreferenced
    // (the store is idempotent, so deleting a missing entry is a no-op).
    let api_key_ref = match api_key {
        Some(key) if !key.is_empty() => {
            if key.len() > MAX_SECRET_LEN {
                return Err(CommandError::invalid(format!(
                    "`apiKey` exceeds the maximum length of {MAX_SECRET_LEN} bytes"
                )));
            }
            Some(
                state
                    .secret_store
                    .store(id, key)
                    .map_err(|e| CommandError::internal(e.to_string()))?,
            )
        }
        _ => {
            if let Some(prior_ref) = existing.as_ref().and_then(|cfg| cfg.api_key_ref.as_ref()) {
                state
                    .secret_store
                    .delete(prior_ref)
                    .map_err(|e| CommandError::internal(e.to_string()))?;
            }
            None
        }
    };
    let has_api_key = api_key_ref.is_some();

    // (4) Upsert the config by its stable per-kind id (get -> update | insert),
    // matching ProviderRepo::update's NotFound-on-missing contract.
    let config = ProviderConfig {
        id: id.to_string(),
        kind,
        base_url: Some(base_url.to_string()),
        api_key_ref,
        extra: serde_json::Value::Null,
    };
    if existing.is_some() {
        repo.update(&config)
            .await
            .map_err(|e| CommandError::internal(e.to_string()))?;
    } else {
        repo.insert(&config)
            .await
            .map_err(|e| CommandError::internal(e.to_string()))?;
    }

    Ok(LocalRuntimeConfigView {
        id: id.to_string(),
        kind,
        base_url: base_url.to_string(),
        has_api_key,
        warning,
    })
}

/// List the currently-configured local runtimes so the webview can rehydrate
/// its Local Runtimes section from the backend source of truth (not a UI-only
/// marker). Display-safe: each row carries only id/kind/base_url/hasApiKey and
/// never the key or a resolved secret.
#[tauri::command]
pub async fn list_local_runtimes(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<LocalRuntimeConfigView>, CommandError> {
    list_local_runtimes_inner(&state).await
}

/// The full body of [`list_local_runtimes`], factored out for direct testing.
/// It loads every persisted [`ProviderConfig`], keeps only the two
/// user-configurable local kinds addressed by their stable per-kind id, and
/// maps each to a display-safe [`LocalRuntimeConfigView`] (`warning` is `None`
/// when rehydrating an existing row; `has_api_key` reflects whether the row has
/// a stored `SecretRef`).
async fn list_local_runtimes_inner(
    state: &AppState,
) -> Result<Vec<LocalRuntimeConfigView>, CommandError> {
    let configs = ProviderRepo::new(state.session_manager.db())
        .list()
        .await
        .map_err(|e| CommandError::internal(e.to_string()))?;
    let views = configs
        .into_iter()
        .filter(|cfg| local_runtime_config_id(cfg.kind) == Some(cfg.id.as_str()))
        .map(|cfg| LocalRuntimeConfigView {
            id: cfg.id,
            kind: cfg.kind,
            base_url: cfg.base_url.unwrap_or_default(),
            has_api_key: cfg.api_key_ref.is_some(),
            warning: None,
        })
        .collect();
    Ok(views)
}

/// Remove a configured local runtime so the user can clear a previously-saved
/// LM Studio / generic OpenAI-compatible endpoint. Only the two
/// user-configurable local kinds are addressable; any other kind is rejected.
#[tauri::command]
pub async fn clear_local_runtime(
    state: tauri::State<'_, AppState>,
    kind: ProviderKind,
) -> Result<(), CommandError> {
    clear_local_runtime_and_notify(&state, kind).await
}

/// The mutation-plus-notify seam behind [`clear_local_runtime`]: run the
/// [`clear_local_runtime_inner`] body and, only on success, emit
/// `ProvidersChanged` so the model selector drops the cleared runtime's models
/// (bug B). The `#[tauri::command]` wrapper delegates to this seam in one line;
/// tests drive the seam directly.
async fn clear_local_runtime_and_notify(
    state: &AppState,
    kind: ProviderKind,
) -> Result<(), CommandError> {
    clear_local_runtime_inner(state, kind).await?;
    emit_providers_changed(state);
    Ok(())
}

/// The full body of [`clear_local_runtime`], factored out for direct testing.
/// It resolves the stable per-kind id (rejecting non-configurable kinds),
/// deletes any secret the row stored so no credential material is orphaned in
/// the keychain (Section 9.1 keychain hygiene), then deletes the row via
/// [`ProviderRepo`]; deleting a missing secret or row is a no-op.
async fn clear_local_runtime_inner(
    state: &AppState,
    kind: ProviderKind,
) -> Result<(), CommandError> {
    let id = local_runtime_config_id(kind).ok_or_else(|| {
        CommandError::invalid(
            "only LmStudio and GenericOpenAI local runtimes can be configured here",
        )
    })?;
    let repo = ProviderRepo::new(state.session_manager.db());
    // Resolve the row's stored SecretRef (if any) and delete the secret before
    // dropping the row, so clearing a runtime never leaves the key behind under
    // the stable per-kind handle. The store is idempotent (deleting a missing
    // entry is not an error).
    if let Some(config) = repo
        .get(id)
        .await
        .map_err(|e| CommandError::internal(e.to_string()))?
    {
        if let Some(secret_ref) = config.api_key_ref.as_ref() {
            state
                .secret_store
                .delete(secret_ref)
                .map_err(|e| CommandError::internal(e.to_string()))?;
        }
    }
    repo.delete(id)
        .await
        .map_err(|e| CommandError::internal(e.to_string()))
}

// --- Cloud providers (OpenAI / Anthropic / Gemini / Bedrock / Azure / Kiro) -

/// The stable per-kind config id for a user-configurable CLOUD provider, or
/// `None` for any kind that is NOT configured through the cloud path (the local
/// runtimes LmStudio/Ollama and the Embedded engine are owned by other command
/// families). Kiro is not a distinct [`ProviderKind`]; it is an
/// OpenAI-compatible endpoint, so the UI maps its "Kiro (OpenAI-compatible)"
/// choice to [`ProviderKind::GenericOpenAI`] with a required base_url, and that
/// kind's cloud id is `generic-openai-cloud`. These ids are DISTINCT from the
/// local-runtime ids ([`local_runtime_config_id`]), [`EMBEDDED_PROVIDER_ID`],
/// and [`OLLAMA_PROVIDER_ID`], so a cloud GenericOpenAI (Kiro) row never
/// collides with a local generic OpenAI-compatible runtime.
fn cloud_provider_config_id(kind: ProviderKind) -> Option<&'static str> {
    match kind {
        ProviderKind::OpenAI => Some("openai-cloud"),
        ProviderKind::Anthropic => Some("anthropic-cloud"),
        ProviderKind::Gemini => Some("gemini-cloud"),
        ProviderKind::Bedrock => Some("bedrock-cloud"),
        ProviderKind::Azure => Some("azure-cloud"),
        ProviderKind::GenericOpenAI => Some("generic-openai-cloud"),
        _ => None,
    }
}

/// A display-safe view of a configured cloud provider for the webview. Carries
/// only non-secret fields (Section 9.1): the plaintext API key and any resolved
/// secret NEVER cross this boundary. `has_api_key` reports only WHETHER a key is
/// stored (as an opaque [`SecretRef`]), never the key itself; `base_url` is the
/// persisted endpoint override (`None` when the adapter's default is used);
/// `warning` carries the optional display-safe base_url advisory from
/// [`check_provider_base_url`].
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudProviderConfigView {
    /// The stable per-kind config id (see [`cloud_provider_config_id`]).
    pub id: String,
    /// The provider kind (OpenAI/Anthropic/Gemini/Bedrock/Azure/GenericOpenAI).
    pub kind: ProviderKind,
    /// The persisted base_url override, or `None` when the adapter default is
    /// used (required and always present for GenericOpenAI/Kiro).
    pub base_url: Option<String>,
    /// Whether an API key is stored (as an opaque `SecretRef`), never the key.
    pub has_api_key: bool,
    /// Optional display-safe advisory (e.g. non-loopback plaintext HTTP), only
    /// surfaced on the write path; `None` when rehydrating existing rows.
    pub warning: Option<String>,
}

/// Persist (upsert) a user-configured cloud provider: a hosted provider the
/// user authenticates with an API key (OpenAI, Anthropic, Gemini, Bedrock,
/// Azure, or a generic OpenAI-compatible endpoint used for "Kiro"). The key is
/// stored straight into the keystore and referenced only as an opaque
/// [`SecretRef`]; it never comes back across IPC. The config is written through
/// [`ProviderRepo`], so the existing adapters, locality classifier, and routing
/// pick it up with no further changes and its models are enumerated by
/// [`list_available_models`].
///
/// `base_url` is REQUIRED for GenericOpenAI (Kiro, which has no default
/// endpoint) and OPTIONAL for the other cloud kinds (their adapters default the
/// base_url when `None`); when present it is validated through the Section 9.3
/// posture ([`check_provider_base_url`]). Tauri maps the snake_case params to
/// camelCase over the wire (`apiKey`, `baseUrl`).
#[tauri::command]
pub async fn set_cloud_provider(
    state: tauri::State<'_, AppState>,
    kind: ProviderKind,
    api_key: String,
    base_url: Option<String>,
) -> Result<CloudProviderConfigView, CommandError> {
    set_cloud_provider_and_notify(&state, kind, &api_key, base_url.as_deref()).await
}

/// The mutation-plus-notify seam behind [`set_cloud_provider`]: run the
/// [`set_cloud_provider_inner`] body and, only if it succeeds (the `?`
/// short-circuits on a validation/persist error before the emit), emit
/// `ProvidersChanged` so the saved provider's models become enumerable (bug B).
/// The `#[tauri::command]` wrapper delegates to this seam in one line; tests
/// drive the seam directly.
async fn set_cloud_provider_and_notify(
    state: &AppState,
    kind: ProviderKind,
    api_key: &str,
    base_url: Option<&str>,
) -> Result<CloudProviderConfigView, CommandError> {
    let view = set_cloud_provider_inner(state, kind, api_key, base_url).await?;
    emit_providers_changed(state);
    Ok(view)
}

/// The full validate-then-store-then-upsert body of [`set_cloud_provider`],
/// factored out so it can be driven directly in tests without a live Tauri
/// `State` (the established `_inner` testability pattern).
///
/// It (1) accepts ONLY the user-configurable cloud kinds (OpenAI, Anthropic,
/// Gemini, Bedrock, Azure, GenericOpenAI), rejecting anything else with
/// [`CommandError::invalid`]; (2) requires a non-empty `api_key` within
/// [`MAX_SECRET_LEN`] (cloud providers always need a key); (3) requires a
/// non-empty `base_url` for GenericOpenAI (Kiro) and validates any provided
/// `base_url` via [`check_provider_base_url`], capturing its optional
/// display-safe advisory into the view; (4) stores the key through the
/// same secret store as [`set_provider_secret`], keeping only the opaque
/// [`SecretRef`]; and (5) upserts the [`ProviderConfig`] by its stable per-kind
/// id (get -> update | insert), so re-saving the same kind never duplicates the
/// row. It returns a display-safe [`CloudProviderConfigView`] and NEVER the key.
async fn set_cloud_provider_inner(
    state: &AppState,
    kind: ProviderKind,
    api_key: &str,
    base_url: Option<&str>,
) -> Result<CloudProviderConfigView, CommandError> {
    // (1) Only the hosted cloud kinds are configurable through this path.
    let id = cloud_provider_config_id(kind).ok_or_else(|| {
        CommandError::invalid(
            "only OpenAI, Anthropic, Gemini, Bedrock, Azure, and generic OpenAI-compatible (Kiro) cloud providers can be configured here",
        )
    })?;

    // (2) A cloud provider always needs a key; reject an empty/oversized one.
    if api_key.is_empty() {
        return Err(CommandError::invalid("`apiKey` must not be empty"));
    }
    if api_key.len() > MAX_SECRET_LEN {
        return Err(CommandError::invalid(format!(
            "`apiKey` exceeds the maximum length of {MAX_SECRET_LEN} bytes"
        )));
    }

    // (3) GenericOpenAI (Kiro) has no default endpoint, so a base_url is
    // REQUIRED; the other cloud kinds default their base_url when None. Any
    // provided base_url is trimmed and validated through the Section 9.3
    // posture before anything is persisted (a Blocked target errors here); an
    // accepted non-loopback plaintext HTTP endpoint yields a display-safe
    // advisory carried back in the view (parity with `set_local_runtime`).
    let mut warning = None;
    let base_url = match base_url.map(str::trim) {
        Some(url) if !url.is_empty() => {
            validate_nonempty("baseUrl", url, MAX_BASE_URL_LEN)?;
            warning = check_provider_base_url(url)?;
            Some(url.to_string())
        }
        _ => {
            if kind == ProviderKind::GenericOpenAI {
                return Err(CommandError::invalid(
                    "`baseUrl` is required for a generic OpenAI-compatible (Kiro) provider",
                ));
            }
            None
        }
    };

    // Load any existing row up front so re-saving updates rather than inserts.
    let repo = ProviderRepo::new(state.session_manager.db());
    let existing = repo
        .get(id)
        .await
        .map_err(|e| CommandError::internal(e.to_string()))?;

    // (4) Store the key as an opaque SecretRef; only the handle is persisted.
    let api_key_ref = Some(
        state
            .secret_store
            .store(id, api_key)
            .map_err(|e| CommandError::internal(e.to_string()))?,
    );

    // (5) Upsert the config by its stable per-kind id (get -> update | insert),
    // matching ProviderRepo::update's NotFound-on-missing contract.
    let config = ProviderConfig {
        id: id.to_string(),
        kind,
        base_url: base_url.clone(),
        api_key_ref,
        extra: serde_json::Value::Null,
    };
    if existing.is_some() {
        repo.update(&config)
            .await
            .map_err(|e| CommandError::internal(e.to_string()))?;
    } else {
        repo.insert(&config)
            .await
            .map_err(|e| CommandError::internal(e.to_string()))?;
    }

    Ok(CloudProviderConfigView {
        id: id.to_string(),
        kind,
        base_url,
        has_api_key: true,
        warning,
    })
}

/// List the currently-configured cloud providers so the webview can rehydrate
/// its Providers & Keys section from the backend source of truth (not a UI-only
/// marker). Display-safe: each row carries only id/kind/baseUrl/hasApiKey and
/// never the key or a resolved secret.
#[tauri::command]
pub async fn list_cloud_providers(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<CloudProviderConfigView>, CommandError> {
    list_cloud_providers_inner(&state).await
}

/// The full body of [`list_cloud_providers`], factored out for direct testing.
/// It loads every persisted [`ProviderConfig`], keeps only the cloud kinds
/// addressed by their stable per-kind id, and maps each to a display-safe
/// [`CloudProviderConfigView`] (`warning` is `None` when rehydrating an existing
/// row; `has_api_key` reflects whether the row has a stored `SecretRef`).
async fn list_cloud_providers_inner(
    state: &AppState,
) -> Result<Vec<CloudProviderConfigView>, CommandError> {
    let configs = ProviderRepo::new(state.session_manager.db())
        .list()
        .await
        .map_err(|e| CommandError::internal(e.to_string()))?;
    let views = configs
        .into_iter()
        .filter(|cfg| cloud_provider_config_id(cfg.kind) == Some(cfg.id.as_str()))
        .map(|cfg| CloudProviderConfigView {
            id: cfg.id,
            kind: cfg.kind,
            base_url: cfg.base_url,
            has_api_key: cfg.api_key_ref.is_some(),
            warning: None,
        })
        .collect();
    Ok(views)
}

/// Remove a configured cloud provider so the user can clear a previously-saved
/// key/endpoint. Only the user-configurable cloud kinds are addressable; any
/// other kind is rejected.
#[tauri::command]
pub async fn clear_cloud_provider(
    state: tauri::State<'_, AppState>,
    kind: ProviderKind,
) -> Result<(), CommandError> {
    clear_cloud_provider_and_notify(&state, kind).await
}

/// The mutation-plus-notify seam behind [`clear_cloud_provider`]: run the
/// [`clear_cloud_provider_inner`] body and, only on success, emit
/// `ProvidersChanged` so the model selector drops the cleared provider's models
/// (bug B). The `#[tauri::command]` wrapper delegates to this seam in one line;
/// tests drive the seam directly.
async fn clear_cloud_provider_and_notify(
    state: &AppState,
    kind: ProviderKind,
) -> Result<(), CommandError> {
    clear_cloud_provider_inner(state, kind).await?;
    emit_providers_changed(state);
    Ok(())
}

/// The full body of [`clear_cloud_provider`], factored out for direct testing.
/// It resolves the stable per-kind id (rejecting non-cloud kinds), deletes any
/// secret the row stored so no credential material is orphaned in the keychain
/// (Section 9.1 keychain hygiene), then deletes the row via [`ProviderRepo`];
/// deleting a missing secret or row is a no-op.
async fn clear_cloud_provider_inner(
    state: &AppState,
    kind: ProviderKind,
) -> Result<(), CommandError> {
    let id = cloud_provider_config_id(kind).ok_or_else(|| {
        CommandError::invalid(
            "only OpenAI, Anthropic, Gemini, Bedrock, Azure, and generic OpenAI-compatible (Kiro) cloud providers can be configured here",
        )
    })?;
    let repo = ProviderRepo::new(state.session_manager.db());
    // Resolve the row's stored SecretRef (if any) and delete the secret before
    // dropping the row, so clearing a provider never leaves the key behind under
    // the stable per-kind handle. The store is idempotent (deleting a missing
    // entry is not an error).
    if let Some(config) = repo
        .get(id)
        .await
        .map_err(|e| CommandError::internal(e.to_string()))?
    {
        if let Some(secret_ref) = config.api_key_ref.as_ref() {
            state
                .secret_store
                .delete(secret_ref)
                .map_err(|e| CommandError::internal(e.to_string()))?;
        }
    }
    repo.delete(id)
        .await
        .map_err(|e| CommandError::internal(e.to_string()))
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
    // Routing only needs the successful model candidates; enumeration errors are
    // surfaced through the `list_available_models` command's UI, not routing.
    let available = list_models(&registry, &configs, &pricing)
        .await
        .map_err(|e| CommandError::internal(e.to_string()))?
        .models;
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
    // Snapshot the LIVE connected MCP handle set (Section 5.3): the registry is
    // mutated at runtime by the MCP-manager commands, so the pipeline attaches
    // tools from whatever servers are currently registered/connected.
    let servers = state.mcp_servers.snapshot().await;
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

// --- Conversation messages / routing / persona (Section 8.1 / 8.2 / 8.5) ----

/// Fetch the ordered message history for a conversation (architecture.md
/// Section 8.1 chat surface; also used on resume, Section 8.5). Validates the
/// id and delegates to [`SessionManager::list_messages`].
#[tauri::command]
pub async fn get_messages(
    state: tauri::State<'_, AppState>,
    conversation_id: String,
) -> Result<Vec<Message>, CommandError> {
    let id = parse_uuid("conversationId", &conversation_id)?;
    state
        .session_manager
        .list_messages(id)
        .await
        .map_err(CommandError::from)
}

/// Pin (or clear) the per-conversation route (architecture.md Section 6.3 /
/// 8.2). Passing `route = None` returns the conversation to Automatic routing.
/// A pinned route is validated (both fields present + bounded) before it is
/// persisted; the same hard-constraint safety gate the pipeline applies still
/// runs at send time.
#[tauri::command]
pub async fn set_conversation_route(
    state: tauri::State<'_, AppState>,
    conversation_id: String,
    route: Option<ManualRoute>,
) -> Result<Conversation, CommandError> {
    let id = parse_uuid("conversationId", &conversation_id)?;
    if let Some(route) = &route {
        validate_manual_route(route)?;
    }
    state
        .session_manager
        .set_conversation_route(id, route)
        .await
        .map_err(CommandError::from)
}

/// Set (or clear) a conversation's routing mode (architecture.md Section 6.1 /
/// 8.2). The segmented UI toggle maps Auto / Prefer Local / Prefer Quality /
/// Manual onto a `domain::RoutingMode`, which the pipeline turns into the
/// existing `RoutingHint` bias via `RoutingMode::effective_hint`. Passing
/// `mode = None` returns the conversation to the default Auto behavior (no
/// conversation-level hint bias). The mode never relaxes the privacy hard
/// constraint, which stays fail-closed in the routing policy.
#[tauri::command]
pub async fn set_conversation_routing_mode(
    state: tauri::State<'_, AppState>,
    conversation_id: String,
    mode: Option<RoutingMode>,
) -> Result<Conversation, CommandError> {
    set_conversation_routing_mode_inner(&state, &conversation_id, mode).await
}

/// The full validate-then-delegate body of [`set_conversation_routing_mode`],
/// factored out so it can be driven directly in tests without a live Tauri
/// `State` (the established `_inner` testability pattern). The `mode` is already
/// validated by enum membership through serde deserialization; this parses the
/// id and delegates to [`SessionManager::set_conversation_routing_mode`].
async fn set_conversation_routing_mode_inner(
    state: &AppState,
    conversation_id: &str,
    mode: Option<RoutingMode>,
) -> Result<Conversation, CommandError> {
    let id = parse_uuid("conversationId", conversation_id)?;
    state
        .session_manager
        .set_conversation_routing_mode(id, mode)
        .await
        .map_err(CommandError::from)
}

/// Assign (or clear) a persona for a conversation (architecture.md Section 8.4 /
/// 8.5). Passing `persona_id = None` detaches any assigned persona. Validates
/// both ids before delegating to [`SessionManager::assign_persona`].
#[tauri::command]
pub async fn assign_persona(
    state: tauri::State<'_, AppState>,
    conversation_id: String,
    persona_id: Option<String>,
) -> Result<Conversation, CommandError> {
    let id = parse_uuid("conversationId", &conversation_id)?;
    let persona_id = match &persona_id {
        Some(p) => Some(parse_uuid("personaId", p)?),
        None => None,
    };
    state
        .session_manager
        .assign_persona(id, persona_id)
        .await
        .map_err(CommandError::from)
}

/// A display-safe preview of how the active conversation would be routed
/// (architecture.md Section 8.2 `get_route_explanation`), for the model
/// selector's "why this model" tooltip.
///
/// DISPLAY-SAFE (Section 9.1 / 9.2): carries only provider/model labels, a
/// human-readable rationale, and the route `source`; never secrets. `provider`
/// / `model` are `None` when the route is Automatic and no prior message has yet
/// recorded a decision (nothing to preview until the first turn runs).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteExplanation {
    pub rationale: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub source: RouteSource,
}

/// Explain how the active conversation is routed (architecture.md Section 8.2).
///
/// A full dry-run route resolution is heavy (it rebuilds the provider registry
/// and may touch the network per provider), so this returns a display-safe
/// PREVIEW derived from persisted state instead, in the same precedence order
/// the pipeline applies (Section 6.3): a per-conversation pin wins
/// (`ConversationPin`); otherwise the persona's default route previews as
/// `Automatic`; otherwise the last answered message's recorded route metadata is
/// echoed; otherwise a plain "automatic routing" note with no provider/model.
#[tauri::command]
pub async fn get_route_explanation(
    state: tauri::State<'_, AppState>,
    conversation_id: String,
) -> Result<RouteExplanation, CommandError> {
    get_route_explanation_inner(&state, &conversation_id).await
}

/// The full body of [`get_route_explanation`], extracted so it can be driven in
/// tests without a live Tauri `State` (the established `_inner` pattern).
async fn get_route_explanation_inner(
    state: &AppState,
    conversation_id: &str,
) -> Result<RouteExplanation, CommandError> {
    let id = parse_uuid("conversationId", conversation_id)?;
    let conversation = state
        .session_manager
        .get_conversation(id)
        .await?
        .ok_or_else(|| CommandError::not_found(format!("conversation not found: {id}")))?;

    // 1. A per-conversation pin previews as ConversationPin (Section 6.3).
    if let Some(pin) = &conversation.conversation_pref {
        return Ok(RouteExplanation {
            rationale: format!("conversation pinned to {}/{}", pin.provider_id, pin.model),
            provider_id: Some(pin.provider_id.clone()),
            model: Some(pin.model.clone()),
            source: RouteSource::ConversationPin,
        });
    }

    // 2. An assigned persona's default route previews as Automatic (the
    //    automatic policy honors it as a bias, Section 6.1).
    if let Some(persona_id) = conversation.persona_id {
        if let Some(persona) = state
            .session_manager
            .personas()
            .get(persona_id)
            .await
            .map_err(|e| CommandError::internal(e.to_string()))?
        {
            if let Some(route) = &persona.default_route {
                return Ok(RouteExplanation {
                    rationale: format!(
                        "persona `{}` default route {}/{}",
                        persona.name, route.provider_id, route.model
                    ),
                    provider_id: Some(route.provider_id.clone()),
                    model: Some(route.model.clone()),
                    source: RouteSource::Automatic,
                });
            }
        }
    }

    // 3. Echo the last answered message's recorded route metadata, if any.
    let messages = state.session_manager.list_messages(id).await?;
    if let Some(route) = messages.iter().rev().find_map(|m| m.route.clone()) {
        return Ok(RouteExplanation {
            rationale: route.rationale,
            provider_id: Some(route.provider_id),
            model: Some(route.model),
            source: route.source,
        });
    }

    // 4. Nothing to preview yet: automatic routing decides at send time.
    Ok(RouteExplanation {
        rationale: "automatic routing will choose a model at send time".to_string(),
        provider_id: None,
        model: None,
        source: RouteSource::Automatic,
    })
}

// --- MCP server management (Section 8.3) ------------------------------------

/// Validate a transport's fields (Section 9.2 "bounds"): stdio requires a
/// non-empty command; httpSse requires a non-empty url; args/env/headers are
/// bounded in count and length.
fn validate_transport(transport: &McpTransport) -> Result<(), CommandError> {
    match transport {
        McpTransport::Stdio { command, args, env } => {
            validate_nonempty("transport.command", command, MAX_COMMAND_LEN)?;
            if args.len() > MAX_MCP_LIST_ITEMS {
                return Err(CommandError::invalid(format!(
                    "too many transport args (max {MAX_MCP_LIST_ITEMS})"
                )));
            }
            for arg in args {
                if arg.chars().count() > MAX_COMMAND_LEN {
                    return Err(CommandError::invalid(
                        "a transport arg exceeds the maximum length",
                    ));
                }
            }
            validate_kv_pairs("transport.env", env)?;
        }
        McpTransport::HttpSse { url, headers } => {
            validate_nonempty("transport.url", url, MAX_URL_LEN)?;
            validate_kv_pairs("transport.headers", headers)?;
        }
    }
    Ok(())
}

/// Validate a bounded list of `(key, value)` pairs (env vars / http headers).
fn validate_kv_pairs(field: &str, pairs: &[(String, String)]) -> Result<(), CommandError> {
    if pairs.len() > MAX_MCP_LIST_ITEMS {
        return Err(CommandError::invalid(format!(
            "too many `{field}` entries (max {MAX_MCP_LIST_ITEMS})"
        )));
    }
    for (key, value) in pairs {
        validate_nonempty(&format!("{field}.key"), key, MAX_ENV_KEY_LEN)?;
        if value.chars().count() > MAX_ENV_VALUE_LEN {
            return Err(CommandError::invalid(format!(
                "`{field}` value exceeds the maximum length of {MAX_ENV_VALUE_LEN}"
            )));
        }
    }
    Ok(())
}

/// Input for [`add_mcp_server`] / [`update_mcp_server`]. Mirrors the persisted
/// [`McpServerConfig`] minus the id (assigned by the command on add, taken from
/// the path on update). Uses the same `McpTransport` serde representation.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerInput {
    pub name: String,
    pub transport: McpTransport,
    pub permission_mode: PermissionMode,
    #[serde(default)]
    pub enabled: bool,
}

impl McpServerInput {
    /// Validate every field and materialize an [`McpServerConfig`] with `id`.
    fn into_config(self, id: Uuid) -> Result<McpServerConfig, CommandError> {
        validate_nonempty("name", &self.name, MAX_NAME_LEN)?;
        validate_transport(&self.transport)?;
        Ok(McpServerConfig {
            id,
            name: self.name,
            transport: self.transport,
            permission_mode: self.permission_mode,
            enabled: self.enabled,
        })
    }
}

/// List every configured MCP server (architecture.md Section 8.3). The returned
/// [`McpServerConfig`] rows are display-safe: transport env/headers carry only
/// what the user entered (no secret material is stored here; provider keys live
/// in the keystore).
#[tauri::command]
pub async fn list_mcp_servers(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<McpServerConfig>, CommandError> {
    McpServerRepo::new(state.session_manager.db())
        .list()
        .await
        .map_err(|e| CommandError::internal(e.to_string()))
}

/// Add a new MCP server (architecture.md Section 8.3): validate, assign an id,
/// persist, register a handle, and connect it in the background when enabled.
#[tauri::command]
pub async fn add_mcp_server(
    state: tauri::State<'_, AppState>,
    config: McpServerInput,
) -> Result<McpServerConfig, CommandError> {
    let cfg = config.into_config(Uuid::new_v4())?;
    McpServerRepo::new(state.session_manager.db())
        .insert(&cfg)
        .await
        .map_err(|e| CommandError::internal(e.to_string()))?;

    let handle = Arc::new(McpServerHandle::new(cfg.clone()));
    state.mcp_servers.insert(cfg.id, handle.clone()).await;
    if cfg.enabled {
        spawn_connect(handle, cfg.id, state.core_events.clone());
    }
    Ok(cfg)
}

/// Update an existing MCP server (architecture.md Section 8.3): validate,
/// persist the new row, then replace the handle and reconnect it in the
/// background when the server is enabled (tearing down the previous handle).
#[tauri::command]
pub async fn update_mcp_server(
    state: tauri::State<'_, AppState>,
    id: String,
    config: McpServerInput,
) -> Result<McpServerConfig, CommandError> {
    let server_id = parse_uuid("id", &id)?;
    let cfg = config.into_config(server_id)?;
    let repo = McpServerRepo::new(state.session_manager.db());
    if repo
        .get(server_id)
        .await
        .map_err(|e| CommandError::internal(e.to_string()))?
        .is_none()
    {
        return Err(CommandError::not_found(format!(
            "mcp server not found: {server_id}"
        )));
    }
    repo.update(&cfg)
        .await
        .map_err(|e| CommandError::internal(e.to_string()))?;

    // Replace the handle with one built from the new config; tear down the old.
    let handle = Arc::new(McpServerHandle::new(cfg.clone()));
    if let Some(previous) = state.mcp_servers.insert(server_id, handle.clone()).await {
        previous.teardown().await;
    }
    if cfg.enabled {
        spawn_connect(handle, server_id, state.core_events.clone());
    }
    Ok(cfg)
}

/// Remove an MCP server (architecture.md Section 8.3): tear down its live handle
/// and delete the persisted row.
#[tauri::command]
pub async fn remove_mcp_server(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<(), CommandError> {
    let server_id = parse_uuid("id", &id)?;
    if let Some(handle) = state.mcp_servers.remove(server_id).await {
        handle.teardown().await;
    }
    McpServerRepo::new(state.session_manager.db())
        .delete(server_id)
        .await
        .map_err(|e| CommandError::internal(e.to_string()))
}

/// Enable or disable an MCP server (architecture.md Section 8.3): persist the
/// `enabled` flag and connect (background) or tear down the live handle to
/// match.
#[tauri::command]
pub async fn set_mcp_enabled(
    state: tauri::State<'_, AppState>,
    id: String,
    enabled: bool,
) -> Result<McpServerConfig, CommandError> {
    let server_id = parse_uuid("id", &id)?;
    let repo = McpServerRepo::new(state.session_manager.db());
    let mut cfg = repo
        .get(server_id)
        .await
        .map_err(|e| CommandError::internal(e.to_string()))?
        .ok_or_else(|| CommandError::not_found(format!("mcp server not found: {server_id}")))?;
    cfg.enabled = enabled;
    repo.update(&cfg)
        .await
        .map_err(|e| CommandError::internal(e.to_string()))?;

    if enabled {
        // Register a fresh handle and connect it in the background.
        let handle = Arc::new(McpServerHandle::new(cfg.clone()));
        if let Some(previous) = state.mcp_servers.insert(server_id, handle.clone()).await {
            previous.teardown().await;
        }
        spawn_connect(handle, server_id, state.core_events.clone());
    } else if let Some(handle) = state.mcp_servers.remove(server_id).await {
        // Remove (not just tear down) the handle so the disabled server no longer
        // appears in send_message's live snapshot (review issue #4, mirroring
        // remove_mcp_server). On re-enable the branch above re-inserts + connects.
        handle.teardown().await;
        let _ = state
            .core_events
            .send(orchestrator_core::CoreEvent::McpStateChanged {
                server_id,
                state: orchestrator_core::McpConnectionState::Disconnected,
            });
    }
    Ok(cfg)
}

/// Refresh an MCP server's tool list (architecture.md Section 8.3): reconnect
/// the live handle (which re-runs the `tools/list` handshake) and return the
/// display-safe descriptors. A server with no registered handle is an error.
#[tauri::command]
pub async fn refresh_mcp_tools(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<Vec<ToolDescriptorView>, CommandError> {
    let server_id = parse_uuid("id", &id)?;
    let handle =
        state.mcp_servers.get(server_id).await.ok_or_else(|| {
            CommandError::not_found(format!("mcp server not connected: {server_id}"))
        })?;
    // Reconnect re-runs the handshake + tools/list, refreshing the cache.
    if let Err(err) = handle.reconnect().await {
        let _ = state
            .core_events
            .send(orchestrator_core::CoreEvent::McpError {
                server_id,
                message: err.to_string(),
            });
        return Err(CommandError::internal(err.to_string()));
    }
    let _ = state
        .core_events
        .send(orchestrator_core::CoreEvent::McpStateChanged {
            server_id,
            state: orchestrator_core::McpConnectionState::Connected,
        });
    Ok(handle
        .tools()
        .await
        .into_iter()
        .map(ToolDescriptorView::from_descriptor)
        .collect())
}

/// A display-safe view of an MCP tool descriptor (architecture.md Section 5.4 /
/// 8.3). Mirrors the mcp-client `ToolDescriptor` fields the Tool Inspector
/// surface shows; carries no secret material.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDescriptorView {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

impl ToolDescriptorView {
    fn from_descriptor(d: mcp_client::ToolDescriptor) -> Self {
        ToolDescriptorView {
            name: d.name,
            description: d.description,
            input_schema: d.input_schema,
        }
    }
}

/// Set an MCP server's tool-invocation permission mode (architecture.md Section
/// 5.6 / 8.3). Persists the per-server [`McpServerConfig::permission_mode`].
///
/// PER-TOOL OVERRIDE LIMITATION: the Phase 5 schema stores a single per-server
/// `permission_mode` (there is no per-tool override column), so `tool_name` is
/// accepted for forward compatibility but a per-tool override is NOT persisted;
/// when `tool_name` is `Some`, the command still sets the server-wide mode. A
/// dedicated per-tool-override store is deferred to a later phase rather than
/// inventing a new schema here.
#[tauri::command]
pub async fn set_tool_permission(
    state: tauri::State<'_, AppState>,
    server_id: String,
    tool_name: Option<String>,
    mode: PermissionMode,
) -> Result<McpServerConfig, CommandError> {
    let id = parse_uuid("serverId", &server_id)?;
    if let Some(name) = &tool_name {
        validate_nonempty("toolName", name, MAX_NAME_LEN)?;
    }
    let repo = McpServerRepo::new(state.session_manager.db());
    let mut cfg = repo
        .get(id)
        .await
        .map_err(|e| CommandError::internal(e.to_string()))?
        .ok_or_else(|| CommandError::not_found(format!("mcp server not found: {id}")))?;
    cfg.permission_mode = mode;
    repo.update(&cfg)
        .await
        .map_err(|e| CommandError::internal(e.to_string()))?;
    Ok(cfg)
}

// --- Conversation export / resume / cancellation (Section 8.1 / 8.5) --------

/// The serialization format for [`export_conversation`] (architecture.md Section
/// 8.5). A unit enum serialized per-variant camelCase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExportFormat {
    Markdown,
    Json,
}

/// Export a conversation and its messages to a display-safe string
/// (architecture.md Section 8.5). Markdown renders a readable transcript; JSON
/// bundles the conversation + messages verbatim. Never includes secret material.
#[tauri::command]
pub async fn export_conversation(
    state: tauri::State<'_, AppState>,
    conversation_id: String,
    format: ExportFormat,
) -> Result<String, CommandError> {
    export_conversation_inner(&state, &conversation_id, format).await
}

/// The full body of [`export_conversation`], extracted for the `_inner` test
/// pattern (drivable without a live Tauri `State`).
async fn export_conversation_inner(
    state: &AppState,
    conversation_id: &str,
    format: ExportFormat,
) -> Result<String, CommandError> {
    let id = parse_uuid("conversationId", conversation_id)?;
    let conversation = state
        .session_manager
        .get_conversation(id)
        .await?
        .ok_or_else(|| CommandError::not_found(format!("conversation not found: {id}")))?;
    let messages = state.session_manager.list_messages(id).await?;

    match format {
        ExportFormat::Json => {
            let payload = json!({ "conversation": conversation, "messages": messages });
            serde_json::to_string_pretty(&payload)
                .map_err(|e| CommandError::internal(e.to_string()))
        }
        ExportFormat::Markdown => Ok(render_markdown(&conversation, &messages)),
    }
}

/// Render a conversation + messages as a readable Markdown transcript.
fn render_markdown(conversation: &Conversation, messages: &[Message]) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(out, "# {}", conversation.title);
    let _ = writeln!(out);
    for message in messages {
        let role = match message.role {
            orchestrator_core::Role::System => "System",
            orchestrator_core::Role::User => "User",
            orchestrator_core::Role::Assistant => "Assistant",
            orchestrator_core::Role::Tool => "Tool",
        };
        let _ = writeln!(out, "## {role}");
        let _ = writeln!(out);
        let _ = writeln!(out, "{}", render_content(&message.content));
        let _ = writeln!(out);
    }
    out
}

/// Render a message's content as display-safe Markdown-friendly text.
fn render_content(content: &orchestrator_core::MessageContent) -> String {
    use orchestrator_core::MessageContent;
    match content {
        MessageContent::Text { text } => text.clone(),
        MessageContent::ToolCalls { calls } => calls
            .iter()
            .map(|c| format!("_tool call: {}_", c.name))
            .collect::<Vec<_>>()
            .join("\n"),
        MessageContent::ToolResults { results } => results
            .iter()
            .map(|r| {
                format!(
                    "_tool result ({})_",
                    if r.is_error { "error" } else { "ok" }
                )
            })
            .collect::<Vec<_>>()
            .join("\n"),
        MessageContent::Attachments { attachments } => attachments
            .iter()
            .map(|a| format!("_attachment: {}_", a.mime_type))
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

/// The full resume payload returned by [`open_conversation`] (architecture.md
/// Section 8.5): the conversation (so the UI can restore persona/route pin/tags/
/// enabled tools) plus its message history in one round-trip. Display-safe.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenedConversation {
    pub conversation: Conversation,
    pub messages: Vec<Message>,
}

/// Load a conversation and its messages for resume (architecture.md Section
/// 8.5). Returns the conversation bundled with its message history so the chat
/// surface can restore the full session in one call.
#[tauri::command]
pub async fn open_conversation(
    state: tauri::State<'_, AppState>,
    conversation_id: String,
) -> Result<OpenedConversation, CommandError> {
    let id = parse_uuid("conversationId", &conversation_id)?;
    let conversation = state
        .session_manager
        .get_conversation(id)
        .await?
        .ok_or_else(|| CommandError::not_found(format!("conversation not found: {id}")))?;
    let messages = state.session_manager.list_messages(id).await?;
    Ok(OpenedConversation {
        conversation,
        messages,
    })
}

/// Request cancellation of an in-flight generation for a conversation
/// (architecture.md Section 8.1 Composer stop button).
///
/// CANCELLATION LIMITATION: the Phase 4 pipeline (`run_turn`) has no
/// cancellation seam yet (no cancel token threaded through the streaming loop),
/// so wiring a real interrupt is out of Phase 5 scope. This command is therefore
/// a VALIDATED NO-OP: it verifies the conversation id parses and exists, then
/// returns `Ok(())` so the UI's stop control has a command to call. A real
/// cancel-flag registry checked by `run_turn` is deferred to a later phase; the
/// limitation is recorded in the feature findings.
#[tauri::command]
pub async fn stop_generation(
    state: tauri::State<'_, AppState>,
    conversation_id: String,
) -> Result<(), CommandError> {
    let id = parse_uuid("conversationId", &conversation_id)?;
    // Validate the conversation exists so the no-op still rejects bad ids.
    if state.session_manager.get_conversation(id).await?.is_none() {
        return Err(CommandError::not_found(format!(
            "conversation not found: {id}"
        )));
    }
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

// --- Embedded local inference engine (Strategy B / FEAT-002) ----------------

/// A display-safe view of one imported embedded (local `.gguf`) model
/// (architecture.md Section 4, Strategy B). Crosses the IPC boundary, so it is
/// camelCase-serde and carries ONLY the model id and its on-disk path: never any
/// secret material (the embedded engine needs no key).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddedModelView {
    /// The provider-scoped model id (defaults to the `.gguf` file stem).
    pub id: String,
    /// The on-disk path to the `.gguf` file, for display in the settings UI.
    pub path: String,
    /// Whether this model is the one currently selected/loaded.
    pub loaded: bool,
}

/// The embedded engine's lifecycle status (architecture.md Section 4, Strategy
/// B). Display-safe camelCase DTO: reports which model (if any) is currently
/// selected/loaded and how many local models are imported.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddedModelStatus {
    /// The id of the currently selected/loaded model, if any.
    pub loaded_model_id: Option<String>,
    /// The number of imported local `.gguf` models.
    pub registered_count: usize,
}

/// Upper bound on a supplied `.gguf` file path (Section 9.2 "bounds").
const MAX_MODEL_PATH_LEN: usize = 4_096;

/// The persisted [`ProviderConfig::id`] the embedded engine uses. It equals the
/// engine's [`ChatProvider::id`] (`engine::EMBEDDED_ENGINE_ID`) so the shared
/// `AppState.embedded_engine`, inserted into the registry under its
/// `ChatProvider::id`, matches this row's id in `list_available_models`'s
/// per-row `registry.get(&cfg.id)` lookup.
const EMBEDDED_PROVIDER_ID: &str = engine::EMBEDDED_ENGINE_ID;

/// Ensure a persisted [`ProviderKind::Embedded`] provider-config row exists so
/// imported local models become routable and surface under Local in the model
/// picker across routing modes (the spec's Phase 9 deliverable). Idempotent:
/// inserts the row on first import, and is a no-op once it exists.
///
/// The row carries no `base_url` (imported paths are absolute/self-contained)
/// and no `api_key_ref` (the in-process engine needs no key), so it is
/// display-safe and never touches the secret store. Its `id` is
/// [`EMBEDDED_PROVIDER_ID`] so it lines up with the shared engine instance the
/// enumerate path registers.
async fn ensure_embedded_provider_config(state: &AppState) -> Result<(), CommandError> {
    let repo = ProviderRepo::new(state.session_manager.db());
    let existing = repo
        .get(EMBEDDED_PROVIDER_ID)
        .await
        .map_err(|e| CommandError::internal(e.to_string()))?;
    if existing.is_some() {
        return Ok(());
    }
    let config = ProviderConfig {
        id: EMBEDDED_PROVIDER_ID.to_string(),
        kind: ProviderKind::Embedded,
        base_url: None,
        api_key_ref: None,
        extra: serde_json::Value::Null,
    };
    repo.insert(&config)
        .await
        .map_err(|e| CommandError::internal(e.to_string()))
}

/// The persisted [`ProviderConfig::id`] the auto-seeded Ollama provider uses. A
/// stable, distinct id (like [`EMBEDDED_PROVIDER_ID`]) so the enumerate path's
/// per-row `registry.get(&cfg.id)` lookup matches the built Ollama instance and
/// the idempotent seed never duplicates the row.
const OLLAMA_PROVIDER_ID: &str = "ollama-local";

/// Ensure a persisted [`ProviderKind::Ollama`] provider-config row exists so a
/// locally-running Ollama's installed models are auto-discovered (via the
/// adapter's native `GET /api/tags`) and surface under Local in the model picker
/// WITHOUT the user configuring anything. Idempotent: inserts the row on first
/// enumeration, and is a no-op once it exists.
///
/// The row carries no `base_url` (the Ollama adapter falls back to its own
/// `DEFAULT_BASE_URL`, `http://127.0.0.1:11434/v1`, when `base_url` is None) and
/// no `api_key_ref` (Ollama is keyless), so it is display-safe and never touches
/// the secret store. Seeding is safe even when Ollama is not installed or not
/// running: the enumeration path skips a provider whose `list_models` errors
/// (logging id + error), so an offline Ollama simply contributes no rows instead
/// of breaking the picker.
async fn ensure_ollama_provider_config(state: &AppState) -> Result<(), CommandError> {
    let repo = ProviderRepo::new(state.session_manager.db());
    let existing = repo
        .get(OLLAMA_PROVIDER_ID)
        .await
        .map_err(|e| CommandError::internal(e.to_string()))?;
    if existing.is_some() {
        return Ok(());
    }
    let config = ProviderConfig {
        id: OLLAMA_PROVIDER_ID.to_string(),
        kind: ProviderKind::Ollama,
        base_url: None,
        api_key_ref: None,
        extra: serde_json::Value::Null,
    };
    repo.insert(&config)
        .await
        .map_err(|e| CommandError::internal(e.to_string()))
}

/// Validate a user-supplied local `.gguf` model path: trimmed non-empty, within
/// bounds, and ending in the `.gguf` extension (case-insensitive). This rejects
/// obvious mistakes before the path reaches the engine registry; it does NOT
/// touch the filesystem (existence is checked lazily on the native load path).
fn validate_gguf_path(path: &str) -> Result<(), CommandError> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err(CommandError::invalid("`path` must not be empty"));
    }
    if trimmed.len() > MAX_MODEL_PATH_LEN {
        return Err(CommandError::invalid(format!(
            "`path` exceeds the maximum length of {MAX_MODEL_PATH_LEN} bytes"
        )));
    }
    if !trimmed.to_ascii_lowercase().ends_with(".gguf") {
        return Err(CommandError::invalid(
            "`path` must point to a `.gguf` model file",
        ));
    }
    Ok(())
}

/// Snapshot the imported embedded models as display-safe views, marking the
/// currently selected/loaded model. Shared by the list/import/select handlers so
/// they all return the same up-to-date view.
async fn embedded_model_views(state: &AppState) -> Vec<EmbeddedModelView> {
    let loaded = state.embedded_loaded_model.read().await.clone();
    state
        .embedded_engine
        .registered_entries()
        .await
        .into_iter()
        .map(|m| EmbeddedModelView {
            loaded: Some(&m.id) == loaded.as_ref(),
            path: m.path.to_string_lossy().into_owned(),
            id: m.id,
        })
        .collect()
}

/// List the imported embedded (local `.gguf`) models (architecture.md Section 4,
/// Strategy B). Display-safe: returns only id/path/loaded, never secret
/// material. Backed by the engine's model registry.
#[tauri::command]
pub async fn list_embedded_models(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<EmbeddedModelView>, CommandError> {
    Ok(embedded_model_views(&state).await)
}

/// Import a local `.gguf` model by path, registering it with the embedded
/// engine (architecture.md Section 4, Strategy B). Validates the path shape
/// (non-empty, bounded, `.gguf` extension) before registering; the file is
/// opened only later on the native load path. Returns the refreshed model list.
#[tauri::command]
pub async fn import_embedded_model(
    state: tauri::State<'_, AppState>,
    path: String,
) -> Result<Vec<EmbeddedModelView>, CommandError> {
    import_embedded_model_and_notify(&state, &path).await
}

/// The mutation-plus-notify seam behind [`import_embedded_model`]: validate and
/// register the `.gguf` path, build the refreshed model list, and only on
/// success emit `ProvidersChanged` (a newly imported model changes what
/// enumerates under Local, bug B). The `?` short-circuits on a bad path before
/// the emit. The `#[tauri::command]` wrapper delegates to this seam in one line;
/// tests drive the seam directly.
async fn import_embedded_model_and_notify(
    state: &AppState,
    path: &str,
) -> Result<Vec<EmbeddedModelView>, CommandError> {
    validate_gguf_path(path)?;
    ensure_embedded_provider_config(state).await?;
    state.embedded_engine.register_path(path.trim()).await;
    let views = embedded_model_views(state).await;
    emit_providers_changed(state);
    Ok(views)
}

/// Select (import if needed, then mark active) a local `.gguf` model by path
/// (architecture.md Section 4, Strategy B). Registers the path and records its
/// id as the selected/loaded model. Returns the refreshed model list.
#[tauri::command]
pub async fn select_embedded_model(
    state: tauri::State<'_, AppState>,
    path: String,
) -> Result<Vec<EmbeddedModelView>, CommandError> {
    select_embedded_model_and_notify(&state, &path).await
}

/// The mutation-plus-notify seam behind [`select_embedded_model`]: validate and
/// register the `.gguf` path, mark it active, build the refreshed model list,
/// and only on success emit `ProvidersChanged` (the active Local model changed,
/// bug B). The `?` short-circuits on a bad path before the emit. The
/// `#[tauri::command]` wrapper delegates to this seam in one line; tests drive
/// the seam directly.
async fn select_embedded_model_and_notify(
    state: &AppState,
    path: &str,
) -> Result<Vec<EmbeddedModelView>, CommandError> {
    validate_gguf_path(path)?;
    ensure_embedded_provider_config(state).await?;
    let id = state.embedded_engine.register_path(path.trim()).await;
    *state.embedded_loaded_model.write().await = Some(id);
    let views = embedded_model_views(state).await;
    emit_providers_changed(state);
    Ok(views)
}

/// Load (make active) an already-imported embedded model by id (architecture.md
/// Section 4, Strategy B). Rejects an id that is not registered. Records it as
/// the selected/loaded model and returns the engine status.
#[tauri::command]
pub async fn load_embedded_model(
    state: tauri::State<'_, AppState>,
    model_id: String,
) -> Result<EmbeddedModelStatus, CommandError> {
    load_embedded_model_and_notify(&state, model_id).await
}

/// The mutation-plus-notify seam behind [`load_embedded_model`]: validate the
/// id, reject an unregistered one, mark it active, build the engine status, and
/// only on success emit `ProvidersChanged` (the active Local model changed, bug
/// B). The `?`/early return short-circuits on an invalid or unknown id before
/// the emit. The `#[tauri::command]` wrapper delegates to this seam in one line;
/// tests drive the seam directly.
async fn load_embedded_model_and_notify(
    state: &AppState,
    model_id: String,
) -> Result<EmbeddedModelStatus, CommandError> {
    validate_nonempty("modelId", &model_id, MAX_MODEL_PATH_LEN)?;
    let registered = state.embedded_engine.registered_model_ids().await;
    if !registered.iter().any(|id| id == &model_id) {
        return Err(CommandError::not_found(format!(
            "no imported embedded model with id: {model_id}"
        )));
    }
    *state.embedded_loaded_model.write().await = Some(model_id);
    let status = embedded_model_status_inner(state).await;
    emit_providers_changed(state);
    Ok(status)
}

/// Unload the currently selected/loaded embedded model (architecture.md Section
/// 4, Strategy B), clearing the active-model state. Idempotent: unloading when
/// nothing is loaded is a no-op. Returns the engine status.
#[tauri::command]
pub async fn unload_embedded_model(
    state: tauri::State<'_, AppState>,
) -> Result<EmbeddedModelStatus, CommandError> {
    unload_embedded_model_and_notify(&state).await
}

/// The mutation-plus-notify seam behind [`unload_embedded_model`]: clear the
/// active-model state, build the engine status, and emit `ProvidersChanged`
/// (the Local surface changed, bug B). Idempotent: unloading when nothing is
/// loaded is a no-op that still notifies. The `#[tauri::command]` wrapper
/// delegates to this seam in one line; tests drive the seam directly.
async fn unload_embedded_model_and_notify(
    state: &AppState,
) -> Result<EmbeddedModelStatus, CommandError> {
    *state.embedded_loaded_model.write().await = None;
    let status = embedded_model_status_inner(state).await;
    emit_providers_changed(state);
    Ok(status)
}

/// Report the embedded engine's lifecycle status (architecture.md Section 4,
/// Strategy B): which model (if any) is selected/loaded and how many are
/// imported. Display-safe camelCase DTO.
#[tauri::command]
pub async fn embedded_model_status(
    state: tauri::State<'_, AppState>,
) -> Result<EmbeddedModelStatus, CommandError> {
    Ok(embedded_model_status_inner(&state).await)
}

/// The shared status snapshot behind [`embedded_model_status`],
/// [`load_embedded_model`], and [`unload_embedded_model`].
async fn embedded_model_status_inner(state: &AppState) -> EmbeddedModelStatus {
    let loaded_model_id = state.embedded_loaded_model.read().await.clone();
    let registered_count = state.embedded_engine.registered_model_ids().await.len();
    EmbeddedModelStatus {
        loaded_model_id,
        registered_count,
    }
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
    use orchestrator_core::{CoreEvent, SessionManager};
    use persistence::Db;
    use providers::AvailableModel;
    use secrets::{InMemorySecretStore, SecretError};
    use tokio::sync::mpsc::UnboundedReceiver;

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

    /// With no user-configured providers, the command body returns an empty
    /// MODEL list. The enumerate path auto-seeds an Ollama config row, but with
    /// no Ollama server running its `list_models` errors and the provider is
    /// skipped, so no models surface. This drives the extracted inner fn
    /// end-to-end (auto-seed Ollama, load configs + pricing, build the registry,
    /// enumerate) exactly as the `#[tauri::command]` wrapper does.
    #[tokio::test]
    async fn list_available_models_inner_empty_when_no_providers() {
        let state = test_state().await;
        let result = list_available_models_inner(&state).await.unwrap();
        assert!(result.models.is_empty());
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
            cfg("ollama", ProviderKind::Ollama, None),
            // The embedded engine runs in-process, so it is always local.
            cfg("embedded", ProviderKind::Embedded, None),
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
        // Ollama runs on the local machine, so it is always local.
        assert!(local.contains("ollama"));
        // The embedded engine runs in-process, so it is always local.
        assert!(local.contains("embedded"));
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

    /// The base-URL validator (Section 9.3) accepts loopback plaintext silently
    /// (the default LM Studio endpoint), accepts remote TLS silently, warns +
    /// recommends TLS for non-loopback plaintext, and blocks link-local /
    /// metadata internal targets. It shares the [`extract_host`] parser with
    /// [`is_loopback_endpoint`].
    #[test]
    fn validate_base_url_enforces_local_network_posture() {
        // Default LM Studio endpoint: loopback plaintext -> accept, no warning.
        assert_eq!(
            validate_base_url("http://localhost:1234/v1"),
            BaseUrlVerdict::AcceptLoopback
        );
        // Other loopback forms -> accept.
        assert_eq!(
            validate_base_url("http://127.0.0.1:1234"),
            BaseUrlVerdict::AcceptLoopback
        );
        assert_eq!(
            validate_base_url("http://[::1]:1234"),
            BaseUrlVerdict::AcceptLoopback
        );
        // Remote over TLS -> accept, no warning.
        assert_eq!(
            validate_base_url("https://api.example.com"),
            BaseUrlVerdict::AcceptTls
        );
        // Remote over plaintext -> accept WITH a TLS-recommending warning.
        match validate_base_url("http://api.example.com") {
            BaseUrlVerdict::AcceptWithWarning(msg) => {
                assert!(
                    msg.contains("https://"),
                    "warning should recommend TLS: {msg}"
                );
            }
            other => panic!("expected AcceptWithWarning, got {other:?}"),
        }
        // A private RFC-1918 LAN endpoint is only WARNED on, never blocked.
        assert!(matches!(
            validate_base_url("http://192.168.1.50:1234/v1"),
            BaseUrlVerdict::AcceptWithWarning(_)
        ));
        // Cloud metadata IP and the wider link-local block -> blocked.
        assert!(matches!(
            validate_base_url("http://169.254.169.254/latest/meta-data/"),
            BaseUrlVerdict::Blocked(_)
        ));
        assert!(matches!(
            validate_base_url("http://169.254.1.1"),
            BaseUrlVerdict::Blocked(_)
        ));
        // IPv6 link-local fe80::/10 -> blocked.
        assert!(matches!(
            validate_base_url("http://[fe80::1]:1234"),
            BaseUrlVerdict::Blocked(_)
        ));
        // Malformed / hostless input -> conservatively blocked.
        assert!(matches!(
            validate_base_url("http:///v1"),
            BaseUrlVerdict::Blocked(_)
        ));
    }

    /// `check_provider_base_url` maps the verdict onto the command conventions:
    /// blocked -> `CommandError::invalid`; loopback/TLS -> `Ok(None)`; remote
    /// plaintext -> `Ok(Some(warning))`.
    #[test]
    fn check_provider_base_url_maps_verdict_to_command_result() {
        assert!(matches!(
            check_provider_base_url("http://localhost:1234/v1"),
            Ok(None)
        ));
        assert!(matches!(
            check_provider_base_url("https://api.example.com"),
            Ok(None)
        ));
        assert!(matches!(
            check_provider_base_url("http://api.example.com"),
            Ok(Some(_))
        ));
        match check_provider_base_url("http://169.254.169.254") {
            Err(err) => assert!(matches!(err.code, ErrorCode::InvalidArgument)),
            Ok(other) => panic!("expected blocked metadata IP, got {other:?}"),
        }
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

    // --- Phase 5 (FEAT-001) command tests ----------------------------------

    use orchestrator_core::{ConversationInit, MessageContent, MessageStatus, Role};

    /// Create a conversation directly via the session manager for command tests.
    async fn seed_conversation(state: &AppState) -> Uuid {
        state
            .session_manager
            .create_conversation(ConversationInit::default())
            .await
            .unwrap()
            .id
    }

    fn user_message(conversation_id: Uuid, text: &str) -> Message {
        Message {
            id: Uuid::new_v4(),
            conversation_id,
            role: Role::User,
            content: MessageContent::Text {
                text: text.to_string(),
            },
            created_at: chrono::Utc::now(),
            route: None,
            usage: None,
            status: MessageStatus::Complete,
        }
    }

    /// `get_messages` returns appended messages in order; an invalid id is
    /// rejected before touching the core.
    #[tokio::test]
    async fn get_messages_returns_appended_messages() {
        let state = test_state().await;
        let id = seed_conversation(&state).await;
        state
            .session_manager
            .append_message(user_message(id, "one"))
            .await
            .unwrap();
        state
            .session_manager
            .append_message(user_message(id, "two"))
            .await
            .unwrap();
        let messages = state.session_manager.list_messages(id).await.unwrap();
        assert_eq!(messages.len(), 2);

        // Invalid conversation id is rejected.
        assert!(parse_uuid("conversationId", "nope").is_err());
    }

    /// `set_conversation_route` persists a pin and clearing it (None) returns to
    /// automatic; a malformed pinned route is rejected.
    #[tokio::test]
    async fn set_conversation_route_pins_and_clears() {
        let state = test_state().await;
        let id = seed_conversation(&state).await;

        // Pin.
        let pinned = state
            .session_manager
            .set_conversation_route(
                id,
                Some(ManualRoute {
                    provider_id: "openai".to_string(),
                    model: "gpt-4o".to_string(),
                }),
            )
            .await
            .unwrap();
        assert_eq!(pinned.conversation_pref.unwrap().provider_id, "openai");

        // Clear -> automatic.
        let cleared = state
            .session_manager
            .set_conversation_route(id, None)
            .await
            .unwrap();
        assert!(cleared.conversation_pref.is_none());

        // A malformed pinned route is rejected by validation.
        let bad = ManualRoute {
            provider_id: "".to_string(),
            model: "m".to_string(),
        };
        assert!(validate_manual_route(&bad).is_err());
    }

    /// `set_conversation_routing_mode` persists a mode (PreferLocal) and clears
    /// it back to Auto/None through the extracted handler body; an invalid id is
    /// rejected before touching the core.
    #[tokio::test]
    async fn set_conversation_routing_mode_persists_and_clears() {
        let state = test_state().await;
        let id = seed_conversation(&state).await;

        // Set PreferLocal.
        let updated = set_conversation_routing_mode_inner(
            &state,
            &id.to_string(),
            Some(RoutingMode::PreferLocal),
        )
        .await
        .unwrap();
        assert_eq!(updated.routing_mode, Some(RoutingMode::PreferLocal));

        // Clear back to Auto/None.
        let cleared = set_conversation_routing_mode_inner(&state, &id.to_string(), None)
            .await
            .unwrap();
        assert!(cleared.routing_mode.is_none());

        // A malformed conversation id is rejected before touching the core.
        let err =
            set_conversation_routing_mode_inner(&state, "nope", Some(RoutingMode::PreferQuality))
                .await
                .unwrap_err();
        assert!(matches!(err.code, ErrorCode::InvalidArgument));
    }

    /// Policy-level guarantee surfaced through the routing crate: with a
    /// LocalOnly privacy tag, a PreferQuality conversation-level hint (what
    /// `RoutingMode::PreferQuality` maps to) can NEVER select a non-local model,
    /// and a Manual pin to a cloud model is still rejected by the hard-constraint
    /// gate. This mirrors the auto_default privacy test structure at the command
    /// crate boundary so a regression in the mode plumbing is caught here too.
    #[tokio::test]
    async fn routing_mode_and_manual_pin_cannot_override_local_only() {
        use providers::{Capabilities, ChatMessage, MessageRole, TokenPrice};
        use routing::{
            AutoDefaultPolicy, ManualOverrideResolver, ManualRoute as RoutingManualRoute,
            RoutingError, RoutingPolicy, RoutingRequest,
        };

        fn caps() -> Capabilities {
            Capabilities {
                streaming: true,
                tools: true,
                vision: false,
                json_mode: true,
                max_context: Some(8_000),
            }
        }
        let cloud = AvailableModel {
            provider_id: "openai".to_string(),
            model: "gpt-4o".to_string(),
            capabilities: caps(),
            price: TokenPrice::new(2.5, 10.0),
        };
        let local = AvailableModel {
            provider_id: "lmstudio".to_string(),
            model: "llama".to_string(),
            capabilities: caps(),
            price: TokenPrice::ZERO,
        };

        // Base request: LocalOnly tag, only the lmstudio row is provably local,
        // and a PreferQuality conversation-level hint (RoutingMode::PreferQuality
        // maps to this) is applied.
        let base = RoutingRequest {
            messages: vec![ChatMessage::text(MessageRole::User, "secret data")],
            privacy_tags: vec![PrivacyTag::LocalOnly],
            persona: None,
            routing_hint: Some(RoutingHint::PreferQuality),
            manual_override: None,
            conversation_pref: None,
            available: vec![cloud, local],
            local_provider_ids: ["lmstudio".to_string()].into_iter().collect(),
            budget: None,
        };

        // Automatic policy with the PreferQuality hint still fails closed to
        // local: it never selects the cloud model.
        let auto = AutoDefaultPolicy::new().decide(&base).await.unwrap();
        assert_eq!(auto.provider_id, "lmstudio");

        // A Manual pin to the cloud model under the same LocalOnly tag is
        // rejected by the hard-constraint gate rather than honored.
        let resolver = ManualOverrideResolver::new(Arc::new(AutoDefaultPolicy::new()));
        let mut pinned = base;
        pinned.conversation_pref = Some(RoutingManualRoute {
            provider_id: "openai".to_string(),
            model: "gpt-4o".to_string(),
        });
        let err = resolver.decide(&pinned).await.unwrap_err();
        assert!(matches!(err, RoutingError::ManualRouteRejected(_)));
    }

    /// `get_route_explanation` previews a pin as `ConversationPin`, and reports
    /// plain automatic routing when nothing is pinned and no message answered.
    #[tokio::test]
    async fn get_route_explanation_previews_precedence() {
        let state = test_state().await;
        let id = seed_conversation(&state).await;

        // No pin, no persona, no answered message -> automatic, no provider.
        let auto = get_route_explanation_inner(&state, &id.to_string())
            .await
            .unwrap();
        assert!(matches!(auto.source, RouteSource::Automatic));
        assert!(auto.provider_id.is_none());

        // Pin the conversation -> ConversationPin preview naming the model.
        state
            .session_manager
            .set_conversation_route(
                id,
                Some(ManualRoute {
                    provider_id: "lmstudio".to_string(),
                    model: "local".to_string(),
                }),
            )
            .await
            .unwrap();
        let pinned = get_route_explanation_inner(&state, &id.to_string())
            .await
            .unwrap();
        assert!(matches!(pinned.source, RouteSource::ConversationPin));
        assert_eq!(pinned.provider_id.as_deref(), Some("lmstudio"));
        assert_eq!(pinned.model.as_deref(), Some("local"));

        // Unknown conversation id -> NotFound.
        let unknown = Uuid::new_v4().to_string();
        let err = get_route_explanation_inner(&state, &unknown)
            .await
            .unwrap_err();
        assert!(matches!(err.code, ErrorCode::NotFound));
    }

    fn stdio_input(name: &str, command: &str) -> McpServerInput {
        McpServerInput {
            name: name.to_string(),
            transport: McpTransport::Stdio {
                command: command.to_string(),
                args: vec![],
                env: vec![],
            },
            permission_mode: PermissionMode::Ask,
            enabled: false,
        }
    }

    /// `add_mcp_server` validates the transport, assigns an id, and persists the
    /// row; an empty stdio command is rejected, and an httpSse with an empty url
    /// is rejected.
    #[tokio::test]
    async fn add_mcp_server_validates_and_persists() {
        let state = test_state().await;

        // Valid stdio server persists and is listed.
        let cfg = stdio_input("fs", "mcp-fs")
            .into_config(Uuid::new_v4())
            .unwrap();
        McpServerRepo::new(state.session_manager.db())
            .insert(&cfg)
            .await
            .unwrap();
        let listed = McpServerRepo::new(state.session_manager.db())
            .list()
            .await
            .unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "fs");

        // Empty stdio command -> InvalidArgument.
        assert!(stdio_input("bad", "  ")
            .into_config(Uuid::new_v4())
            .is_err());

        // httpSse with an empty url -> InvalidArgument.
        let bad_http = McpServerInput {
            name: "remote".to_string(),
            transport: McpTransport::HttpSse {
                url: "".to_string(),
                headers: vec![],
            },
            permission_mode: PermissionMode::Ask,
            enabled: false,
        };
        assert!(bad_http.into_config(Uuid::new_v4()).is_err());
    }

    /// `set_mcp_enabled` toggles the persisted `enabled` flag; `set_tool_permission`
    /// persists the per-server permission mode.
    #[tokio::test]
    async fn set_mcp_enabled_and_permission_persist() {
        let state = test_state().await;
        let repo = McpServerRepo::new(state.session_manager.db());
        let cfg = stdio_input("fs", "mcp-fs")
            .into_config(Uuid::new_v4())
            .unwrap();
        repo.insert(&cfg).await.unwrap();

        // Toggle enabled true, then false.
        let mut row = repo.get(cfg.id).await.unwrap().unwrap();
        row.enabled = true;
        repo.update(&row).await.unwrap();
        assert!(repo.get(cfg.id).await.unwrap().unwrap().enabled);
        row.enabled = false;
        repo.update(&row).await.unwrap();
        assert!(!repo.get(cfg.id).await.unwrap().unwrap().enabled);

        // Permission mode persists.
        row.permission_mode = PermissionMode::Deny;
        repo.update(&row).await.unwrap();
        assert_eq!(
            repo.get(cfg.id).await.unwrap().unwrap().permission_mode,
            PermissionMode::Deny
        );
    }

    /// `validate_transport` enforces stdio/httpSse required fields and bounds.
    #[test]
    fn validate_transport_enforces_required_fields() {
        assert!(validate_transport(&McpTransport::Stdio {
            command: "run".to_string(),
            args: vec!["--flag".to_string()],
            env: vec![("K".to_string(), "V".to_string())],
        })
        .is_ok());
        assert!(validate_transport(&McpTransport::Stdio {
            command: "".to_string(),
            args: vec![],
            env: vec![],
        })
        .is_err());
        assert!(validate_transport(&McpTransport::HttpSse {
            url: "https://example.com/sse".to_string(),
            headers: vec![],
        })
        .is_ok());
        assert!(validate_transport(&McpTransport::HttpSse {
            url: "".to_string(),
            headers: vec![],
        })
        .is_err());
    }

    /// `export_conversation` produces Markdown containing the title + message
    /// text, and JSON bundling the conversation + messages.
    #[tokio::test]
    async fn export_conversation_produces_expected_shape() {
        let state = test_state().await;
        let conv = state
            .session_manager
            .create_conversation(ConversationInit {
                title: Some("My Chat".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();
        state
            .session_manager
            .append_message(user_message(conv.id, "hello world"))
            .await
            .unwrap();

        let md = export_conversation_inner(&state, &conv.id.to_string(), ExportFormat::Markdown)
            .await
            .unwrap();
        assert!(md.contains("# My Chat"));
        assert!(md.contains("## User"));
        assert!(md.contains("hello world"));

        let json = export_conversation_inner(&state, &conv.id.to_string(), ExportFormat::Json)
            .await
            .unwrap();
        assert!(json.contains("\"conversation\""));
        assert!(json.contains("\"messages\""));
        assert!(json.contains("hello world"));

        // Unknown id -> NotFound.
        let unknown = Uuid::new_v4().to_string();
        let err = export_conversation_inner(&state, &unknown, ExportFormat::Json)
            .await
            .unwrap_err();
        assert!(matches!(err.code, ErrorCode::NotFound));
    }

    /// `stop_generation` is a validated no-op: it rejects a malformed id and an
    /// unknown conversation, and returns Ok for an existing conversation.
    #[tokio::test]
    async fn stop_generation_validates() {
        let state = test_state().await;
        let id = seed_conversation(&state).await;

        assert!(parse_uuid("conversationId", "nope").is_err());
        assert!(state
            .session_manager
            .get_conversation(id)
            .await
            .unwrap()
            .is_some());
    }

    /// `ExportFormat` deserializes from camelCase strings.
    #[test]
    fn export_format_deserializes_camel_case() {
        let md: ExportFormat = serde_json::from_str("\"markdown\"").unwrap();
        assert_eq!(md, ExportFormat::Markdown);
        let js: ExportFormat = serde_json::from_str("\"json\"").unwrap();
        assert_eq!(js, ExportFormat::Json);
    }

    /// The `RouteExplanation` shape is display-safe: it serializes to
    /// rationale/providerId/model/source only.
    #[test]
    fn route_explanation_is_display_safe() {
        let ex = RouteExplanation {
            rationale: "why".to_string(),
            provider_id: Some("openai".to_string()),
            model: Some("gpt-4o".to_string()),
            source: RouteSource::ConversationPin,
        };
        let json = serde_json::to_string(&ex).unwrap();
        assert!(json.contains("\"rationale\":\"why\""));
        assert!(json.contains("\"providerId\":\"openai\""));
        assert!(json.contains("\"source\":\"conversationPin\""));
        assert!(!json.to_lowercase().contains("secret"));
    }

    /// The `ToolDescriptorView` shape is display-safe camelCase.
    #[test]
    fn tool_descriptor_view_is_display_safe() {
        let view = ToolDescriptorView {
            name: "read_file".to_string(),
            description: "reads a file".to_string(),
            input_schema: json!({ "type": "object" }),
        };
        let out = serde_json::to_string(&view).unwrap();
        assert!(out.contains("\"name\":\"read_file\""));
        assert!(out.contains("\"inputSchema\""));
    }

    // --- Phase 6 (FEAT-001) secret-hygiene + validation audit tests ---------

    /// P6.1 IPC audit regression: after a secret is stored via the actual
    /// `set_provider_secret` handler body, the `list_available_models` handler
    /// body must return ONLY display-safe rows and never echo the planted
    /// plaintext. With no configured provider rows the model list is empty, but
    /// the invariant under test is that the model-enumeration return path never
    /// carries resolved key material: the secret lives in the keystore, is
    /// reachable only via the core-internal `resolve` seam, and never crosses
    /// this IPC boundary. A regression that folded resolved secrets into the
    /// returned rows would fail this test.
    #[tokio::test]
    async fn list_available_models_inner_never_returns_stored_secret() {
        let state = test_state().await;
        let plaintext = "sk-planted-secret-must-not-surface";

        // Store a secret through the real handler body.
        let secret_ref = set_provider_secret_inner(&state, "openai", plaintext).unwrap();
        assert_eq!(secret_ref, SecretRef::new("openai"));

        // The model list handler body returns display-safe rows with no secret.
        let result = list_available_models_inner(&state).await.unwrap();
        let json = serde_json::to_string(&result).unwrap();
        assert!(
            !json.contains(plaintext),
            "list_available_models must not surface stored secret material"
        );
        assert!(!json.to_lowercase().contains("secretref"));
        assert!(!json.to_lowercase().contains("apikey"));

        // The plaintext remains reachable ONLY via the core-internal resolve
        // seam, which is deliberately not wired to any command.
        assert_eq!(state.secret_store.resolve(&secret_ref).unwrap(), plaintext);
    }

    /// P6.1/P6.2 audit-style regression documenting the two structural secret
    /// invariants of the IPC surface, enforced by types rather than runtime
    /// checks:
    ///
    /// 1. NO command handler returns raw key material. Every `#[tauri::command]`
    ///    in this module returns one of: `()`, a bounded id/handle
    ///    (`SecretRef`), a domain row (`Conversation`/`AgentPersona`/`Message`/
    ///    `McpServerConfig`), a display-safe view (`AvailableModel`/
    ///    `RouteExplanation`/`ToolDescriptorView`/`OpenedConversation`), a
    ///    `String` that is an app version or an exported transcript, or a
    ///    `bool`. None is a resolved secret. `set_provider_secret` returns ONLY a
    ///    `SecretRef` (asserted here and in `set_provider_secret_inner_returns_only_ref`).
    /// 2. `SecretStore::resolve` (the single plaintext seam) is NOT reachable
    ///    from any command: it is called only inside `providers::build_registry`
    ///    when instantiating a provider, and that resolved value stays inside the
    ///    provider instance - it is never a command return value. The command
    ///    layer holds the store only to `store()` new secrets, never to surface
    ///    a resolved one.
    ///
    /// This test asserts the observable end of invariant (1): the handle
    /// `set_provider_secret` returns serializes to just the opaque provider id,
    /// so the return path cannot carry the plaintext.
    #[tokio::test]
    async fn no_command_return_path_exposes_plaintext() {
        let state = test_state().await;
        let plaintext = "sk-command-return-must-stay-opaque";
        let secret_ref = set_provider_secret_inner(&state, "anthropic", plaintext).unwrap();

        // The only secret-adjacent command return is a SecretRef, which
        // serializes to the bare handle - never the plaintext it references.
        let json = serde_json::to_string(&secret_ref).unwrap();
        assert_eq!(json, "\"anthropic\"");
        assert!(!json.contains(plaintext));

        // The plaintext is reachable ONLY through the core-internal resolve seam
        // (not a command), confirming the store->resolve path is the sole reader.
        assert_eq!(state.secret_store.resolve(&secret_ref).unwrap(), plaintext);
    }

    /// P6.2 per-command validation regression for a previously-untested
    /// rejection path: `create_conversation`'s argument validation
    /// (`validate_opt_len` title, `validate_privacy_tags`, and the persona-id
    /// UUID parse) rejects invalid input before any row is created. This
    /// exercises the exact validation the `#[tauri::command]` runs, driving the
    /// same helpers on the same arg struct.
    #[tokio::test]
    async fn create_conversation_validation_rejects_invalid_args() {
        // An over-long title is rejected before touching the core.
        let too_long_title = "x".repeat(MAX_TITLE_LEN + 1);
        assert!(validate_opt_len("title", &Some(too_long_title), MAX_TITLE_LEN).is_err());

        // Too many privacy tags are rejected.
        let too_many = vec![PrivacyTag::Custom("t".to_string()); MAX_TAGS + 1];
        assert!(validate_privacy_tags(&too_many).is_err());

        // An empty custom privacy tag is rejected.
        let empty_custom = vec![PrivacyTag::Custom("   ".to_string())];
        assert!(validate_privacy_tags(&empty_custom).is_err());

        // A malformed persona id is rejected before the conversation is created.
        assert!(parse_uuid("personaId", "not-a-uuid").is_err());

        // A well-formed set of args validates cleanly.
        let ok_tags = vec![PrivacyTag::Custom("work".to_string())];
        assert!(validate_opt_len("title", &Some("Chat".to_string()), MAX_TITLE_LEN).is_ok());
        assert!(validate_privacy_tags(&ok_tags).is_ok());
    }

    // --- Embedded local inference engine (Strategy B / FEAT-002) ------------

    /// `validate_gguf_path` rejects empty/over-long/non-`.gguf` paths and
    /// accepts a well-formed `.gguf` path (case-insensitive extension).
    #[test]
    fn validate_gguf_path_enforces_extension_and_bounds() {
        assert!(validate_gguf_path("   ").is_err());
        assert!(validate_gguf_path("/models/llama.bin").is_err());
        assert!(validate_gguf_path(&format!("{}.gguf", "x".repeat(MAX_MODEL_PATH_LEN))).is_err());
        assert!(validate_gguf_path("/models/phi-3-mini.gguf").is_ok());
        // Case-insensitive extension is accepted.
        assert!(validate_gguf_path("/models/Phi-3-Mini.GGUF").is_ok());
    }

    /// The embedded-model lifecycle helpers drive the engine registry + the
    /// loaded-model state end to end: import registers a model, select marks it
    /// loaded, status reports it, and unload clears it. This exercises the exact
    /// bodies the `#[tauri::command]` handlers call.
    #[tokio::test]
    async fn embedded_model_lifecycle_import_select_unload() {
        let state = test_state().await;

        // Nothing imported yet: status is empty and the list is empty.
        let status = embedded_model_status_inner(&state).await;
        assert_eq!(status.registered_count, 0);
        assert!(status.loaded_model_id.is_none());
        assert!(embedded_model_views(&state).await.is_empty());

        // Import a model: it is registered but not loaded.
        state
            .embedded_engine
            .register_path("/models/phi-3-mini.gguf")
            .await;
        let views = embedded_model_views(&state).await;
        assert_eq!(views.len(), 1);
        assert_eq!(views[0].id, "phi-3-mini");
        assert_eq!(views[0].path, "/models/phi-3-mini.gguf");
        assert!(!views[0].loaded);

        // Select (mark loaded): status reflects the loaded id.
        *state.embedded_loaded_model.write().await = Some("phi-3-mini".to_string());
        let status = embedded_model_status_inner(&state).await;
        assert_eq!(status.registered_count, 1);
        assert_eq!(status.loaded_model_id.as_deref(), Some("phi-3-mini"));
        let views = embedded_model_views(&state).await;
        assert!(views[0].loaded);

        // Unload clears the loaded state but keeps the registry.
        *state.embedded_loaded_model.write().await = None;
        let status = embedded_model_status_inner(&state).await;
        assert_eq!(status.registered_count, 1);
        assert!(status.loaded_model_id.is_none());
    }

    /// REGRESSION (review issue 1/4): an imported embedded model must surface
    /// through `list_available_models_inner` as a zero-priced [`AvailableModel`]
    /// so it groups under Local in the picker across routing modes. This drives
    /// the real enumerate pipeline end to end (seed the persisted Embedded
    /// config row + import onto the SHARED `AppState.embedded_engine`, then
    /// build the registry and enumerate), which is exactly what would have
    /// caught the split-instance bug: before the fix the enumerate path built a
    /// fresh, empty engine and returned zero embedded rows.
    #[tokio::test]
    async fn imported_embedded_model_surfaces_in_list_available_models() {
        let state = test_state().await;

        // Before any import there is no embedded provider row, so no embedded
        // model is enumerated.
        assert!(list_available_models_inner(&state)
            .await
            .unwrap()
            .models
            .iter()
            .all(|m| m.provider_id != EMBEDDED_PROVIDER_ID));

        // Import a local `.gguf` exactly as `import_embedded_model` does: seed
        // the persisted Embedded config row, then register the path on the
        // shared engine.
        ensure_embedded_provider_config(&state).await.unwrap();
        state
            .embedded_engine
            .register_path("/models/phi-3-mini.gguf")
            .await;

        // The imported model now appears as a zero-priced AvailableModel under
        // the embedded provider id, so the picker groups it under Local.
        let models = list_available_models_inner(&state).await.unwrap().models;
        let embedded: Vec<_> = models
            .iter()
            .filter(|m| m.provider_id == EMBEDDED_PROVIDER_ID)
            .collect();
        assert_eq!(
            embedded.len(),
            1,
            "the imported embedded model must surface"
        );
        assert_eq!(embedded[0].model, "phi-3-mini");
        assert_eq!(
            embedded[0].price,
            TokenPrice::ZERO,
            "embedded models are zero-priced so they group under Local"
        );
        // Streaming-text-only capabilities are carried through from the engine.
        assert!(embedded[0].capabilities.streaming);
        assert!(!embedded[0].capabilities.tools);

        // The row is also classified provably-local for the routing privacy
        // gate.
        let configs = ProviderRepo::new(state.session_manager.db())
            .list()
            .await
            .unwrap();
        assert!(local_provider_ids(&configs).contains(EMBEDDED_PROVIDER_ID));
    }

    /// Seeding the embedded provider-config row is idempotent: a second import
    /// does not insert a duplicate row (the persisted providers list keeps a
    /// single embedded entry).
    #[tokio::test]
    async fn ensure_embedded_provider_config_is_idempotent() {
        let state = test_state().await;
        ensure_embedded_provider_config(&state).await.unwrap();
        ensure_embedded_provider_config(&state).await.unwrap();
        let configs = ProviderRepo::new(state.session_manager.db())
            .list()
            .await
            .unwrap();
        let embedded_rows = configs
            .iter()
            .filter(|c| c.kind == ProviderKind::Embedded)
            .count();
        assert_eq!(embedded_rows, 1);
    }

    /// Seeding the Ollama provider-config row is idempotent: calling it twice
    /// (as repeated `list_available_models` enumerations do) inserts exactly one
    /// `OLLAMA_PROVIDER_ID` row of kind Ollama with no base_url (the adapter
    /// falls back to its `DEFAULT_BASE_URL`) and no api_key_ref (Ollama is
    /// keyless), and never errors or duplicates.
    #[tokio::test]
    async fn ensure_ollama_provider_config_is_idempotent() {
        let state = test_state().await;
        ensure_ollama_provider_config(&state).await.unwrap();
        ensure_ollama_provider_config(&state).await.unwrap();
        let configs = ProviderRepo::new(state.session_manager.db())
            .list()
            .await
            .unwrap();
        let ollama_rows: Vec<_> = configs
            .iter()
            .filter(|c| c.kind == ProviderKind::Ollama)
            .collect();
        assert_eq!(ollama_rows.len(), 1);
        assert_eq!(ollama_rows[0].id, OLLAMA_PROVIDER_ID);
        assert_eq!(
            ollama_rows[0].base_url, None,
            "seeded Ollama row carries no base_url so the adapter uses its DEFAULT_BASE_URL"
        );
        assert_eq!(
            ollama_rows[0].api_key_ref, None,
            "Ollama is keyless so the seeded row carries no api_key_ref"
        );
        // The seeded row is classified provably-local for the routing privacy
        // gate, so its discovered models group under Local in the picker.
        assert!(local_provider_ids(&configs).contains(OLLAMA_PROVIDER_ID));
    }

    /// `list_available_models_inner` auto-seeds the Ollama config row on first
    /// enumeration (zero user configuration), so an `OLLAMA_PROVIDER_ID` row of
    /// kind Ollama exists afterwards even though nothing was imported/saved. The
    /// seeded row contributes no models here because no Ollama server is running
    /// in the test (the enumerate path skips the unreachable provider), which is
    /// exactly the safe offline behavior.
    #[tokio::test]
    async fn list_available_models_auto_seeds_ollama_provider_config() {
        let state = test_state().await;
        // Enumerate as the picker does; this must not error even with no Ollama.
        let result = list_available_models_inner(&state).await.unwrap();
        // No Ollama server is running in the test, so the auto-seeded row cannot
        // be enumerated: its failure is surfaced as a display-safe enumeration
        // error (instead of being silently swallowed) so the UI can explain why
        // no local models appeared. The message never carries secret material.
        let ollama_err = result
            .errors
            .iter()
            .find(|e| e.provider_id == OLLAMA_PROVIDER_ID)
            .expect("the unreachable seeded Ollama row must surface an enumeration error");
        assert!(!ollama_err.message.is_empty());
        let err_json = serde_json::to_string(&result.errors).unwrap();
        assert!(err_json.contains("\"providerId\""));
        assert!(!err_json.to_lowercase().contains("apikey"));
        assert!(!err_json.to_lowercase().contains("secretref"));
        let configs = ProviderRepo::new(state.session_manager.db())
            .list()
            .await
            .unwrap();
        let ollama_rows = configs
            .iter()
            .filter(|c| c.kind == ProviderKind::Ollama)
            .count();
        assert_eq!(
            ollama_rows, 1,
            "the picker's enumerate path must auto-seed exactly one Ollama row"
        );
    }

    /// `provider_diagnostics_inner` reports the SAME reality the picker sees: with
    /// zero user configuration it auto-seeds the `ollama-local` row (kind Ollama),
    /// builds an instance for it (`instanceBuilt == true`), but because no live
    /// Ollama exists in the sandbox its `list_models` errors, so the row carries a
    /// non-empty display-safe `error` and `modelCount == 0`. This is exactly the
    /// clean-empty diagnosis the user needs (a configured, built, but unreachable
    /// provider) surfaced instead of an invisible skip.
    #[tokio::test]
    async fn provider_diagnostics_inner_reports_seeded_ollama_row() {
        let state = test_state().await;
        let report = provider_diagnostics_inner(&state).await.unwrap();

        // The auto-seeded Ollama row is present and configured.
        assert!(report.configured_count >= 1);
        let ollama = report
            .providers
            .iter()
            .find(|p| p.id == OLLAMA_PROVIDER_ID)
            .expect("diagnostics must include the auto-seeded ollama-local row");
        assert_eq!(ollama.kind, ProviderKind::Ollama);
        // The seeded row carries no base_url (the adapter uses its default).
        assert_eq!(ollama.base_url, None);
        // An instance is built for the keyless Ollama row (the build succeeds
        // even offline; only list_models hits the network).
        assert!(
            ollama.instance_built,
            "the keyless Ollama row must build an instance even when offline"
        );
        // No live Ollama in the sandbox, so enumeration errors and no models.
        assert_eq!(ollama.model_count, 0);
        let err = ollama
            .error
            .as_ref()
            .expect("the unreachable Ollama row must carry a display-safe error");
        assert!(!err.is_empty());

        // configuredCount reflects the seeded rows and matches the row count.
        assert_eq!(report.configured_count, report.providers.len());
        // With no reachable provider in the sandbox, nothing contributes models.
        assert_eq!(report.total_model_count, 0);
        assert_eq!(report.provider_count_with_models, 0);

        // DISPLAY-SAFE: the serialized report is camelCase and secret-free.
        let json = serde_json::to_string(&report).unwrap();
        assert!(json.contains("\"configuredCount\""));
        assert!(json.contains("\"totalModelCount\""));
        assert!(json.contains("\"providerCountWithModels\""));
        assert!(json.contains("\"instanceBuilt\""));
        assert!(json.contains("\"modelCount\""));
        assert!(json.contains("\"baseUrl\""));
        assert!(!json.to_lowercase().contains("apikey"));
        assert!(!json.to_lowercase().contains("secretref"));
    }

    /// A configured provider row whose instance cannot be built (an
    /// un-instantiable configuration) is reported as `instanceBuilt == false`
    /// with a non-empty display-safe error and zero models, rather than aborting
    /// the whole report. This pins the per-provider isolation of the resilient
    /// registry build and the visible self-report of the invisible-skip cause.
    #[tokio::test]
    async fn provider_diagnostics_inner_reports_unbuildable_row() {
        let state = test_state().await;

        // Persist a GenericOpenAI row with no base_url. The GenericOpenAI factory
        // requires an endpoint, so its build fails; the row is left un-built and
        // must surface as instanceBuilt == false with a display-safe error.
        let cfg = ProviderConfig {
            id: "broken-generic".to_string(),
            kind: ProviderKind::GenericOpenAI,
            base_url: None,
            api_key_ref: None,
            extra: serde_json::Value::Null,
        };
        ProviderRepo::new(state.session_manager.db())
            .insert(&cfg)
            .await
            .unwrap();

        let report = provider_diagnostics_inner(&state).await.unwrap();
        let broken = report
            .providers
            .iter()
            .find(|p| p.id == "broken-generic")
            .expect("the unbuildable row must appear in the diagnostics report");
        assert!(
            !broken.instance_built,
            "a row whose factory build failed must report instanceBuilt == false"
        );
        assert_eq!(broken.model_count, 0);
        let err = broken
            .error
            .as_ref()
            .expect("an unbuilt row must carry a display-safe reason");
        // FEAT-001: the row must now surface the REAL build error captured from
        // `build_from_config` (the GenericOpenAI factory rejects a missing
        // base_url with a message mentioning `base_url`), NOT the generic
        // "no instance was built" fallback that hid the true cause. This asserts
        // the discarded `Err(ProviderError)` is threaded through the shared
        // enumeration; reverting the fix (restoring `let _ =`) would fail here.
        assert!(
            err.contains("base_url"),
            "the unbuildable GenericOpenAI row must surface the REAL base_url \
             build error, got: {err}"
        );
        assert!(
            !err.contains("no instance was built"),
            "the real build error must replace the generic fallback, got: {err}"
        );

        // The report still includes the other rows (per-provider isolation).
        assert!(report.configured_count >= 2);

        // DISPLAY-SAFE: no secret material in the serialized report.
        let json = serde_json::to_string(&report).unwrap();
        assert!(!json.to_lowercase().contains("apikey"));
        assert!(!json.to_lowercase().contains("secretref"));
    }

    /// FEAT-001: a Gemini row whose `api_key_ref` points at a handle with NO
    /// stored secret cannot build (its factory resolves the secret and fails with
    /// a `ProviderError::Auth`). The diagnostics must report that row as
    /// `instanceBuilt == false` with the REAL Auth reason surfaced (the
    /// `SecretError::NotFound` message contains "no secret found"), NOT the
    /// generic "no instance was built" fallback. This proves an Auth build failure
    /// (the likely real cause the user hit, previously hidden by the discarded
    /// `Err`) is now diagnosable, and stays display-safe (no key material).
    #[tokio::test]
    async fn provider_diagnostics_inner_surfaces_gemini_auth_failure() {
        let state = test_state().await;

        // Persist a Gemini row whose api_key_ref points at a handle that has NO
        // stored secret, so `build_gemini`'s `secrets.resolve` fails with Auth.
        let cfg = ProviderConfig {
            id: "gemini-cloud".to_string(),
            kind: ProviderKind::Gemini,
            base_url: None,
            api_key_ref: Some(SecretRef::new("gemini-cloud")),
            extra: serde_json::Value::Null,
        };
        ProviderRepo::new(state.session_manager.db())
            .insert(&cfg)
            .await
            .unwrap();

        let report = provider_diagnostics_inner(&state).await.unwrap();
        let gemini = report
            .providers
            .iter()
            .find(|p| p.id == "gemini-cloud")
            .expect("the Gemini row must appear in the diagnostics report");
        assert!(
            !gemini.instance_built,
            "a Gemini row whose secret cannot be resolved must not build"
        );
        assert_eq!(gemini.model_count, 0);
        let err = gemini
            .error
            .as_ref()
            .expect("the Auth build failure must carry a display-safe reason");
        // The REAL Auth reason is surfaced (SecretError::NotFound Display), not
        // the generic fallback.
        assert!(
            err.contains("no secret found"),
            "the Gemini row must surface the real Auth reason, got: {err}"
        );
        assert!(
            !err.contains("no instance was built"),
            "the real Auth error must replace the generic fallback, got: {err}"
        );

        // DISPLAY-SAFE: no secret material in the serialized report.
        let json = serde_json::to_string(&report).unwrap();
        assert!(!json.to_lowercase().contains("apikey"));
        assert!(!json.to_lowercase().contains("secretref"));
    }

    /// FEAT-001: a Gemini row WITH a stored key builds an instance at the default
    /// base_url (base_url None -> the adapter's built-in default). The
    /// diagnostics must report `instanceBuilt == true`. Its `list_models` errors
    /// offline (no live Gemini endpoint in the sandbox), which is expected and
    /// fine: the point is that a plain Gemini key at default BUILDS. This pins the
    /// traced Gemini-by-key wiring (`set_cloud_provider` stores the key under the
    /// config id 'gemini-cloud'; `build_gemini` resolves that same handle).
    #[tokio::test]
    async fn provider_diagnostics_inner_builds_keyed_gemini_row() {
        let state = test_state().await;

        // Store a key under the 'gemini-cloud' handle, exactly as
        // `set_cloud_provider_inner` does, then persist a keyed Gemini row that
        // references it (base_url None => adapter default).
        let key_ref = state
            .secret_store
            .store("gemini-cloud", "AIza-test-key-do-not-leak")
            .unwrap();
        let cfg = ProviderConfig {
            id: "gemini-cloud".to_string(),
            kind: ProviderKind::Gemini,
            base_url: None,
            api_key_ref: Some(key_ref),
            extra: serde_json::Value::Null,
        };
        ProviderRepo::new(state.session_manager.db())
            .insert(&cfg)
            .await
            .unwrap();

        let report = provider_diagnostics_inner(&state).await.unwrap();
        let gemini = report
            .providers
            .iter()
            .find(|p| p.id == "gemini-cloud")
            .expect("the Gemini row must appear in the diagnostics report");
        assert!(
            gemini.instance_built,
            "a keyed Gemini row at default base_url MUST build an instance"
        );
        // list_models errors offline, so no models and (typically) a transport
        // error; the built distinction is what this test pins.
        assert_eq!(gemini.model_count, 0);

        // DISPLAY-SAFE: the stored key never crosses the report boundary.
        let json = serde_json::to_string(&report).unwrap();
        assert!(!json.contains("AIza-test-key-do-not-leak"));
        assert!(!json.to_lowercase().contains("apikey"));
        assert!(!json.to_lowercase().contains("secretref"));
    }

    /// The embedded-model DTOs are display-safe camelCase and carry no secret
    /// material (the embedded engine needs no key).
    #[test]
    fn embedded_model_dtos_are_display_safe_camel_case() {
        let view = EmbeddedModelView {
            id: "phi-3-mini".to_string(),
            path: "/models/phi-3-mini.gguf".to_string(),
            loaded: true,
        };
        let json = serde_json::to_string(&view).unwrap();
        assert!(json.contains("\"id\":\"phi-3-mini\""));
        assert!(json.contains("\"path\""));
        assert!(json.contains("\"loaded\":true"));

        let status = EmbeddedModelStatus {
            loaded_model_id: Some("phi-3-mini".to_string()),
            registered_count: 2,
        };
        let json = serde_json::to_string(&status).unwrap();
        assert!(json.contains("\"loadedModelId\":\"phi-3-mini\""));
        assert!(json.contains("\"registeredCount\":2"));
        assert!(!json.to_lowercase().contains("secret"));
        assert!(!json.to_lowercase().contains("apikey"));
    }

    /// `set_local_runtime_inner` persists an LM Studio row with the given
    /// base_url and NO api_key_ref when the key is None, and re-saving the same
    /// kind with a DIFFERENT base_url UPDATES the same row (the persisted list
    /// keeps exactly one row whose base_url reflects the latest save). This
    /// proves the stable-per-kind-id upsert is idempotent (no duplicates).
    #[tokio::test]
    async fn set_local_runtime_inner_upserts_by_stable_id() {
        let state = test_state().await;

        let view = set_local_runtime_inner(
            &state,
            ProviderKind::LmStudio,
            "http://localhost:1234/v1",
            None,
        )
        .await
        .unwrap();
        assert_eq!(view.id, "lmstudio-local");
        assert_eq!(view.base_url, "http://localhost:1234/v1");
        assert!(!view.has_api_key);
        assert!(view.warning.is_none());

        // Re-save the same kind with a different loopback base_url.
        set_local_runtime_inner(
            &state,
            ProviderKind::LmStudio,
            "http://127.0.0.1:5000",
            None,
        )
        .await
        .unwrap();

        let configs = ProviderRepo::new(state.session_manager.db())
            .list()
            .await
            .unwrap();
        let rows: Vec<_> = configs
            .iter()
            .filter(|c| c.kind == ProviderKind::LmStudio)
            .collect();
        assert_eq!(rows.len(), 1, "re-saving must update, not duplicate");
        assert_eq!(rows[0].id, "lmstudio-local");
        assert_eq!(rows[0].base_url.as_deref(), Some("http://127.0.0.1:5000"));
        assert!(rows[0].api_key_ref.is_none());
    }

    /// A Blocked base_url (a cloud-metadata / link-local target) is rejected
    /// with `InvalidArgument` and nothing is persisted.
    #[tokio::test]
    async fn set_local_runtime_inner_rejects_blocked_base_url() {
        let state = test_state().await;

        let err = set_local_runtime_inner(
            &state,
            ProviderKind::GenericOpenAI,
            "http://169.254.169.254/v1",
            None,
        )
        .await
        .unwrap_err();
        assert!(matches!(err.code, ErrorCode::InvalidArgument));

        // Nothing was persisted for the rejected call.
        let configs = ProviderRepo::new(state.session_manager.db())
            .list()
            .await
            .unwrap();
        assert!(configs.is_empty());
    }

    /// A non-loopback plaintext http base_url is accepted but surfaces a
    /// display-safe warning recommending TLS.
    #[tokio::test]
    async fn set_local_runtime_inner_surfaces_plaintext_warning() {
        let state = test_state().await;

        let view = set_local_runtime_inner(
            &state,
            ProviderKind::GenericOpenAI,
            "http://192.168.1.50:1234/v1",
            None,
        )
        .await
        .unwrap();
        assert!(
            view.warning.is_some(),
            "a non-loopback plaintext endpoint must carry a warning"
        );
    }

    /// Storing an API key sets `has_api_key` and persists an opaque
    /// `SecretRef`, but the plaintext key NEVER appears in the returned view
    /// (nor its serialized form).
    #[tokio::test]
    async fn set_local_runtime_inner_stores_key_without_echo() {
        let state = test_state().await;
        let plaintext = "sk-local-do-not-leak-9999";

        let view = set_local_runtime_inner(
            &state,
            ProviderKind::GenericOpenAI,
            "http://localhost:8080/v1",
            Some(plaintext),
        )
        .await
        .unwrap();
        assert!(view.has_api_key);

        // The view (and its serialized form) never carries the key.
        let json = serde_json::to_string(&view).unwrap();
        assert!(!json.contains(plaintext));

        // The row references the secret by an opaque handle only, and the
        // plaintext is reachable ONLY via the internal resolve seam.
        let configs = ProviderRepo::new(state.session_manager.db())
            .list()
            .await
            .unwrap();
        let row = configs
            .iter()
            .find(|c| c.id == "generic-openai-local")
            .unwrap();
        let secret_ref = row.api_key_ref.as_ref().unwrap();
        assert!(!secret_ref.handle().contains(plaintext));
        assert_eq!(state.secret_store.resolve(secret_ref).unwrap(), plaintext);
    }

    /// A non-local kind (e.g. OpenAI) is rejected with `InvalidArgument` and
    /// nothing is persisted; the same guard applies to `clear_local_runtime`.
    #[tokio::test]
    async fn set_local_runtime_inner_rejects_non_local_kind() {
        let state = test_state().await;

        let err =
            set_local_runtime_inner(&state, ProviderKind::OpenAI, "http://localhost:1234", None)
                .await
                .unwrap_err();
        assert!(matches!(err.code, ErrorCode::InvalidArgument));

        let err = clear_local_runtime_inner(&state, ProviderKind::OpenAI)
            .await
            .unwrap_err();
        assert!(matches!(err.code, ErrorCode::InvalidArgument));

        let configs = ProviderRepo::new(state.session_manager.db())
            .list()
            .await
            .unwrap();
        assert!(configs.is_empty());
    }

    /// A persisted GenericOpenAI row with a loopback base_url is classified
    /// local by `local_provider_ids`, while one pointed at a remote host is
    /// not. This mirrors `local_provider_ids_classifies_by_kind_and_endpoint`
    /// but drives the full persisted write path via `set_local_runtime_inner`.
    #[tokio::test]
    async fn persisted_generic_runtime_locality_by_endpoint() {
        // Loopback generic endpoint -> local.
        let state = test_state().await;
        set_local_runtime_inner(
            &state,
            ProviderKind::GenericOpenAI,
            "http://127.0.0.1:1234/v1",
            None,
        )
        .await
        .unwrap();
        let configs = ProviderRepo::new(state.session_manager.db())
            .list()
            .await
            .unwrap();
        assert!(local_provider_ids(&configs).contains("generic-openai-local"));

        // Remote generic endpoint (over TLS so it is accepted) -> not local.
        let state = test_state().await;
        set_local_runtime_inner(
            &state,
            ProviderKind::GenericOpenAI,
            "https://api.example.com/v1",
            None,
        )
        .await
        .unwrap();
        let configs = ProviderRepo::new(state.session_manager.db())
            .list()
            .await
            .unwrap();
        assert!(!local_provider_ids(&configs).contains("generic-openai-local"));
    }

    /// `list_local_runtimes_inner` returns the configured LM Studio row after a
    /// save, mapping it to a display-safe view (and `clear_local_runtime_inner`
    /// removes it again).
    #[tokio::test]
    async fn list_local_runtimes_inner_returns_configured_row() {
        let state = test_state().await;

        // Empty before any save.
        assert!(list_local_runtimes_inner(&state).await.unwrap().is_empty());

        set_local_runtime_inner(
            &state,
            ProviderKind::LmStudio,
            "http://localhost:1234/v1",
            None,
        )
        .await
        .unwrap();

        let runtimes = list_local_runtimes_inner(&state).await.unwrap();
        assert_eq!(runtimes.len(), 1);
        assert_eq!(runtimes[0].id, "lmstudio-local");
        assert_eq!(runtimes[0].kind, ProviderKind::LmStudio);
        assert_eq!(runtimes[0].base_url, "http://localhost:1234/v1");
        assert!(!runtimes[0].has_api_key);
        assert!(runtimes[0].warning.is_none());

        // Clearing removes the row.
        clear_local_runtime_inner(&state, ProviderKind::LmStudio)
            .await
            .unwrap();
        assert!(list_local_runtimes_inner(&state).await.unwrap().is_empty());
    }

    /// Clearing a runtime deletes the stored secret as well as the config row,
    /// so no credential material is orphaned in the keychain under the stable
    /// per-kind handle. After a keyed save the secret is resolvable; after the
    /// clear the same `SecretRef` must no longer resolve and the row is gone.
    #[tokio::test]
    async fn clear_local_runtime_inner_deletes_stored_secret() {
        let state = test_state().await;
        let plaintext = "sk-clear-me-do-not-leak";

        set_local_runtime_inner(
            &state,
            ProviderKind::GenericOpenAI,
            "http://localhost:8080/v1",
            Some(plaintext),
        )
        .await
        .unwrap();

        // Capture the SecretRef the save stored and confirm it resolves now.
        let old_ref = {
            let configs = ProviderRepo::new(state.session_manager.db())
                .list()
                .await
                .unwrap();
            let row = configs
                .iter()
                .find(|c| c.id == "generic-openai-local")
                .unwrap();
            row.api_key_ref.clone().unwrap()
        };
        assert_eq!(state.secret_store.resolve(&old_ref).unwrap(), plaintext);

        clear_local_runtime_inner(&state, ProviderKind::GenericOpenAI)
            .await
            .unwrap();

        // The config row is removed.
        assert!(list_local_runtimes_inner(&state).await.unwrap().is_empty());
        // The secret is no longer resolvable (deleted, not merely dereferenced).
        assert!(matches!(
            state.secret_store.resolve(&old_ref),
            Err(SecretError::NotFound(_))
        ));
    }

    /// Re-saving a runtime with NO key deletes the previously stored secret so
    /// nothing lingers under the stable per-kind handle: after a keyed save
    /// then a keyless re-save, the prior `SecretRef` no longer resolves and the
    /// row's `api_key_ref` is `None`.
    #[tokio::test]
    async fn keyless_resave_deletes_prior_secret() {
        let state = test_state().await;
        let plaintext = "sk-first-key-do-not-leak";

        set_local_runtime_inner(
            &state,
            ProviderKind::LmStudio,
            "http://localhost:1234/v1",
            Some(plaintext),
        )
        .await
        .unwrap();

        let old_ref = {
            let configs = ProviderRepo::new(state.session_manager.db())
                .list()
                .await
                .unwrap();
            let row = configs.iter().find(|c| c.id == "lmstudio-local").unwrap();
            row.api_key_ref.clone().unwrap()
        };
        assert_eq!(state.secret_store.resolve(&old_ref).unwrap(), plaintext);

        // Re-save the same kind without a key.
        let view = set_local_runtime_inner(
            &state,
            ProviderKind::LmStudio,
            "http://localhost:1234/v1",
            None,
        )
        .await
        .unwrap();
        assert!(!view.has_api_key);

        // The row no longer references a secret.
        let configs = ProviderRepo::new(state.session_manager.db())
            .list()
            .await
            .unwrap();
        let row = configs.iter().find(|c| c.id == "lmstudio-local").unwrap();
        assert!(row.api_key_ref.is_none());

        // The prior secret is no longer resolvable in the store.
        assert!(matches!(
            state.secret_store.resolve(&old_ref),
            Err(SecretError::NotFound(_))
        ));
    }

    /// `set_cloud_provider_inner` persists a ProviderConfig row of the given
    /// cloud kind by its stable per-kind id with an `api_key_ref`, returns
    /// `has_api_key = true`, and NEVER echoes the plaintext key (in the view or
    /// its serialized form). Re-saving the same kind UPDATES the one row rather
    /// than duplicating it (the stable-per-kind-id upsert is idempotent).
    #[tokio::test]
    async fn set_cloud_provider_inner_persists_row_without_echo() {
        let state = test_state().await;
        let plaintext = "sk-cloud-do-not-leak-1234";

        let view = set_cloud_provider_inner(&state, ProviderKind::Gemini, plaintext, None)
            .await
            .unwrap();
        assert_eq!(view.id, "gemini-cloud");
        assert_eq!(view.kind, ProviderKind::Gemini);
        assert_eq!(view.base_url, None);
        assert!(view.has_api_key);

        // Neither the view nor its serialized form carries the key.
        let json = serde_json::to_string(&view).unwrap();
        assert!(!json.contains(plaintext));
        assert!(!json.to_lowercase().contains("secret"));

        // The persisted row is a real Gemini ProviderConfig referencing the
        // secret only by an opaque handle; the plaintext is reachable ONLY via
        // the internal resolve seam.
        let configs = ProviderRepo::new(state.session_manager.db())
            .list()
            .await
            .unwrap();
        let row = configs.iter().find(|c| c.id == "gemini-cloud").unwrap();
        assert_eq!(row.kind, ProviderKind::Gemini);
        let secret_ref = row.api_key_ref.as_ref().unwrap();
        assert!(!secret_ref.handle().contains(plaintext));
        assert_eq!(state.secret_store.resolve(secret_ref).unwrap(), plaintext);

        // Re-saving the same kind updates the one row (no duplicate).
        set_cloud_provider_inner(&state, ProviderKind::Gemini, "sk-cloud-rotated", None)
            .await
            .unwrap();
        let configs = ProviderRepo::new(state.session_manager.db())
            .list()
            .await
            .unwrap();
        let rows: Vec<_> = configs
            .iter()
            .filter(|c| c.kind == ProviderKind::Gemini)
            .collect();
        assert_eq!(rows.len(), 1, "re-saving must update, not duplicate");
    }

    /// A non-cloud kind (e.g. Ollama) is rejected with `InvalidArgument` by both
    /// the set and clear inner paths and nothing is persisted.
    #[tokio::test]
    async fn set_cloud_provider_inner_rejects_non_cloud_kind() {
        let state = test_state().await;

        let err = set_cloud_provider_inner(&state, ProviderKind::Ollama, "sk-x", None)
            .await
            .unwrap_err();
        assert!(matches!(err.code, ErrorCode::InvalidArgument));

        let err = clear_cloud_provider_inner(&state, ProviderKind::Ollama)
            .await
            .unwrap_err();
        assert!(matches!(err.code, ErrorCode::InvalidArgument));

        let configs = ProviderRepo::new(state.session_manager.db())
            .list()
            .await
            .unwrap();
        assert!(configs.is_empty());
    }

    /// An empty api_key is rejected with `InvalidArgument` (cloud providers
    /// always need a key) and nothing is persisted.
    #[tokio::test]
    async fn set_cloud_provider_inner_rejects_empty_api_key() {
        let state = test_state().await;

        let err = set_cloud_provider_inner(&state, ProviderKind::OpenAI, "", None)
            .await
            .unwrap_err();
        assert!(matches!(err.code, ErrorCode::InvalidArgument));

        let configs = ProviderRepo::new(state.session_manager.db())
            .list()
            .await
            .unwrap();
        assert!(configs.is_empty());
    }

    /// Kiro maps to GenericOpenAI, which has no default endpoint, so a base_url
    /// is REQUIRED: omitting it rejects with `InvalidArgument` (and persists
    /// nothing), while a valid base_url is accepted, validated, and persisted on
    /// the row. A Blocked base_url is rejected by the Section 9.3 posture.
    #[tokio::test]
    async fn set_cloud_provider_inner_requires_and_validates_kiro_base_url() {
        let state = test_state().await;

        // Missing base_url for GenericOpenAI (Kiro) is rejected.
        let err = set_cloud_provider_inner(&state, ProviderKind::GenericOpenAI, "sk-kiro", None)
            .await
            .unwrap_err();
        assert!(matches!(err.code, ErrorCode::InvalidArgument));
        assert!(ProviderRepo::new(state.session_manager.db())
            .list()
            .await
            .unwrap()
            .is_empty());

        // A Blocked base_url is rejected by the base-url posture.
        let err = set_cloud_provider_inner(
            &state,
            ProviderKind::GenericOpenAI,
            "sk-kiro",
            Some("http://169.254.169.254/v1"),
        )
        .await
        .unwrap_err();
        assert!(matches!(err.code, ErrorCode::InvalidArgument));

        // A valid (TLS) base_url is accepted and persisted on the row.
        let view = set_cloud_provider_inner(
            &state,
            ProviderKind::GenericOpenAI,
            "sk-kiro",
            Some("https://kiro.example.com/v1"),
        )
        .await
        .unwrap();
        assert_eq!(view.id, "generic-openai-cloud");
        assert_eq!(
            view.base_url.as_deref(),
            Some("https://kiro.example.com/v1")
        );
        assert!(view.has_api_key);
        // A TLS endpoint carries no advisory.
        assert!(view.warning.is_none());

        let configs = ProviderRepo::new(state.session_manager.db())
            .list()
            .await
            .unwrap();
        let row = configs
            .iter()
            .find(|c| c.id == "generic-openai-cloud")
            .unwrap();
        assert_eq!(row.kind, ProviderKind::GenericOpenAI);
        assert_eq!(row.base_url.as_deref(), Some("https://kiro.example.com/v1"));
    }

    /// A cloud provider (Kiro / GenericOpenAI) configured with a non-loopback
    /// plaintext http base_url is accepted but carries a display-safe advisory
    /// recommending TLS (parity with `set_local_runtime`); a loopback/TLS
    /// base_url carries none. The advisory NEVER echoes the key material.
    #[tokio::test]
    async fn set_cloud_provider_inner_surfaces_plaintext_warning() {
        let state = test_state().await;
        let plaintext = "sk-kiro-do-not-leak-1234";

        // Non-loopback plaintext http endpoint: accepted with a warning.
        let view = set_cloud_provider_inner(
            &state,
            ProviderKind::GenericOpenAI,
            plaintext,
            Some("http://example.com/v1"),
        )
        .await
        .unwrap();
        assert!(
            view.warning.is_some(),
            "a non-loopback plaintext endpoint must carry a warning"
        );
        // The advisory (and the whole serialized view) never carries the key.
        let warning = view.warning.as_deref().unwrap();
        assert!(!warning.contains(plaintext));
        let json = serde_json::to_string(&view).unwrap();
        assert!(!json.contains(plaintext));

        // A loopback base_url carries no advisory.
        let loopback = set_cloud_provider_inner(
            &state,
            ProviderKind::GenericOpenAI,
            plaintext,
            Some("http://127.0.0.1:1234/v1"),
        )
        .await
        .unwrap();
        assert!(loopback.warning.is_none());

        // A TLS base_url carries no advisory.
        let tls = set_cloud_provider_inner(
            &state,
            ProviderKind::GenericOpenAI,
            plaintext,
            Some("https://example.com/v1"),
        )
        .await
        .unwrap();
        assert!(tls.warning.is_none());
    }

    /// `list_cloud_providers_inner` returns the configured cloud rows after a
    /// save (display-safe views with `has_api_key = true`), and does not leak
    /// any key material in the serialized list.
    #[tokio::test]
    async fn list_cloud_providers_inner_returns_configured_rows() {
        let state = test_state().await;

        // Empty before any save.
        assert!(list_cloud_providers_inner(&state).await.unwrap().is_empty());

        set_cloud_provider_inner(&state, ProviderKind::Anthropic, "sk-anthropic", None)
            .await
            .unwrap();

        let providers = list_cloud_providers_inner(&state).await.unwrap();
        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].id, "anthropic-cloud");
        assert_eq!(providers[0].kind, ProviderKind::Anthropic);
        assert!(providers[0].has_api_key);

        let json = serde_json::to_string(&providers).unwrap();
        assert!(!json.contains("sk-anthropic"));
    }

    /// `clear_cloud_provider_inner` deletes both the config row and the stored
    /// secret so no credential material is orphaned in the keychain under the
    /// stable per-kind handle.
    #[tokio::test]
    async fn clear_cloud_provider_inner_deletes_row_and_secret() {
        let state = test_state().await;
        let plaintext = "sk-clear-cloud-do-not-leak";

        set_cloud_provider_inner(&state, ProviderKind::OpenAI, plaintext, None)
            .await
            .unwrap();

        // Capture the SecretRef the save stored and confirm it resolves now.
        let old_ref = {
            let configs = ProviderRepo::new(state.session_manager.db())
                .list()
                .await
                .unwrap();
            let row = configs.iter().find(|c| c.id == "openai-cloud").unwrap();
            row.api_key_ref.clone().unwrap()
        };
        assert_eq!(state.secret_store.resolve(&old_ref).unwrap(), plaintext);

        clear_cloud_provider_inner(&state, ProviderKind::OpenAI)
            .await
            .unwrap();

        // The config row is removed and the secret is no longer resolvable.
        assert!(list_cloud_providers_inner(&state).await.unwrap().is_empty());
        assert!(matches!(
            state.secret_store.resolve(&old_ref),
            Err(SecretError::NotFound(_))
        ));
    }

    // --- ProvidersChanged emit (bug B: the model selector never refetched) ---
    //
    // The provider-config mutation commands emit `CoreEvent::ProvidersChanged`
    // so the frontend providers store refetches `list_available_models`. The
    // emit lives in a `*_and_notify(&AppState, ...)` seam that runs the mutation
    // then emits; each thin `#[tauri::command]` wrapper is a one-line delegation
    // to that seam (the wrappers hold a live `tauri::State`, which cannot be
    // built offline). These tests drive the SEAM directly - NOT
    // `emit_providers_changed` in the test body - so a dropped emit inside a
    // seam (equivalently, a wrapper) is a test failure. Each positive test
    // asserts the seam emits EXACTLY ONE `ProvidersChanged` (event queued, then
    // the receiver is empty); the rejected-mutation test proves the emit is
    // skipped on the `?` short-circuit.

    /// A state whose core-event receiver half is RETAINED (the shared
    /// `test_state()` drops it), so a test can drain emitted [`CoreEvent`]s.
    async fn test_state_with_rx() -> (AppState, UnboundedReceiver<CoreEvent>) {
        let db = Db::open_in_memory().await.unwrap();
        AppState::new(
            SessionManager::new(db),
            Arc::new(InMemorySecretStore::new()),
        )
    }

    /// Drain one queued `CoreEvent`, assert it is `ProvidersChanged`, and assert
    /// it was the ONLY one queued. Draining exactly one pins the seam to a single
    /// emit: a dropped emit fails the first `try_recv`, and a duplicated emit
    /// fails the emptiness check.
    fn assert_one_providers_changed(rx: &mut UnboundedReceiver<CoreEvent>) {
        match rx.try_recv() {
            Ok(CoreEvent::ProvidersChanged) => {}
            other => panic!("expected CoreEvent::ProvidersChanged, got {other:?}"),
        }
        assert!(
            rx.try_recv().is_err(),
            "a mutation seam must emit ProvidersChanged exactly once"
        );
    }

    /// Saving a cloud provider emits `ProvidersChanged` (via the seam the
    /// `set_cloud_provider` wrapper delegates to).
    #[tokio::test]
    async fn set_cloud_provider_emits_providers_changed() {
        let (state, mut rx) = test_state_with_rx().await;
        set_cloud_provider_and_notify(&state, ProviderKind::Gemini, "sk-gemini-key", None)
            .await
            .unwrap();
        assert_one_providers_changed(&mut rx);
    }

    /// Clearing a cloud provider emits `ProvidersChanged` (via the seam the
    /// `clear_cloud_provider` wrapper delegates to).
    #[tokio::test]
    async fn clear_cloud_provider_emits_providers_changed() {
        let (state, mut rx) = test_state_with_rx().await;
        set_cloud_provider_and_notify(&state, ProviderKind::OpenAI, "sk-openai-key", None)
            .await
            .unwrap();
        assert_one_providers_changed(&mut rx);

        clear_cloud_provider_and_notify(&state, ProviderKind::OpenAI)
            .await
            .unwrap();
        assert_one_providers_changed(&mut rx);
    }

    /// Saving a local runtime emits `ProvidersChanged` (via the seam the
    /// `set_local_runtime` wrapper delegates to).
    #[tokio::test]
    async fn set_local_runtime_emits_providers_changed() {
        let (state, mut rx) = test_state_with_rx().await;
        set_local_runtime_and_notify(
            &state,
            ProviderKind::LmStudio,
            "http://localhost:1234/v1",
            None,
        )
        .await
        .unwrap();
        assert_one_providers_changed(&mut rx);
    }

    /// Clearing a local runtime emits `ProvidersChanged` (via the seam the
    /// `clear_local_runtime` wrapper delegates to).
    #[tokio::test]
    async fn clear_local_runtime_emits_providers_changed() {
        let (state, mut rx) = test_state_with_rx().await;
        set_local_runtime_and_notify(
            &state,
            ProviderKind::LmStudio,
            "http://localhost:1234/v1",
            None,
        )
        .await
        .unwrap();
        assert_one_providers_changed(&mut rx);

        clear_local_runtime_and_notify(&state, ProviderKind::LmStudio)
            .await
            .unwrap();
        assert_one_providers_changed(&mut rx);
    }

    /// A failed (rejected) mutation must NOT emit `ProvidersChanged`: the seam
    /// only emits after the inner body returns `Ok`, so a validation error (here
    /// a non-cloud kind) short-circuits at the `?` before the emit and leaves the
    /// receiver empty. Driving the seam (not `_inner`) proves the skip happens on
    /// the exact path the wrapper takes.
    #[tokio::test]
    async fn rejected_mutation_does_not_emit_providers_changed() {
        let (state, mut rx) = test_state_with_rx().await;
        // Ollama is a local kind, not configurable via the cloud path: the inner
        // body returns Err, so the seam's `?` short-circuits before the emit.
        let err = set_cloud_provider_and_notify(&state, ProviderKind::Ollama, "sk-x", None)
            .await
            .unwrap_err();
        assert!(matches!(err.code, ErrorCode::InvalidArgument));
        assert!(
            rx.try_recv().is_err(),
            "a rejected mutation must not emit ProvidersChanged"
        );
    }

    /// The embedded-model lifecycle seams emit `ProvidersChanged`: importing,
    /// selecting, loading, and unloading each change what enumerates under Local.
    /// Driving each seam (the wrapper delegates to it) asserts exactly one emit
    /// per mutation, so a dropped emit in any of the four is a test failure.
    #[tokio::test]
    async fn embedded_model_mutations_emit_providers_changed() {
        let (state, mut rx) = test_state_with_rx().await;

        // import
        import_embedded_model_and_notify(&state, "/tmp/model-a.gguf")
            .await
            .unwrap();
        assert_one_providers_changed(&mut rx);

        // select (registers and marks active, returning its id for load)
        select_embedded_model_and_notify(&state, "/tmp/model-b.gguf")
            .await
            .unwrap();
        assert_one_providers_changed(&mut rx);
        let id = state
            .embedded_loaded_model
            .read()
            .await
            .clone()
            .expect("select marks a model loaded");

        // load
        load_embedded_model_and_notify(&state, id).await.unwrap();
        assert_one_providers_changed(&mut rx);

        // unload
        unload_embedded_model_and_notify(&state).await.unwrap();
        assert_one_providers_changed(&mut rx);
    }

    /// A rejected embedded-model seam must NOT emit `ProvidersChanged`: an
    /// invalid `.gguf` path short-circuits at the `?` before the emit, so the
    /// receiver stays empty. This pins the emit-only-after-Ok contract for the
    /// embedded path too.
    #[tokio::test]
    async fn rejected_embedded_mutation_does_not_emit_providers_changed() {
        let (state, mut rx) = test_state_with_rx().await;
        let err = import_embedded_model_and_notify(&state, "/tmp/not-a-model.txt")
            .await
            .unwrap_err();
        assert!(matches!(err.code, ErrorCode::InvalidArgument));
        assert!(
            rx.try_recv().is_err(),
            "a rejected embedded mutation must not emit ProvidersChanged"
        );
    }
}
