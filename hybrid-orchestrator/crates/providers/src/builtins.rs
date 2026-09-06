//! Built-in provider registry wiring and `list_available_models` (architecture.md
//! Section 4.5 registry + Section 6.1 / 6.2 / 8.2 model selector data).
//!
//! [`builtin_registry`] constructs a [`ProviderRegistry`] with all seven
//! built-in [`ProviderFactory`]s registered, one per [`ProviderKind`] (Section
//! 4.5: "built-in factories are registered at startup"). [`build_registry`]
//! layers on the user-configured provider instances from persisted
//! [`ProviderConfig`] rows.
//!
//! [`list_available_models`] is the data source the Phase 5 model selector
//! (Section 8.2) and Phase 4 routing (Section 6.1) consume: for every configured
//! provider instance it lists the provider's models and attaches per-model
//! [`Capabilities`] and an optional [`TokenPrice`] resolved from a
//! [`PricingTable`]. Local providers (LM Studio) default to zero price
//! (Section 6.2). Because [`ChatProvider::list_models`] may hit the network, the
//! function drives the trait method, so tests inject a fake provider whose
//! `list_models` returns a fixed list (no live network).
//!
//! SECRET HYGIENE (Section 9.1 / 9.2): [`AvailableModel`] carries only
//! provider/model/capabilities/price labels; it never carries secret material or
//! a resolved [`secrets::SecretRef`]. Secrets are resolved (if at all) only when
//! a factory builds an instance, and stay inside the instance.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use domain::{ProviderConfig, ProviderKind};
use secrets::SecretStore;

use crate::contract::{Capabilities, ProviderError};
use crate::registry::ProviderRegistry;
use crate::{
    AnthropicFactory, AzureOpenAiFactory, BedrockFactory, GeminiFactory, GenericOpenAiFactory,
    LmStudioFactory, OpenAiFactory,
};

/// Per-model token pricing in currency units per one million tokens
/// (architecture.md Section 6.2). Both rates default to zero so a partially
/// filled entry (or a local provider) is a valid zero-cost price rather than a
/// missing one. This is the single price shape the cost signal and
/// [`list_available_models`] both read; the persisted user-entered rates in
/// `persistence::AppConfig` mirror this shape.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenPrice {
    /// Input (prompt) token rate, currency units per 1M tokens.
    #[serde(default)]
    pub input_per_mtok: f64,
    /// Output (completion) token rate, currency units per 1M tokens.
    #[serde(default)]
    pub output_per_mtok: f64,
}

impl TokenPrice {
    /// A zero-cost price, used for local providers (LM Studio) and as the
    /// default for unpriced entries (Section 6.2).
    pub const ZERO: TokenPrice = TokenPrice {
        input_per_mtok: 0.0,
        output_per_mtok: 0.0,
    };

    /// Construct a price from explicit input/output per-million-token rates.
    pub fn new(input_per_mtok: f64, output_per_mtok: f64) -> Self {
        TokenPrice {
            input_per_mtok,
            output_per_mtok,
        }
    }
}

impl Default for TokenPrice {
    fn default() -> Self {
        TokenPrice::ZERO
    }
}

/// Token-rate pricing resolved per provider kind, with optional per-model
/// overrides (architecture.md Section 6.2). Keyed by [`ProviderKind`] for the
/// coarse default and by model id for overrides; a lookup falls back
/// kind-default -> zero.
///
/// This is the in-memory shape [`list_available_models`] reads. The persisted,
/// user-entered rates live in the versioned `persistence::AppConfig` (the single
/// source of truth, Section 6.2); the Tauri command converts the persisted table
/// into this shape before calling `list_available_models`.
#[derive(Debug, Clone, Default)]
pub struct PricingTable {
    /// One default price per provider kind (local kinds default to zero).
    per_kind: BTreeMap<ProviderKind, TokenPrice>,
    /// Optional per-model overrides, keyed by `(kind, model_id)`.
    per_model: BTreeMap<(ProviderKind, String), TokenPrice>,
}

