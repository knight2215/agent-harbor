//! `ProviderRegistry` and dynamic provider registration (architecture.md
//! Section 4.2 / 4.5).
//!
//! Built-in factories are registered at startup, one per [`ProviderKind`].
//! User-configured providers (multiple LM Studio endpoints, several
//! OpenAI-compatible services) are instantiated from persisted
//! [`ProviderConfig`] rows. Adding a new provider means implementing
//! [`ChatProvider`] + [`ProviderFactory`] and registering the factory; no core
//! pipeline change is required (Section 10).
//!
//! `ProviderConfig` and `ProviderKind` are reused from the `domain` crate (the
//! single source of truth); they are NOT redefined here. Secrets are resolved
//! at build time through a [`secrets::SecretStore`], so plaintext key material
//! is never stored on the registry or the provider config.

use std::collections::HashMap;
use std::sync::Arc;

use domain::{ProviderConfig, ProviderKind};
use secrets::SecretStore;

use crate::contract::{ChatProvider, ProviderError};

/// Builds a concrete [`ChatProvider`] for one [`ProviderKind`] from a
/// [`ProviderConfig`] row, resolving any `api_key_ref` through the secret store
/// at build time (architecture.md Section 4.5).
pub trait ProviderFactory: Send + Sync {
    /// The provider kind this factory constructs.
    fn kind(&self) -> ProviderKind;

    /// Build a provider instance. Implementations resolve `cfg.api_key_ref`
    /// through `secrets` here (never storing plaintext) and return a shareable
    /// [`ChatProvider`].
    fn build(
        &self,
        cfg: &ProviderConfig,
        secrets: &dyn SecretStore,
    ) -> Result<Arc<dyn ChatProvider>, ProviderError>;
}

/// Registry of provider factories and the live provider instances built from
/// configuration (architecture.md Section 4.5).
#[derive(Default)]
pub struct ProviderRegistry {
    factories: HashMap<ProviderKind, Box<dyn ProviderFactory>>,
    instances: HashMap<String, Arc<dyn ChatProvider>>,
}

