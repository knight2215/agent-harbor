//! Azure OpenAI adapter (architecture.md Section 4.3, tasks.md P2.6).
//!
//! Near-direct OpenAI-compatible: the wire body is identical to OpenAI, but the
//! request path is rewritten to
//! `/openai/deployments/{deployment}/chat/completions`, an `api-version` query
//! parameter is added, and auth uses an `api-key` header (NOT `Bearer`). The
//! deployment name and API version come from `ProviderConfig::extra`
//! (`extra.deployment`, `extra.api_version`). Everything else reuses the shared
//! [`super::native::NativeAdapter`] code path.

use std::sync::Arc;

use domain::{ProviderConfig, ProviderKind};
use secrets::SecretStore;

use super::native::{default_openai_caps, AuthStrategy, NativeAdapter, Routing};
use crate::contract::{ChatProvider, ProviderError};
use crate::registry::ProviderFactory;

/// Read a required string field from `ProviderConfig::extra`, erroring clearly
/// when it is missing or not a string.
fn required_extra(cfg: &ProviderConfig, key: &str) -> Result<String, ProviderError> {
    cfg.extra
        .get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| {
            ProviderError::Other(format!(
                "Azure provider requires a non-empty string `extra.{key}`"
            ))
        })
}

/// Build the shared native adapter configured for Azure OpenAI: `base_url` is
/// the resource endpoint, routing rewrites to the deployment path with an
/// `api-version` query, and auth is the `api-key` header from the resolved key.
///
/// Errors clearly if `base_url`, `api_key_ref`, `extra.deployment`, or
/// `extra.api_version` are missing.
pub fn build_azure_openai(
    cfg: &ProviderConfig,
    secrets: &dyn SecretStore,
) -> Result<NativeAdapter, ProviderError> {
    let base_url = cfg.base_url.clone().ok_or_else(|| {
        ProviderError::Other(
            "Azure provider requires a base_url (the resource endpoint)".to_string(),
        )
    })?;
    let deployment = required_extra(cfg, "deployment")?;
    let api_version = required_extra(cfg, "api_version")?;

    let key_ref = cfg
        .api_key_ref
        .as_ref()
        .ok_or_else(|| ProviderError::Auth("Azure provider requires an api_key_ref".to_string()))?;
    let key = secrets
        .resolve(key_ref)
        .map_err(|e| ProviderError::Auth(e.to_string()))?;

    Ok(NativeAdapter::new(
        cfg.id.clone(),
        base_url,
        AuthStrategy::ApiKeyHeader(key),
        Routing::Azure {
            deployment,
            api_version,
        },
        default_openai_caps(),
    ))
}

/// [`ProviderFactory`] for [`ProviderKind::Azure`].
pub struct AzureOpenAiFactory;

impl ProviderFactory for AzureOpenAiFactory {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Azure
    }

    fn build(
        &self,
        cfg: &ProviderConfig,
        secrets: &dyn SecretStore,
    ) -> Result<Arc<dyn ChatProvider>, ProviderError> {
        Ok(Arc::new(build_azure_openai(cfg, secrets)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use secrets::InMemorySecretStore;
    use serde_json::json;

    fn cfg(base_url: Option<&str>, extra: serde_json::Value, with_key: bool) -> ProviderConfig {
        let store = InMemorySecretStore::new();
        let api_key_ref = if with_key {
            Some(store.store("azure-key", "az-secret").unwrap())
        } else {
            None
        };
        ProviderConfig {
            id: "azure".to_string(),
            kind: ProviderKind::Azure,
            base_url: base_url.map(|s| s.to_string()),
            api_key_ref,
            extra,
        }
    }

    #[test]
    fn missing_deployment_errors() {
        let store = InMemorySecretStore::new();
        let c = cfg(
            Some("https://res.openai.azure.com"),
            json!({"api_version": "2024-02-01"}),
            true,
        );
        match build_azure_openai(&c, &store) {
            Err(ProviderError::Other(msg)) => assert!(msg.contains("deployment")),
            other => panic!("expected deployment error, got {other:?}"),
        }
    }

    #[test]
    fn missing_api_version_errors() {
        let store = InMemorySecretStore::new();
        let c = cfg(
            Some("https://res.openai.azure.com"),
            json!({"deployment": "gpt4o"}),
            true,
        );
        match build_azure_openai(&c, &store) {
            Err(ProviderError::Other(msg)) => assert!(msg.contains("api_version")),
            other => panic!("expected api_version error, got {other:?}"),
        }
    }

    #[test]
    fn missing_base_url_errors() {
        let store = InMemorySecretStore::new();
        let c = cfg(
            None,
            json!({"deployment": "gpt4o", "api_version": "2024-02-01"}),
            true,
        );
        match build_azure_openai(&c, &store) {
            Err(ProviderError::Other(msg)) => assert!(msg.contains("base_url")),
            other => panic!("expected base_url error, got {other:?}"),
        }
    }

    #[test]
    fn missing_key_errors() {
        let store = InMemorySecretStore::new();
        let c = cfg(
            Some("https://res.openai.azure.com"),
            json!({"deployment": "gpt4o", "api_version": "2024-02-01"}),
            false,
        );
        match build_azure_openai(&c, &store) {
            Err(ProviderError::Auth(msg)) => assert!(msg.contains("api_key_ref")),
            other => panic!("expected auth error, got {other:?}"),
        }
    }

    #[test]
    fn valid_config_builds() {
        // Resolve the key through the SAME store the ref was created in.
        let store = InMemorySecretStore::new();
        let api_key_ref = store.store("azure-key", "az-secret").unwrap();
        let c = ProviderConfig {
            id: "azure".to_string(),
            kind: ProviderKind::Azure,
            base_url: Some("https://res.openai.azure.com".to_string()),
            api_key_ref: Some(api_key_ref),
            extra: json!({"deployment": "gpt4o", "api_version": "2024-02-01"}),
        };
        let adapter = build_azure_openai(&c, &store).expect("build azure adapter");
        assert_eq!(ChatProvider::id(&adapter), "azure");
    }
}