impl PricingTable {
    /// An empty table: every lookup resolves to [`TokenPrice::ZERO`].
    pub fn new() -> Self {
        PricingTable::default()
    }

    /// The bundled defaults seeded per [`ProviderKind`] (architecture.md Section
    /// 6.2: "seeded with optional bundled defaults per ProviderKind; local
    /// providers default to zero"). These are conservative, user-overridable
    /// placeholders, NOT authoritative vendor prices; the user-entered rates in
    /// `AppConfig` take precedence once set.
    pub fn bundled_defaults() -> Self {
        let mut table = PricingTable::new();
        // Cloud kinds seeded with nonzero placeholder rates; local kinds (LM
        // Studio) stay at zero. GenericOpenAI is left at zero because its
        // endpoint (and pricing) is user-defined.
        table.set_kind(ProviderKind::OpenAI, TokenPrice::new(2.5, 10.0));
        table.set_kind(ProviderKind::Anthropic, TokenPrice::new(3.0, 15.0));
        table.set_kind(ProviderKind::Bedrock, TokenPrice::new(3.0, 15.0));
        table.set_kind(ProviderKind::Gemini, TokenPrice::new(1.25, 5.0));
        table.set_kind(ProviderKind::Azure, TokenPrice::new(2.5, 10.0));
        table.set_kind(ProviderKind::LmStudio, TokenPrice::ZERO);
        table.set_kind(ProviderKind::GenericOpenAI, TokenPrice::ZERO);
        table
    }

    /// Set the default price for a provider kind.
    pub fn set_kind(&mut self, kind: ProviderKind, price: TokenPrice) {
        self.per_kind.insert(kind, price);
    }

    /// Set a per-model price override for a `(kind, model)` pair.
    pub fn set_model(&mut self, kind: ProviderKind, model: impl Into<String>, price: TokenPrice) {
        self.per_model.insert((kind, model.into()), price);
    }

    /// Resolve the price for a `(kind, model)`: a per-model override wins, then
    /// the per-kind default, then [`TokenPrice::ZERO`]. Local providers resolve
    /// to zero unless the user explicitly priced them (Section 6.2).
    pub fn price_for(&self, kind: ProviderKind, model: &str) -> TokenPrice {
        if let Some(p) = self.per_model.get(&(kind, model.to_string())) {
            return *p;
        }
        if let Some(p) = self.per_kind.get(&kind) {
            return *p;
        }
        TokenPrice::ZERO
    }
}

/// One selectable model surfaced to routing (Section 6.1) and the model-selector
/// UI (Section 8.2): the provider instance id, the model id, the negotiated
/// [`Capabilities`], and the resolved [`TokenPrice`].
///
/// DISPLAY-SAFE: this is the exact shape that crosses the Tauri IPC boundary.
/// It carries only provider/model/capabilities/price labels, NEVER secret
/// material (Section 9.1 / 9.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AvailableModel {
    /// The configured provider instance id (`ProviderConfig::id`).
    pub provider_id: String,
    /// The provider-scoped model id, e.g. `gpt-4o`.
    pub model: String,
    /// What this provider/model can do (streaming, tools, vision, json, ctx).
    pub capabilities: Capabilities,
    /// Resolved per-model token price; zero for local providers (Section 6.2).
    pub price: TokenPrice,
}

/// Construct a [`ProviderRegistry`] with every built-in [`ProviderFactory`]
/// registered, one per [`ProviderKind`] (architecture.md Section 4.5). No
/// instances are built yet; call [`ProviderRegistry::build_all`] (or
/// [`build_registry`]) with persisted [`ProviderConfig`] rows to instantiate.
pub fn builtin_registry() -> ProviderRegistry {
    let mut registry = ProviderRegistry::new();
    registry.register_factory(Box::new(OpenAiFactory));
    registry.register_factory(Box::new(LmStudioFactory));
    registry.register_factory(Box::new(GenericOpenAiFactory));
    registry.register_factory(Box::new(AzureOpenAiFactory));
    registry.register_factory(Box::new(AnthropicFactory));
    registry.register_factory(Box::new(GeminiFactory));
    registry.register_factory(Box::new(BedrockFactory));
    registry
}