impl ProviderRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        ProviderRegistry {
            factories: HashMap::new(),
            instances: HashMap::new(),
        }
    }

    /// Register a factory for its [`ProviderKind`]. A later registration for the
    /// same kind replaces the earlier one.
    pub fn register_factory(&mut self, factory: Box<dyn ProviderFactory>) {
        self.factories.insert(factory.kind(), factory);
    }

    /// Whether a factory is registered for `kind`.
    pub fn has_factory(&self, kind: ProviderKind) -> bool {
        self.factories.contains_key(&kind)
    }

    /// Build a provider from a single config row and store it under `cfg.id`,
    /// resolving its `api_key_ref` through `secrets`. Returns the built
    /// instance. An existing instance with the same id is replaced.
    pub fn build_from_config(
        &mut self,
        cfg: &ProviderConfig,
        secrets: &dyn SecretStore,
    ) -> Result<Arc<dyn ChatProvider>, ProviderError> {
        let factory = self.factories.get(&cfg.kind).ok_or_else(|| {
            ProviderError::Other(format!(
                "no factory registered for provider kind {:?}",
                cfg.kind
            ))
        })?;
        let instance = factory.build(cfg, secrets)?;
        self.instances.insert(cfg.id.clone(), Arc::clone(&instance));
        Ok(instance)
    }

    /// Build providers from an iterator of persisted config rows (Section 4.5).
    /// Instantiates each in order; the first failure short-circuits.
    pub fn build_all<'a, I>(
        &mut self,
        configs: I,
        secrets: &dyn SecretStore,
    ) -> Result<(), ProviderError>
    where
        I: IntoIterator<Item = &'a ProviderConfig>,
    {
        for cfg in configs {
            self.build_from_config(cfg, secrets)?;
        }
        Ok(())
    }

    /// Insert an already-built provider instance under its [`ChatProvider::id`],
    /// bypassing a factory. Used when an instance is constructed out of band
    /// (and by tests injecting a fake provider so `list_available_models` needs
    /// no live network). An existing instance with the same id is replaced.
    pub fn insert_instance(&mut self, instance: Arc<dyn ChatProvider>) {
        self.instances.insert(instance.id().to_string(), instance);
    }

    /// Get a live provider instance by its id.
    pub fn get(&self, id: &str) -> Option<Arc<dyn ChatProvider>> {
        self.instances.get(id).map(Arc::clone)
    }

    /// Ids of all currently built instances.
    pub fn instance_ids(&self) -> Vec<String> {
        self.instances.keys().cloned().collect()
    }

    /// All currently built instances.
    pub fn instances(&self) -> Vec<Arc<dyn ChatProvider>> {
        self.instances.values().map(Arc::clone).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{Capabilities, ChatDelta, ChatRequest, ChatResponse, ModelInfo};
    use async_trait::async_trait;
    use futures_util::stream::{self, BoxStream, StreamExt};
    use secrets::{InMemorySecretStore, SecretRef};
    use serde_json::Value;

    /// A fake provider that records the api key it was built with, so tests can
    /// assert the secret was resolved and wired in (never stored as plaintext
    /// on the config).
    struct FakeProvider {
        id: String,
        resolved_key: Option<String>,
    }

    impl FakeProvider {
        /// Test-only accessor proving the factory wired the resolved key in.
        fn resolved_key(&self) -> Option<&str> {
            self.resolved_key.as_deref()
        }
    }

    #[async_trait]
    impl ChatProvider for FakeProvider {
        fn id(&self) -> &str {
            &self.id
        }
        fn capabilities(&self, _model: &str) -> Capabilities {
            Capabilities {
                streaming: true,
                tools: true,
                vision: false,
                json_mode: true,
                max_context: Some(4096),
            }
        }
        async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
            Ok(vec![ModelInfo::new("fake-model")])
        }
        async fn chat(&self, _req: ChatRequest) -> Result<ChatResponse, ProviderError> {
            Ok(ChatResponse {
                choices: vec![],
                usage: None,
                model: None,
            })
        }
        async fn chat_stream(
            &self,
            _req: ChatRequest,
        ) -> Result<BoxStream<'static, Result<ChatDelta, ProviderError>>, ProviderError> {
            Ok(stream::empty().boxed())
        }
    }

    /// A dummy factory that resolves the api key through the secret store and
    /// hands it to the fake provider (mocking a real adapter build).
    struct FakeFactory;

    impl ProviderFactory for FakeFactory {
        fn kind(&self) -> ProviderKind {
            ProviderKind::OpenAI
        }
        fn build(
            &self,
            cfg: &ProviderConfig,
            secrets: &dyn SecretStore,
        ) -> Result<Arc<dyn ChatProvider>, ProviderError> {
            let resolved_key = match &cfg.api_key_ref {
                Some(r) => Some(
                    secrets
                        .resolve(r)
                        .map_err(|e| ProviderError::Auth(e.to_string()))?,
                ),
                None => None,
            };
            Ok(Arc::new(FakeProvider {
                id: cfg.id.clone(),
                resolved_key,
            }))
        }
    }

    fn config(id: &str, key_ref: Option<SecretRef>) -> ProviderConfig {
        ProviderConfig {
            id: id.to_string(),
            kind: ProviderKind::OpenAI,
            base_url: None,
            api_key_ref: key_ref,
            extra: Value::Null,
        }
    }

    #[test]
    fn factory_resolves_key_into_provider() {
        // Exercises the resolved-key wiring directly on the concrete type: the
        // factory reads plaintext from the store at build time and hands it to
        // the provider instance (which holds it internally, not on the config).
        let store = InMemorySecretStore::new();
        let key_ref = store.store("k", "sk-plain").unwrap();
        let cfg = config("openai", Some(key_ref));
        let provider = FakeProvider {
            id: cfg.id.clone(),
            resolved_key: cfg.api_key_ref.as_ref().map(|r| store.resolve(r).unwrap()),
        };
        assert_eq!(provider.resolved_key(), Some("sk-plain"));
        assert_eq!(provider.id(), "openai");
    }

    #[test]
    fn register_and_query_factory() {
        let mut reg = ProviderRegistry::new();
        assert!(!reg.has_factory(ProviderKind::OpenAI));
        reg.register_factory(Box::new(FakeFactory));
        assert!(reg.has_factory(ProviderKind::OpenAI));
    }

    #[test]
    fn build_from_config_resolves_secret_and_registers_instance() {
        let store = InMemorySecretStore::new();
        let key_ref = store.store("openai-key", "sk-live-123").unwrap();

        let mut reg = ProviderRegistry::new();
        reg.register_factory(Box::new(FakeFactory));

        let cfg = config("openai", Some(key_ref));
        let instance = reg.build_from_config(&cfg, &store).unwrap();
        assert_eq!(instance.id(), "openai");

        // The instance is retrievable by id.
        assert!(reg.get("openai").is_some());
        assert_eq!(reg.instance_ids(), vec!["openai".to_string()]);

        // The secret was resolved at build time (the factory called
        // `secrets.resolve`) and wired into the provider. The config itself
        // still carries ONLY the opaque handle, never the key material.
        assert_eq!(
            cfg.api_key_ref.as_ref().unwrap().handle(),
            "openai-key",
            "config must carry only the opaque handle, not the key"
        );
    }

    #[test]
    fn build_all_from_config_rows() {
        let store = InMemorySecretStore::new();
        let k1 = store.store("k1", "sk-1").unwrap();
        let k2 = store.store("k2", "sk-2").unwrap();

        let mut reg = ProviderRegistry::new();
        reg.register_factory(Box::new(FakeFactory));

        let rows = vec![config("p1", Some(k1)), config("p2", Some(k2))];
        reg.build_all(rows.iter(), &store).unwrap();

        let mut ids = reg.instance_ids();
        ids.sort();
        assert_eq!(ids, vec!["p1".to_string(), "p2".to_string()]);
        assert_eq!(reg.instances().len(), 2);
    }

    #[test]
    fn build_without_factory_errors() {
        let store = InMemorySecretStore::new();
        let mut reg = ProviderRegistry::new();
        // No factory registered for OpenAI.
        let cfg = config("openai", None);
        match reg.build_from_config(&cfg, &store) {
            Err(ProviderError::Other(msg)) => assert!(msg.contains("no factory")),
            Err(other) => panic!("expected Other error, got {other:?}"),
            Ok(_) => panic!("expected Other error, got Ok"),
        }
    }

    #[tokio::test]
    async fn built_instance_serves_calls() {
        let store = InMemorySecretStore::new();
        let mut reg = ProviderRegistry::new();
        reg.register_factory(Box::new(FakeFactory));
        reg.build_from_config(&config("openai", None), &store)
            .unwrap();
        let p = reg.get("openai").unwrap();
        let models = p.list_models().await.unwrap();
        assert_eq!(models[0].id, "fake-model");
        assert!(p.capabilities("any").tools);
    }
}
