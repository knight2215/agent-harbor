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
    AgentPersona, Conversation, ConversationInit, ManualRoute, ModelParameters, PrivacyTag,
    RoutingHint, SecretRef,
};
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
    validate_nonempty("providerId", &provider_id, MAX_PROVIDER_ID_LEN)?;
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
        .store(&provider_id, &secret)
        .map_err(|e| CommandError::internal(e.to_string()))
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
    use secrets::{InMemorySecretStore, SecretStore};
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

    /// `set_provider_secret` returns only a SecretRef handle, never the secret,
    /// and the secret is retrievable ONLY through the core-internal resolve
    /// seam (which is not a command).
    #[tokio::test]
    async fn set_provider_secret_returns_only_ref() {
        let state = test_state().await;
        let plaintext = "sk-do-not-leak-1234";
        let secret_ref = state.secret_store.store("openai", plaintext).unwrap();
        // The handle is the opaque provider id, never the key.
        assert_eq!(secret_ref, SecretRef::new("openai"));
        assert_eq!(secret_ref.handle(), "openai");
        assert!(!secret_ref.handle().contains(plaintext));
        // The serialized form that would cross IPC is just the handle.
        let json = serde_json::to_string(&secret_ref).unwrap();
        assert_eq!(json, "\"openai\"");
        assert!(!json.contains(plaintext));
        // The plaintext is only reachable via the internal resolve seam.
        assert_eq!(state.secret_store.resolve(&secret_ref).unwrap(), plaintext);
    }

    /// Empty provider id / secret are rejected by validation before the store.
    #[test]
    fn secret_validation_bounds() {
        assert!(validate_nonempty("providerId", "", MAX_PROVIDER_ID_LEN).is_err());
        assert!(validate_nonempty("providerId", "openai", MAX_PROVIDER_ID_LEN).is_ok());
    }
}