/// Build a [`builtin_registry`] and instantiate every configured provider from
/// the persisted [`ProviderConfig`] rows, resolving each row's `api_key_ref`
/// through `secrets` at build time (Section 4.5). The first build failure
/// short-circuits.
pub fn build_registry<'a, I>(
    configs: I,
    secrets: &dyn SecretStore,
) -> Result<ProviderRegistry, ProviderError>
where
    I: IntoIterator<Item = &'a ProviderConfig>,
{
    let mut registry = builtin_registry();
    registry.build_all(configs, secrets)?;
    Ok(registry)
}

/// List every available model across all built provider instances, attaching
/// per-model [`Capabilities`] and the [`TokenPrice`] resolved from `pricing`
/// (architecture.md Section 6.1 / 8.2).
///
/// For each instance this calls [`ChatProvider::list_models`] (which may hit the
/// network) and, for every returned model, records the instance's
/// `capabilities(model)` and `pricing.price_for(kind, model)`. To resolve the
/// price kind, the instance id is matched back to its row in `configs`; any
/// instance without a matching config row is skipped (it cannot be priced or
/// attributed to a kind).
///
/// Because it drives the trait method, tests inject a fake [`ChatProvider`]
/// whose `list_models` returns a fixed list, so no live network is required.
pub async fn list_available_models(
    registry: &ProviderRegistry,
    configs: &[ProviderConfig],
    pricing: &PricingTable,
) -> Result<Vec<AvailableModel>, ProviderError> {
    let mut out = Vec::new();
    for cfg in configs {
        let Some(instance) = registry.get(&cfg.id) else {
            // No live instance for this config row (e.g. build was skipped);
            // nothing to enumerate.
            continue;
        };
        let models = instance.list_models().await?;
        for model in models {
            let capabilities = instance.capabilities(&model.id);
            let price = pricing.price_for(cfg.kind, &model.id);
            out.push(AvailableModel {
                provider_id: cfg.id.clone(),
                model: model.id,
                capabilities,
                price,
            });
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{ChatDelta, ChatProvider, ChatRequest, ChatResponse, ModelInfo};
    use async_trait::async_trait;
    use futures_util::stream::{self, BoxStream, StreamExt};
    use secrets::InMemorySecretStore;
    use serde_json::Value;
    use std::sync::Arc;

    /// A fake provider with an injected, fixed model list (no network). Used to
    /// prove `list_available_models` assembles rows from the trait method.
    struct FakeProvider {
        id: String,
        models: Vec<ModelInfo>,
        caps: Capabilities,
    }

    #[async_trait]
    impl ChatProvider for FakeProvider {
        fn id(&self) -> &str {
            &self.id
        }
        fn capabilities(&self, _model: &str) -> Capabilities {
            self.caps
        }
        async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
            Ok(self.models.clone())
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

    fn config(id: &str, kind: ProviderKind) -> ProviderConfig {
        ProviderConfig {
            id: id.to_string(),
            kind,
            base_url: None,
            api_key_ref: None,
            extra: Value::Null,
        }
    }

    #[test]
    fn builtin_registry_registers_all_seven_kinds() {
        let registry = builtin_registry();
        for kind in [
            ProviderKind::OpenAI,
            ProviderKind::LmStudio,
            ProviderKind::GenericOpenAI,
            ProviderKind::Azure,
            ProviderKind::Anthropic,
            ProviderKind::Gemini,
            ProviderKind::Bedrock,
        ] {
            assert!(
                registry.has_factory(kind),
                "built-in registry must register a factory for {kind:?}"
            );
        }
    }

    #[test]
    fn bundled_defaults_price_cloud_and_zero_for_local() {
        let pricing = PricingTable::bundled_defaults();
        // A cloud kind has a nonzero seeded default.
        let openai = pricing.price_for(ProviderKind::OpenAI, "gpt-4o");
        assert!(openai.input_per_mtok > 0.0 && openai.output_per_mtok > 0.0);
        // Local providers default to zero (Section 6.2).
        assert_eq!(
            pricing.price_for(ProviderKind::LmStudio, "local-model"),
            TokenPrice::ZERO
        );
    }

    #[test]
    fn per_model_override_wins_over_kind_default() {
        let mut pricing = PricingTable::bundled_defaults();
        pricing.set_model(
            ProviderKind::OpenAI,
            "gpt-4o-mini",
            TokenPrice::new(0.15, 0.6),
        );
        assert_eq!(
            pricing.price_for(ProviderKind::OpenAI, "gpt-4o-mini"),
            TokenPrice::new(0.15, 0.6)
        );
        // A different model still resolves to the kind default.
        assert_ne!(
            pricing.price_for(ProviderKind::OpenAI, "gpt-4o"),
            TokenPrice::new(0.15, 0.6)
        );
    }

    #[tokio::test]
    async fn list_available_models_attaches_capabilities_and_price() {
        // Register the fake instances directly on a registry so no network or
        // real adapter is involved (the injected `list_models` returns fixed
        // lists).
        let mut registry = builtin_registry();

        let cloud_caps = Capabilities {
            streaming: true,
            tools: true,
            vision: true,
            json_mode: true,
            max_context: Some(128_000),
        };
        registry.insert_instance(Arc::new(FakeProvider {
            id: "openai".to_string(),
            models: vec![ModelInfo::new("gpt-4o"), ModelInfo::new("gpt-4o-mini")],
            caps: cloud_caps,
        }));
        registry.insert_instance(Arc::new(FakeProvider {
            id: "local".to_string(),
            models: vec![ModelInfo::new("llama-3.1-8b")],
            caps: Capabilities::default(),
        }));

        let configs = vec![
            config("openai", ProviderKind::OpenAI),
            config("local", ProviderKind::LmStudio),
        ];
        let pricing = PricingTable::bundled_defaults();

        let mut models = list_available_models(&registry, &configs, &pricing)
            .await
            .unwrap();
        models.sort_by(|a, b| {
            (a.provider_id.clone(), a.model.clone()).cmp(&(b.provider_id.clone(), b.model.clone()))
        });

        assert_eq!(models.len(), 3);

        // The local provider's model is zero-priced (Section 6.2).
        let local = models.iter().find(|m| m.provider_id == "local").unwrap();
        assert_eq!(local.model, "llama-3.1-8b");
        assert_eq!(local.price, TokenPrice::ZERO);

        // The cloud provider's models carry capabilities and a nonzero price.
        let cloud: Vec<_> = models
            .iter()
            .filter(|m| m.provider_id == "openai")
            .collect();
        assert_eq!(cloud.len(), 2);
        assert!(cloud[0].capabilities.tools);
        assert!(cloud[0].price.input_per_mtok > 0.0);

        // DISPLAY-SAFE: the serialized rows carry only provider/model/caps/price
        // labels, never secret material.
        let json = serde_json::to_string(&models).unwrap();
        assert!(json.contains("\"providerId\""));
        assert!(json.contains("\"capabilities\""));
        assert!(json.contains("\"price\""));
        assert!(!json.contains("apiKey"));
        assert!(!json.contains("secret"));
    }

    #[tokio::test]
    async fn instances_without_a_config_row_are_skipped() {
        let mut registry = builtin_registry();
        registry.insert_instance(Arc::new(FakeProvider {
            id: "ghost".to_string(),
            models: vec![ModelInfo::new("m")],
            caps: Capabilities::default(),
        }));
        // No config row references "ghost", so nothing is listed.
        let out = list_available_models(&registry, &[], &PricingTable::new())
            .await
            .unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn build_registry_from_config_rows_registers_instances() {
        // Local LM Studio needs no key, so this builds offline-style without a
        // network call (build != list_models). Proves build_registry wires the
        // instance up under its id.
        let store = InMemorySecretStore::new();
        let configs = [config("local", ProviderKind::LmStudio)];
        let registry = build_registry(configs.iter(), &store).unwrap();
        assert!(registry.get("local").is_some());
    }
}
