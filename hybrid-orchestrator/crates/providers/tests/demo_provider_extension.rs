//! Extensibility validation for the provider seam (architecture.md Section 10,
//! tasks.md P6.5). This is a TEST-ONLY artifact: it ships nothing to production.
//!
//! It proves the "add a provider without touching the core" claim from Section
//! 10: a brand-new [`ChatProvider`] + [`ProviderFactory`] pair, defined entirely
//! here in the test, can be
//!
//!   1. registered on a [`ProviderRegistry`] built from [`builtin_registry`],
//!   2. instantiated from an ordinary [`ProviderConfig`] row (the same shape a
//!      user persists), and
//!   3. surfaced through [`list_available_models`] as an [`AvailableModel`] that
//!      the model selector (Section 8.2) and the routing engine (Section 6.1)
//!      consume unmodified.
//!
//! ZERO-EDIT PROOF: this file is the ONLY thing added for the demo. No file in
//! `providers/src`, `routing`, or `orchestrator-core` changes to make the demo
//! provider register, build, or enumerate. The registry accepts the demo
//! factory purely through its public [`ProviderRegistry::register_factory`] +
//! [`ProviderRegistry::build_from_config`] seam.
//!
//! NOTE on [`ProviderKind`]: it is a FIXED enum in `domain`, so the demo does
//! NOT invent a new kind (that would be a domain edit). It registers its factory
//! against an existing kind ([`ProviderKind::GenericOpenAI`], the
//! any-OpenAI-compatible-endpoint kind) - which is exactly the real-world story:
//! any OpenAI-compatible endpoint needs no new adapter at all, only a config row
//! pointing [`ProviderKind::GenericOpenAI`] at its base URL. This test exercises
//! the factory/registry seam itself with a self-contained fake so it needs no
//! live network.

use std::sync::Arc;

use async_trait::async_trait;
use futures_util::stream::{self, BoxStream, StreamExt};
use serde_json::Value;

use domain::{ProviderConfig, ProviderKind};
use providers::{
    builtin_registry, list_available_models, Capabilities, ChatDelta, ChatProvider, ChatRequest,
    ChatResponse, MessageRole, ModelInfo, PricingTable, ProviderError, ProviderFactory,
    ProviderRegistry, TokenPrice,
};
use secrets::{InMemorySecretStore, SecretStore};

/// A demo "echo" [`ChatProvider`]: it advertises a couple of models and answers
/// `chat` by echoing the last user message back. This is a TEST fake - it never
/// touches the network - so it doubles as a self-contained example of the
/// minimal surface a new adapter must implement.
#[derive(Debug)]
struct EchoProvider {
    id: String,
}

#[async_trait]
impl ChatProvider for EchoProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn capabilities(&self, _model: &str) -> Capabilities {
        // A modest, tool-capable local-ish model so routing can select it.
        Capabilities {
            streaming: true,
            tools: true,
            vision: false,
            json_mode: true,
            max_context: Some(8_192),
        }
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        Ok(vec![
            ModelInfo::new("echo-small"),
            ModelInfo::new("echo-large"),
        ])
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, ProviderError> {
        let echoed = req
            .messages
            .iter()
            .rev()
            .find(|m| matches!(m.role, MessageRole::User))
            .and_then(|m| m.content.clone())
            .unwrap_or_default();
        Ok(ChatResponse {
            choices: vec![],
            usage: None,
            model: Some(echoed),
        })
    }

    async fn chat_stream(
        &self,
        _req: ChatRequest,
    ) -> Result<BoxStream<'static, Result<ChatDelta, ProviderError>>, ProviderError> {
        Ok(stream::empty().boxed())
    }
}

/// A demo [`ProviderFactory`] that builds an [`EchoProvider`]. It resolves an
/// optional `api_key_ref` through the secret store at build time (never storing
/// plaintext), exactly like a real factory - though the echo provider ignores
/// the key. Registered against the existing [`ProviderKind::GenericOpenAI`]
/// seam so the demo needs no `domain` edit.
struct EchoFactory;

