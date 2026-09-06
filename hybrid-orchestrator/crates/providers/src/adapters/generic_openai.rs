//! Generic OpenAI-compatible adapter (architecture.md Section 4.3, tasks.md
//! P2.5).
//!
//! The escape hatch for ANY user-supplied OpenAI-compatible endpoint. Reuses
//! the same shared code path as OpenAI/LM Studio
//! ([`super::native::NativeAdapter`]); the `base_url` is user-supplied and
//! REQUIRED (there is no sensible default), and the Bearer key is optional.

use std::sync::Arc;

use domain::{ProviderConfig, ProviderKind};
use secrets::SecretStore;

use super::native::{default_openai_caps, AuthStrategy, NativeAdapter, Routing};
use crate::contract::{ChatProvider, ProviderError};
use crate::registry::ProviderFactory;

/// Build the shared native adapter for a generic OpenAI-compatible endpoint.
///
/// `base_url` is required: a missing one is a clear [`ProviderError::Other`]
/// (this is the user-supplied-endpoint escape hatch, so there is nothing to
/// default to). A Bearer key is attached only when `api_key_ref` is set;
/// otherwise no auth header is sent.
pub fn build_generic_openai(
    cfg: &ProviderConfig,
    secrets: &dyn SecretStore,
) -> Result<NativeAdapter, ProviderError> {
    let base_url = cfg.base_url.clone().ok_or_else(|| {
        ProviderError::Other(
            "genericOpenAI provider requires an explicit base_url (no default)".to_string(),
        )
    })?;
    if base_url.trim().is_empty() {
        return Err(ProviderError::Other(
            "genericOpenAI provider base_url must not be empty".to_string(),
        ));
    }

    let auth = match &cfg.api_key_ref {
        Some(key_ref) => {
            let key = secrets
                .resolve(key_ref)
                .map_err(|e| ProviderError::Auth(e.to_string()))?;
            AuthStrategy::Bearer(key)
        }
        None => AuthStrategy::None,
    };

    Ok(NativeAdapter::new(
        cfg.id.clone(),
        base_url,
        auth,
        Routing::Standard,
        default_openai_caps(),
    ))
}

/// [`ProviderFactory`] for [`ProviderKind::GenericOpenAI`].
pub struct GenericOpenAiFactory;

impl ProviderFactory for GenericOpenAiFactory {
    fn kind(&self) -> ProviderKind {
        ProviderKind::GenericOpenAI
    }

    fn build(
        &self,
        cfg: &ProviderConfig,
        secrets: &dyn SecretStore,
    ) -> Result<Arc<dyn ChatProvider>, ProviderError> {
        Ok(Arc::new(build_generic_openai(cfg, secrets)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::ProviderKind;
    use secrets::InMemorySecretStore;
    use serde_json::Value;

    fn cfg(base_url: Option<String>) -> ProviderConfig {
        ProviderConfig {
            id: "generic".to_string(),
            kind: ProviderKind::GenericOpenAI,
            base_url,
            api_key_ref: None,
            extra: Value::Null,
        }
    }

    #[test]
    fn missing_base_url_errors_clearly() {
        let store = InMemorySecretStore::new();
        match build_generic_openai(&cfg(None), &store) {
            Err(ProviderError::Other(msg)) => assert!(msg.contains("base_url")),
            other => panic!("expected Other error about base_url, got {other:?}"),
        }
    }

    #[test]
    fn empty_base_url_errors_clearly() {
        let store = InMemorySecretStore::new();
        match build_generic_openai(&cfg(Some("   ".to_string())), &store) {
            Err(ProviderError::Other(msg)) => assert!(msg.contains("base_url")),
            other => panic!("expected Other error about empty base_url, got {other:?}"),
        }
    }

    #[test]
    fn user_base_url_is_accepted() {
        let store = InMemorySecretStore::new();
        let adapter =
            build_generic_openai(&cfg(Some("https://example.test/v1".to_string())), &store)
                .expect("build generic adapter");
        assert_eq!(ChatProvider::id(&adapter), "generic");
    }
}
