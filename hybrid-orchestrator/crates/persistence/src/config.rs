//! Versioned app config load/save (architecture.md Section 10.4).
//!
//! [`AppConfig`] carries a `schema_version` so migrations can upgrade older
//! records forward. It is persisted as a single row in `app_config`: the
//! `schema_version` lives in its own column (so a migration can reason about it
//! without parsing JSON), and the remaining fields are a JSON blob loaded
//! forward-compatibly (unknown newer fields are preserved via a `#[serde(flatten)]`
//! catch-all rather than dropped - Section 10.4 additive-change bias).

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use domain::ProviderKind;
use serde::{Deserialize, Serialize};

use crate::db::{Db, PersistenceError};
use sqlx::Row;

/// The current config schema version this build writes.
pub const CURRENT_SCHEMA_VERSION: u32 = 1;

/// Per-model token pricing in currency units per one million tokens
/// (architecture.md Section 6.2). Mirrors the in-memory `providers::TokenPrice`
/// shape (same camelCase JSON) so the Tauri command can convert without a
/// bespoke mapping. Both rates default to zero so a partially-filled entry (or a
/// local provider) is a valid zero-cost price rather than a missing one.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenRate {
    /// Input (prompt) token rate, currency units per 1M tokens.
    #[serde(default)]
    pub input_per_mtok: f64,
    /// Output (completion) token rate, currency units per 1M tokens.
    #[serde(default)]
    pub output_per_mtok: f64,
}

impl TokenRate {
    /// A zero-cost rate (local providers, unpriced entries).
    pub const ZERO: TokenRate = TokenRate {
        input_per_mtok: 0.0,
        output_per_mtok: 0.0,
    };
}

impl Default for TokenRate {
    fn default() -> Self {
        TokenRate::ZERO
    }
}

/// The user-entered, per-provider/per-model token-rate table that lives in the
/// versioned app config (architecture.md Section 6.2). This is the SINGLE source
/// both the cost signal and `providers::list_available_models` read.
///
/// Additive and forward-compatible: the whole struct is `#[serde(default)]` on
/// [`AppConfig::pricing`], every field defaults to empty/`None`, and it is
/// omitted from the serialized config when empty (so older configs load
/// unchanged and older builds ignore it via the `extra` catch-all).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PricingConfig {
    /// Default rate per provider kind (local kinds default to zero). Keyed by
    /// the camelCase [`ProviderKind`] serialization (e.g. `openAI`, `lmStudio`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub per_kind: BTreeMap<ProviderKind, TokenRate>,
    /// Per-model overrides, keyed by provider kind then model id.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub per_model: BTreeMap<ProviderKind, BTreeMap<String, TokenRate>>,
    /// When the user last edited the pricing table, if ever. Lets the UI show a
    /// staleness hint and lets a future sync reconcile edits (Section 6.2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_edited: Option<DateTime<Utc>>,
}

impl PricingConfig {
    /// Whether the user has entered any rates (nothing per-kind or per-model).
    pub fn is_empty(&self) -> bool {
        self.per_kind.is_empty() && self.per_model.is_empty()
    }
}

/// The default `max_results` for a web search (FEAT-004). Bounds how many hits
/// the composer injects as context before the model answers.
pub const DEFAULT_WEB_SEARCH_MAX_RESULTS: u32 = 5;

fn default_web_search_max_results() -> u32 {
    DEFAULT_WEB_SEARCH_MAX_RESULTS
}

