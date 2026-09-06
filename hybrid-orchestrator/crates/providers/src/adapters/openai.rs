//! Native OpenAI adapter (architecture.md Section 4.3, tasks.md P2.4).
//!
//! Direct pass-through of the internal OpenAI-compatible [`ChatRequest`] to
//! `POST {base_url}/chat/completions` with `Authorization: Bearer <key>`. All
//! request-building, non-streaming decode, and SSE normalization live in the
//! shared [`super::native::NativeAdapter`]; this module only wires the
//! OpenAI-specific base_url + Bearer auth and provides a [`ProviderFactory`].

use std::sync::Arc;

use domain::{ProviderConfig, ProviderKind};
use secrets::SecretStore;

use super::native::{default_openai_caps, AuthStrategy, NativeAdapter, Routing};
use crate::contract::{ChatProvider, ProviderError};
use crate::registry::ProviderFactory;

/// Default OpenAI API root when `ProviderConfig::base_url` is unset.
pub const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";

/// Build the shared native adapter configured for OpenAI: the config's
/// `base_url` (or the default) and Bearer auth from the resolved key.
///
/// The key is resolved through `secrets` here (build time) and lives only
/// inside the returned adapter's [`AuthStrategy`]; it is never stored on the
/// config. OpenAI requires a key: a missing `api_key_ref` is an [`Auth`] error.
///
/// [`Auth`]: ProviderError::Auth
pub fn build_openai(
    cfg: &ProviderConfig,
    secrets: &dyn SecretStore,
) -> Result<NativeAdapter, ProviderError> {
    let key_ref = cfg.api_key_ref.as_ref().ok_or_else(|| {
        ProviderError::Auth("OpenAI provider requires an api_key_ref".to_string())
    })?;
    let key = secrets
        .resolve(key_ref)
        .map_err(|e| ProviderError::Auth(e.to_string()))?;
    let base_url = cfg
        .base_url
        .clone()
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
    Ok(NativeAdapter::new(
        cfg.id.clone(),
        base_url,
        AuthStrategy::Bearer(key),
        Routing::Standard,
        default_openai_caps(),
    ))
}

/// [`ProviderFactory`] for [`ProviderKind::OpenAI`].
pub struct OpenAiFactory;

impl ProviderFactory for OpenAiFactory {
    fn kind(&self) -> ProviderKind {
        ProviderKind::OpenAI
    }

    fn build(
        &self,
        cfg: &ProviderConfig,
        secrets: &dyn SecretStore,
    ) -> Result<Arc<dyn ChatProvider>, ProviderError> {
        Ok(Arc::new(build_openai(cfg, secrets)?))
    }
}
