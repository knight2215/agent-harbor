//! Embedded local inference engine adapter (architecture.md Section 4,
//! Strategy B / FEAT-002).
//!
//! This adapter surfaces the `engine` crate's [`engine::EmbeddedEngine`] behind
//! the [`ProviderKind::Embedded`] factory, so the in-process llama.cpp-backed
//! engine is registered and routed exactly like the HTTP adapters. The engine
//! needs NO API key (it runs locally), so [`EmbeddedFactory::build`] never
//! touches the secret store.
//!
//! The engine's models directory is resolved from the [`ProviderConfig`]: a
//! `base_url` value, when present, is treated as the on-disk directory that
//! bare relative `.gguf` file names resolve against; when absent the engine
//! uses paths as-given. Registered GGUF entries are managed at runtime through
//! the Tauri embedded-model commands (FEAT-002), not from static config.
//!
//! The native inference path lives entirely behind the engine crate's `llama`
//! feature (default OFF). This adapter builds the engine the same way whether or
//! not `llama` is enabled: under default features the engine is a stub whose
//! `chat`/`chat_stream` return a clear "not compiled" error, and the routine
//! providers build needs no native library.

use std::sync::Arc;

use domain::{ProviderConfig, ProviderKind};
use secrets::SecretStore;

use crate::contract::{ChatProvider, ProviderError};
use crate::registry::ProviderFactory;

use engine::{EmbeddedEngine, EngineConfig};

/// Build an [`EmbeddedEngine`] from a [`ProviderConfig`]. A `base_url`, when
/// present, seeds the engine's models directory (relative `.gguf` paths resolve
/// against it); otherwise the engine resolves paths as-given. No API key is
/// consulted: the embedded engine runs in-process and is always local.
pub fn build_embedded(cfg: &ProviderConfig) -> EmbeddedEngine {
    let config = match cfg.base_url.as_deref() {
        Some(dir) if !dir.is_empty() => EngineConfig::new().with_models_dir(dir),
        _ => EngineConfig::new(),
    };
    EmbeddedEngine::new(config)
}

/// [`ProviderFactory`] for [`ProviderKind::Embedded`].
pub struct EmbeddedFactory;

impl ProviderFactory for EmbeddedFactory {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Embedded
    }

    fn build(
        &self,
        cfg: &ProviderConfig,
        _secrets: &dyn SecretStore,
    ) -> Result<Arc<dyn ChatProvider>, ProviderError> {
        Ok(Arc::new(build_embedded(cfg)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use secrets::InMemorySecretStore;
    use serde_json::Value;

    fn config(base_url: Option<&str>) -> ProviderConfig {
        ProviderConfig {
            id: "embedded-local".to_string(),
            kind: ProviderKind::Embedded,
            base_url: base_url.map(str::to_string),
            api_key_ref: None,
            extra: Value::Null,
        }
    }

    #[test]
    fn factory_reports_embedded_kind() {
        assert_eq!(EmbeddedFactory.kind(), ProviderKind::Embedded);
    }

    #[test]
    fn builds_without_secret_store_or_api_key() {
        // The embedded engine is always local and needs no key: build must
        // succeed with an empty store and no `api_key_ref`.
        let store = InMemorySecretStore::new();
        let instance = EmbeddedFactory
            .build(&config(None), &store)
            .expect("build embedded engine");
        assert_eq!(instance.id(), "embedded");
        // Streaming text only: tools/vision/json are unsupported.
        let caps = instance.capabilities("any-model");
        assert!(caps.streaming);
        assert!(!caps.tools);
        assert!(!caps.vision);
        assert!(!caps.json_mode);
    }

    #[test]
    fn builds_with_models_dir_from_base_url() {
        let store = InMemorySecretStore::new();
        let instance = EmbeddedFactory
            .build(&config(Some("/data/models")), &store)
            .expect("build embedded engine");
        assert_eq!(instance.id(), "embedded");
    }
}