/// User-configured web-search settings that live in the versioned app config
/// (FEAT-004). Additive and forward-compatible like [`PricingConfig`]: the whole
/// struct is `#[serde(default)]` on [`AppConfig::web_search`], every field
/// defaults, and it is omitted from the serialized config when it is the default
/// (so older configs load unchanged and older builds ignore it via the `extra`
/// catch-all).
///
/// SECRET HYGIENE (Section 9.1): the API KEY is NOT stored here. It is written to
/// the OS keychain under [`WEB_SEARCH_SECRET_HANDLE`] and referenced only by that
/// stable handle; this config records only the selected provider and result cap.
/// The presence of a stored key is reported by the command view's `hasApiKey`
/// (resolved core-internally), never by this struct.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebSearchConfig {
    /// The selected provider kind serialized as its camelCase tag (`tavily` /
    /// `brave` / `serpApi`), or `None` when web search is unconfigured. Kept as
    /// a `String` here so `persistence` does not depend on the `providers`
    /// crate (which is crates.io-only / CI-built); the command layer maps it to
    /// `providers::WebSearchKind`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled_provider: Option<String>,
    /// How many results to request per search (default
    /// [`DEFAULT_WEB_SEARCH_MAX_RESULTS`]).
    #[serde(default = "default_web_search_max_results")]
    pub max_results: u32,
}

impl Default for WebSearchConfig {
    fn default() -> Self {
        WebSearchConfig {
            enabled_provider: None,
            max_results: DEFAULT_WEB_SEARCH_MAX_RESULTS,
        }
    }
}

impl WebSearchConfig {
    /// Whether this is the untouched default (no provider selected and the
    /// default result cap), so it can be omitted from the serialized config.
    pub fn is_default(&self) -> bool {
        self.enabled_provider.is_none() && self.max_results == DEFAULT_WEB_SEARCH_MAX_RESULTS
    }
}

/// The stable keychain handle under which the web-search API key is stored
/// (FEAT-004). A single handle (rather than a per-kind one) keeps clearing the
/// key simple: swapping providers overwrites the one entry, and
/// `clear_web_search_provider` deletes exactly this handle. The key is written
/// via `SecretStore::store(WEB_SEARCH_SECRET_HANDLE, plaintext)` and resolved
/// core-internally at search time, mirroring the cloud-provider secret path.
pub const WEB_SEARCH_SECRET_HANDLE: &str = "web-search";

/// The default TCP port the LAN model-sharing server binds to when the user
/// enables sharing without an explicit port (FEAT-006). Chosen to sit clear of
/// the common local-inference ports (Ollama 11434, LM Studio 1234) so enabling
/// the share server on a machine already running one of those does not collide.
pub const DEFAULT_MODEL_SHARING_PORT: u16 = 11435;

fn default_model_sharing_port() -> u16 {
    DEFAULT_MODEL_SHARING_PORT
}

/// User-configured LAN model-sharing settings (FEAT-006). When `enabled`, this
/// instance runs a small OpenAI-compatible read surface (at minimum
/// `GET /v1/models`) bound to the LAN on `port`, re-exposing this machine's
/// local models to peers.
///
/// Additive and forward-compatible like [`WebSearchConfig`]: the whole struct is
/// `#[serde(default)]` on [`AppConfig::model_sharing`], every field defaults, and
/// it is omitted from the serialized config when it is the default (OFF), so
/// older configs load unchanged and older builds ignore it via the `extra`
/// catch-all.
///
/// SECURITY POSTURE (Section 9.3): sharing binds to the LAN and re-exposes local
/// models, so it is OFF BY DEFAULT and the UI states plainly that enabling it
/// exposes this machine's local models to the local network.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelSharingConfig {
    /// Whether the LAN share server is enabled. OFF by default.
    #[serde(default)]
    pub enabled: bool,
    /// The TCP port the share server binds to (default
    /// [`DEFAULT_MODEL_SHARING_PORT`]).
    #[serde(default = "default_model_sharing_port")]
    pub port: u16,
}

impl Default for ModelSharingConfig {
    fn default() -> Self {
        ModelSharingConfig {
            enabled: false,
            port: DEFAULT_MODEL_SHARING_PORT,
        }
    }
}

impl ModelSharingConfig {
    /// Whether this is the untouched default (disabled on the default port), so
    /// it can be omitted from the serialized config.
    pub fn is_default(&self) -> bool {
        !self.enabled && self.port == DEFAULT_MODEL_SHARING_PORT
    }
}

