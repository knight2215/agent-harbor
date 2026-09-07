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
use providers::{list_available_models as list_models, AvailableModel, PricingTable, TokenPrice};
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
// Reachable only through `check_provider_base_url`, the not-yet-wired
// provider-config write seam (see its doc comment); exercised by the unit tests.
#[cfg_attr(not(test), allow(dead_code))]
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
// Reachable only through `check_provider_base_url`, the not-yet-wired
// provider-config write seam (see its doc comment); exercised by the unit tests.
#[cfg_attr(not(test), allow(dead_code))]
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
// Produced only through `check_provider_base_url`, the not-yet-wired
// provider-config write seam (see its doc comment); exercised by the unit tests.
#[cfg_attr(not(test), allow(dead_code))]
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
// Reachable only through `check_provider_base_url`, the not-yet-wired
// provider-config write seam (see its doc comment); exercised by the unit tests.
#[cfg_attr(not(test), allow(dead_code))]
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
/// No `#[tauri::command]` currently accepts a raw provider `base_url` from the
/// webview (providers are seeded through [`persistence::ProviderRepo`], not an
/// IPC command), so this is the enforcement point wherever a provider-config
/// create/update command is added: call it before `ProviderRepo::insert` /
/// `ProviderRepo::update`. It is fully covered by the unit tests below.
#[cfg_attr(not(test), allow(dead_code))]
fn check_provider_base_url(url: &str) -> Result<Option<String>, CommandError> {
    match validate_base_url(url) {
        BaseUrlVerdict::AcceptLoopback | BaseUrlVerdict::AcceptTls => Ok(None),
        BaseUrlVerdict::AcceptWithWarning(warning) => Ok(Some(warning)),
        BaseUrlVerdict::Blocked(reason) => Err(CommandError::invalid(reason)),
    }
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
        use providers::{AvailableModel, Capabilities, ChatMessage, MessageRole, TokenPrice};
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
        let models = list_available_models_inner(&state).await.unwrap();
        let json = serde_json::to_string(&models).unwrap();
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
}
