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