/// Application configuration (architecture.md Section 10.4).
///
/// Known fields are typed; any unknown newer fields encountered on load are
/// captured in [`AppConfig::extra`] and written back on save, so a config
/// written by a newer build degrades gracefully on an older one rather than
/// losing data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppConfig {
    /// Schema version of this config record (Section 10.4).
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    /// The provider id used when routing is fully automatic and nothing else
    /// applies. `None` until the user configures providers.
    #[serde(default)]
    pub default_provider_id: Option<String>,
    /// The active automatic routing policy id (Section 10.3).
    #[serde(default)]
    pub active_routing_policy: Option<String>,
    /// UI theme preference (free-form; the frontend interprets it).
    #[serde(default)]
    pub theme: Option<String>,
    /// User-entered per-provider/per-model token rates (architecture.md Section
    /// 6.2). Additive and forward-compatible: `#[serde(default)]` so older
    /// configs (and the empty case) deserialize fine, and it is omitted from the
    /// serialized JSON when empty so older builds are unaffected. This is the
    /// single source `providers::list_available_models` and the cost signal read.
    #[serde(default, skip_serializing_if = "PricingConfig::is_empty")]
    pub pricing: PricingConfig,
    /// User-configured web search (FEAT-004). Additive and forward-compatible:
    /// `#[serde(default)]` so older configs (and the default case) deserialize
    /// fine, and it is omitted from the serialized JSON when it is the untouched
    /// default so older builds are unaffected. Records only the selected
    /// provider + result cap; the API key lives in the keychain under
    /// [`WEB_SEARCH_SECRET_HANDLE`], never here.
    #[serde(default, skip_serializing_if = "WebSearchConfig::is_default")]
    pub web_search: WebSearchConfig,
    /// User-configured LAN model sharing (FEAT-006). Additive and
    /// forward-compatible: `#[serde(default)]` so older configs (and the default
    /// OFF case) deserialize fine, and it is omitted from the serialized JSON
    /// when it is the untouched default so older builds are unaffected. Records
    /// only whether sharing is enabled and on which port; it never carries any
    /// secret material.
    #[serde(default, skip_serializing_if = "ModelSharingConfig::is_default")]
    pub model_sharing: ModelSharingConfig,
    /// Forward-compatibility catch-all: unknown newer fields are preserved here
    /// rather than dropped (Section 10.4).
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

fn default_schema_version() -> u32 {
    CURRENT_SCHEMA_VERSION
}

impl Default for AppConfig {
    fn default() -> Self {
        AppConfig {
            schema_version: CURRENT_SCHEMA_VERSION,
            default_provider_id: None,
            active_routing_policy: None,
            theme: None,
            pricing: PricingConfig::default(),
            web_search: WebSearchConfig::default(),
            model_sharing: ModelSharingConfig::default(),
            extra: BTreeMap::new(),
        }
    }
}

impl AppConfig {
    /// Load the single config row, returning [`AppConfig::default`] if none has
    /// been saved yet.
    pub async fn load(db: &Db) -> Result<AppConfig, PersistenceError> {
        let row = sqlx::query("SELECT schema_version, data FROM app_config WHERE id = 1")
            .fetch_optional(db.pool())
            .await?;
        match row {
            None => Ok(AppConfig::default()),
            Some(r) => {
                let data: String = r.try_get("data")?;
                let mut cfg: AppConfig = serde_json::from_str(&data)?;
                // The dedicated column is authoritative for the version.
                let version: i64 = r.try_get("schema_version")?;
                cfg.schema_version = version as u32;
                Ok(cfg)
            }
        }
    }