impl ProviderFactory for EchoFactory {
    fn kind(&self) -> ProviderKind {
        ProviderKind::GenericOpenAI
    }

    fn build(
        &self,
        cfg: &ProviderConfig,
        secrets: &dyn SecretStore,
    ) -> Result<Arc<dyn ChatProvider>, ProviderError> {
        // Resolve the key through the store to mirror a real adapter build (the
        // resolved plaintext stays local and is intentionally unused here).
        if let Some(key_ref) = &cfg.api_key_ref {
            let _resolved = secrets
                .resolve(key_ref)
                .map_err(|e| ProviderError::Auth(e.to_string()))?;
        }
        Ok(Arc::new(EchoProvider { id: cfg.id.clone() }))
    }
}

fn demo_config(id: &str) -> ProviderConfig {
    ProviderConfig {
        id: id.to_string(),
        kind: ProviderKind::GenericOpenAI,
        base_url: Some("http://localhost:9/v1".to_string()),
        api_key_ref: None,
        extra: Value::Null,
    }
}

/// The demo factory registers on a builtin registry and REPLACES the built-in
/// factory for its kind (registration is last-writer-wins per kind), proving the
/// public `register_factory` seam accepts a brand-new adapter with no core edit.
#[test]
fn demo_factory_registers_on_builtin_registry() {
    let mut registry = builtin_registry();
    assert!(registry.has_factory(ProviderKind::GenericOpenAI));
    // Registering the demo factory for the same kind is accepted (it swaps in).
    registry.register_factory(Box::new(EchoFactory));
    assert!(registry.has_factory(ProviderKind::GenericOpenAI));
}

/// End-to-end for P6.5: register the demo factory, build an instance from a
/// config row, and drive it through [`list_available_models`] so it surfaces as
/// an [`AvailableModel`] visible to the model selector.
#[tokio::test]
async fn demo_provider_surfaces_as_available_model() {
    let store = InMemorySecretStore::new();
    let mut registry = builtin_registry();
    registry.register_factory(Box::new(EchoFactory));

    let cfg = demo_config("demo-echo");
    // Build the instance from the config row through the public factory seam.
    let instance = registry
        .build_from_config(&cfg, &store)
        .expect("demo factory builds an instance from a config row");
    assert_eq!(instance.id(), "demo-echo");

    // The model selector / routing data source enumerates it with no core edit.
    let configs = vec![cfg];
    let pricing = PricingTable::bundled_defaults();
    let models = list_available_models(&registry, &configs, &pricing)
        .await
        .expect("list_available_models drives the demo provider")
        .models;

    let ids: Vec<&str> = models.iter().map(|m| m.model.as_str()).collect();
    assert!(
        ids.contains(&"echo-small") && ids.contains(&"echo-large"),
        "demo provider models must surface as AvailableModels, got {ids:?}"
    );

    // The surfaced rows are attributed to the demo instance and carry the
    // negotiated capabilities (tools-capable, so routing can select them).
    let small = models
        .iter()
        .find(|m| m.model == "echo-small")
        .expect("echo-small surfaced");
    assert_eq!(small.provider_id, "demo-echo");
    assert!(small.capabilities.tools);
    // GenericOpenAI is priced at zero by the bundled defaults (user-defined
    // endpoint), so the demo model is a zero-cost candidate.
    assert_eq!(small.price, TokenPrice::ZERO);
}

/// A demo instance inserted out of band (no factory) also surfaces, proving the
/// `insert_instance` seam works for adapters constructed however the extender
/// likes. This is the path a plugin that builds its own instance would use.
#[tokio::test]
async fn demo_provider_via_insert_instance_surfaces() {
    let mut registry = ProviderRegistry::new();
    registry.insert_instance(Arc::new(EchoProvider {
        id: "demo-inserted".to_string(),
    }));

    let cfg = demo_config("demo-inserted");
    let models = list_available_models(&registry, &[cfg], &PricingTable::new())
        .await
        .expect("list_available_models drives the inserted demo provider")
        .models;
    assert_eq!(models.len(), 2);
    assert!(models.iter().all(|m| m.provider_id == "demo-inserted"));
}
