//! Versioned app config load/save (architecture.md Section 10.4).
//!
//! [`AppConfig`] carries a `schema_version` so migrations can upgrade older
//! records forward. It is persisted as a single row in `app_config`: the
//! `schema_version` lives in its own column (so a migration can reason about it
//! without parsing JSON), and the remaining fields are a JSON blob loaded
//! forward-compatibly (unknown newer fields are preserved via a `#[serde(flatten)]`
//! catch-all rather than dropped - Section 10.4 additive-change bias).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::db::{Db, PersistenceError};
use sqlx::Row;

/// The current config schema version this build writes.
pub const CURRENT_SCHEMA_VERSION: u32 = 1;

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