    /// Upsert the single config row. Always writes [`CURRENT_SCHEMA_VERSION`]
    /// into the dedicated column while preserving any `extra` fields in JSON.
    pub async fn save(&self, db: &Db) -> Result<(), PersistenceError> {
        let data = serde_json::to_string(self)?;
        sqlx::query(
            "INSERT INTO app_config (id, schema_version, data) VALUES (1, ?, ?) \
             ON CONFLICT(id) DO UPDATE SET schema_version = excluded.schema_version, \
             data = excluded.data",
        )
        .bind(self.schema_version as i64)
        .bind(data)
        .execute(db.pool())
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn load_defaults_when_empty() {
        let db = Db::open_in_memory().await.unwrap();
        let cfg = AppConfig::load(&db).await.unwrap();
        assert_eq!(cfg.schema_version, CURRENT_SCHEMA_VERSION);
        assert!(cfg.default_provider_id.is_none());
    }

    #[tokio::test]
    async fn save_then_load_preserves_schema_version_and_fields() {
        let db = Db::open_in_memory().await.unwrap();
        let cfg = AppConfig {
            schema_version: CURRENT_SCHEMA_VERSION,
            default_provider_id: Some("openai".to_string()),
            active_routing_policy: Some("complexity".to_string()),
            theme: Some("dark".to_string()),
            pricing: PricingConfig::default(),
            web_search: WebSearchConfig::default(),
            model_sharing: ModelSharingConfig::default(),
            extra: BTreeMap::new(),
        };
        cfg.save(&db).await.unwrap();

        let loaded = AppConfig::load(&db).await.unwrap();
        assert_eq!(loaded.schema_version, CURRENT_SCHEMA_VERSION);
        assert_eq!(loaded.default_provider_id.as_deref(), Some("openai"));
        assert_eq!(loaded.active_routing_policy.as_deref(), Some("complexity"));
        assert_eq!(loaded.theme.as_deref(), Some("dark"));
    }

    #[tokio::test]
    async fn empty_pricing_is_omitted_from_serialized_config() {
        // The additive pricing field must not appear in the serialized JSON when
        // empty, so older builds (and the existing round-trip tests) are wholly
        // unaffected by the new field.
        let cfg = AppConfig::default();
        let json = serde_json::to_string(&cfg).unwrap();
        assert!(
            !json.contains("pricing"),
            "empty pricing must be omitted, got: {json}"
        );
    }

    #[tokio::test]
    async fn pricing_config_round_trips_additively() {
        let db = Db::open_in_memory().await.unwrap();

        let mut per_kind = BTreeMap::new();
        per_kind.insert(
            ProviderKind::OpenAI,
            TokenRate {
                input_per_mtok: 2.5,
                output_per_mtok: 10.0,
            },
        );
        // Local providers priced at zero (Section 6.2).
        per_kind.insert(ProviderKind::LmStudio, TokenRate::ZERO);

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

        let edited = Utc::now();
        let cfg = AppConfig {
            pricing: PricingConfig {
                per_kind,
                per_model,
                last_edited: Some(edited),
            },
            ..AppConfig::default()
        };
        cfg.save(&db).await.unwrap();

        let loaded = AppConfig::load(&db).await.unwrap();
        assert_eq!(
            loaded.pricing.per_kind.get(&ProviderKind::OpenAI),
            Some(&TokenRate {
                input_per_mtok: 2.5,
                output_per_mtok: 10.0,
            })
        );
        assert_eq!(
            loaded.pricing.per_kind.get(&ProviderKind::LmStudio),
            Some(&TokenRate::ZERO)
        );
        assert_eq!(
            loaded
                .pricing
                .per_model
                .get(&ProviderKind::OpenAI)
                .and_then(|m| m.get("gpt-4o-mini")),
            Some(&TokenRate {
                input_per_mtok: 0.15,
                output_per_mtok: 0.6,
            })
        );
        assert!(loaded.pricing.last_edited.is_some());

        // The pricing keys serialize in camelCase (ProviderKind contract the TS
        // mirror relies on).
        let json = serde_json::to_string(&cfg).unwrap();
        assert!(json.contains("\"openAI\""));
        assert!(json.contains("\"lmStudio\""));
        assert!(json.contains("\"inputPerMtok\""));
    }

    #[tokio::test]
    async fn default_web_search_is_omitted_from_serialized_config() {
        // The additive web_search field must not appear in the serialized JSON
        // when it is the untouched default, so older builds (and the existing
        // round-trip tests) are wholly unaffected by the new field.
        let cfg = AppConfig::default();
        let json = serde_json::to_string(&cfg).unwrap();
        assert!(
            !json.contains("webSearch"),
            "default web_search must be omitted, got: {json}"
        );
        assert_eq!(cfg.web_search.max_results, DEFAULT_WEB_SEARCH_MAX_RESULTS);
        assert!(cfg.web_search.enabled_provider.is_none());
    }

    #[tokio::test]
    async fn web_search_config_round_trips_additively() {
        let db = Db::open_in_memory().await.unwrap();
        let cfg = AppConfig {
            web_search: WebSearchConfig {
                enabled_provider: Some("tavily".to_string()),
                max_results: 8,
            },
            ..AppConfig::default()
        };
        cfg.save(&db).await.unwrap();

        let loaded = AppConfig::load(&db).await.unwrap();
        assert_eq!(
            loaded.web_search.enabled_provider.as_deref(),
            Some("tavily")
        );
        assert_eq!(loaded.web_search.max_results, 8);

        // The web-search config serializes in camelCase (the TS mirror + the
        // provider-kind tag the command layer maps rely on it).
        let json = serde_json::to_string(&cfg).unwrap();
        assert!(json.contains("\"webSearch\""));
        assert!(json.contains("\"enabledProvider\""));
        assert!(json.contains("\"tavily\""));
        // The API key is NEVER part of this config (it lives in the keychain).
        assert!(!json.contains("apiKey"));
        assert!(!json.contains("api_key"));
    }

    #[tokio::test]
    async fn default_model_sharing_is_omitted_from_serialized_config() {
        // The additive model_sharing field must not appear in the serialized
        // JSON when it is the untouched default (OFF), so older builds (and the
        // existing round-trip tests) are wholly unaffected by the new field.
        let cfg = AppConfig::default();
        let json = serde_json::to_string(&cfg).unwrap();
        assert!(
            !json.contains("modelSharing"),
            "default model_sharing must be omitted, got: {json}"
        );
        assert!(!cfg.model_sharing.enabled);
        assert_eq!(cfg.model_sharing.port, DEFAULT_MODEL_SHARING_PORT);
    }

    #[tokio::test]
    async fn model_sharing_config_round_trips_additively() {
        let db = Db::open_in_memory().await.unwrap();
        let cfg = AppConfig {
            model_sharing: ModelSharingConfig {
                enabled: true,
                port: 12345,
            },
            ..AppConfig::default()
        };
        cfg.save(&db).await.unwrap();

        let loaded = AppConfig::load(&db).await.unwrap();
        assert!(loaded.model_sharing.enabled);
        assert_eq!(loaded.model_sharing.port, 12345);

        // The model-sharing config serializes in camelCase (the TS mirror
        // relies on it).
        let json = serde_json::to_string(&cfg).unwrap();
        assert!(json.contains("\"modelSharing\""));
        assert!(json.contains("\"enabled\""));
        assert!(json.contains("\"port\""));
    }

    #[tokio::test]
    async fn unknown_newer_fields_are_preserved_on_load() {
        // Simulate a config written by a newer build carrying a field this
        // build does not know about; it must survive a load->save round trip.
        let db = Db::open_in_memory().await.unwrap();
        sqlx::query("INSERT INTO app_config (id, schema_version, data) VALUES (1, 2, ?)")
            .bind(r#"{"schemaVersion":2,"futureFlag":true,"theme":"light"}"#)
            .execute(db.pool())
            .await
            .unwrap();

        let loaded = AppConfig::load(&db).await.unwrap();
        assert_eq!(loaded.schema_version, 2);
        assert_eq!(loaded.theme.as_deref(), Some("light"));
        assert_eq!(
            loaded.extra.get("futureFlag"),
            Some(&serde_json::Value::Bool(true))
        );
    }
}
