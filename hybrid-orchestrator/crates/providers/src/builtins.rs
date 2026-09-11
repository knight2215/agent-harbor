//! Built-in provider registry wiring and `list_available_models` (architecture.md
//! Section 4.5 registry + Section 6.1 / 6.2 / 8.2 model selector data).
//!
//! [`builtin_registry`] constructs a [`ProviderRegistry`] with all nine
//! built-in [`ProviderFactory`]s registered, one per [`ProviderKind`] (Section
//! 4.5: "built-in factories are registered at startup"). [`build_registry`]
//! layers on the user-configured provider instances from persisted
//! [`ProviderConfig`] rows.
//!
//! [`list_available_models`] is the data source the Phase 5 model selector
//! (Section 8.2) and Phase 4 routing (Section 6.1) consume: for every configured
//! provider instance it lists the provider's models and attaches per-model
//! [`Capabilities`] and an optional [`TokenPrice`] resolved from a
//! [`PricingTable`]. Local providers (LM Studio, Ollama, the embedded engine)
//! default to zero price (Section 6.2). Because [`ChatProvider::list_models`]
//! may hit the network, the function drives the trait method, so tests inject a
//! fake provider whose `list_models` returns a fixed list (no live network).
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
    AnthropicFactory, AzureOpenAiFactory, BedrockFactory, EmbeddedFactory, GeminiFactory,
    GenericOpenAiFactory, LmStudioFactory, OllamaFactory, OpenAiFactory,
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
    /// A zero-cost price, used for local providers (LM Studio, Ollama, the
    /// embedded engine) and as the default for unpriced entries (Section 6.2).
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
        // Studio, Ollama) stay at zero. GenericOpenAI is left at zero because
        // its endpoint (and pricing) is user-defined.
        table.set_kind(ProviderKind::OpenAI, TokenPrice::new(2.5, 10.0));
        table.set_kind(ProviderKind::Anthropic, TokenPrice::new(3.0, 15.0));
        table.set_kind(ProviderKind::Bedrock, TokenPrice::new(3.0, 15.0));
        table.set_kind(ProviderKind::Gemini, TokenPrice::new(1.25, 5.0));
        table.set_kind(ProviderKind::Azure, TokenPrice::new(2.5, 10.0));
        table.set_kind(ProviderKind::LmStudio, TokenPrice::ZERO);
        table.set_kind(ProviderKind::GenericOpenAI, TokenPrice::ZERO);
        table.set_kind(ProviderKind::Ollama, TokenPrice::ZERO);
        // The embedded engine runs in-process; it is always zero-cost.
        table.set_kind(ProviderKind::Embedded, TokenPrice::ZERO);
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
    /// A coarse per-model quality tier in `0.0..=1.0` resolved from a generic,
    /// user-overridable per-kind/per-family map ([`quality_for`]). Routing's
    /// `AutoDefaultPolicy` blends it with the capability/context proxy so
    /// Auto / Prefer-Quality can pick a stronger model for hard tasks and a
    /// cheaper/faster one for simple tasks (Section 6.2). Serialized as the
    /// camelCase JSON key `quality`.
    #[serde(default)]
    pub quality: f64,
}

/// The neutral default quality tier for a model with no matching family/kind
/// rule. A middle value so an unknown model neither out-ranks a known pro-tier
/// model nor is dismissed below a known low tier (architecture.md Section 6.2).
pub const DEFAULT_QUALITY: f64 = 0.5;

/// Resolve a coarse per-model quality tier in `0.0..=1.0` from a small, generic,
/// provider-agnostic map keyed by [`ProviderKind`] + model-id substrings
/// (architecture.md Section 6.2 quality signal). This is a REPRESENTATIVE,
/// user-overridable default, NOT an authoritative vendor ranking; it exists so
/// routing can differentiate otherwise identically-capable models when picking
/// for task complexity or a Prefer-Quality hint.
///
/// The tiers are intentionally family-based (matched on lowercased id
/// substrings), so they generalize across providers rather than hardcoding one
/// vendor:
///   - `pro` / `opus` / `-4` / `4o` / `ultra` families read as top tier (~0.9);
///   - `flash-lite` / `mini` / `nano` / `haiku` / `small` / an anchored `8b`
///     (`:8b` or `-8b`) read as a lean tier (~0.4);
///   - `flash` / `sonnet` / `turbo` / `medium` read as a mid tier (~0.6);
///   - local kinds (LM Studio, Ollama, GenericOpenAI loopback, the embedded
///     engine) default to a middle tier (~0.5) because a locally-run model's
///     strength varies with the user's hardware and chosen weights;
///   - everything else falls back to [`DEFAULT_QUALITY`].
///
/// Order matters: the more specific `flash-lite` / `mini` rule is checked before
/// the broader `flash` rule so a `*-flash-lite` model does not read as mid tier.
pub fn quality_for(kind: ProviderKind, model: &str) -> f64 {
    let id = model.to_ascii_lowercase();
    // Lean / small families first (most specific), so a `flash-lite` is not
    // captured by the broader `flash` rule below. The `8b` size marker is
    // anchored to `:8b` / `-8b` so it matches ids like `qwen3:8b` or
    // `llama-3.1-8b` without a false positive on an unrelated id such as
    // `model-128b`.
    if id.contains("flash-lite")
        || id.contains("mini")
        || id.contains("nano")
        || id.contains("haiku")
        || id.contains("-small")
        || id.contains(":8b")
        || id.contains("-8b")
    {
        return 0.4;
    }
    // Top-tier families.
    if id.contains("pro")
        || id.contains("opus")
        || id.contains("ultra")
        || id.contains("4o")
        || id.contains("gpt-4")
    {
        return 0.9;
    }
    // Mid-tier families.
    if id.contains("flash")
        || id.contains("sonnet")
        || id.contains("turbo")
        || id.contains("medium")
    {
        return 0.6;
    }
    // Local kinds default to a middle tier; their strength depends on the
    // user's hardware and chosen weights rather than a fixed vendor tier.
    match kind {
        ProviderKind::LmStudio
        | ProviderKind::Ollama
        | ProviderKind::GenericOpenAI
        | ProviderKind::Embedded => 0.5,
        _ => DEFAULT_QUALITY,
    }
}

/// A single provider instance that could not be enumerated, surfaced to the UI
/// so a misconfigured or unreachable provider is diagnosable instead of silently
/// contributing nothing (architecture.md Section 8.2 model selector diagnostics).
///
/// DISPLAY-SAFE: this crosses the Tauri IPC boundary. It carries only the
/// configured provider instance id and the [`ProviderError`]'s Display string.
/// Every [`ProviderError`] variant (Transport/HttpStatus/Decode/Auth/Other) is
/// display-safe and never carries resolved secret or key material (Section 9.1 /
/// 9.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderEnumerationError {
    /// The configured provider instance id (`ProviderConfig::id`) that failed.
    pub provider_id: String,
    /// The display-safe error message (`ProviderError`'s Display), never secret.
    pub message: String,
}

/// The combined result of [`list_available_models`]: the successfully enumerated
/// models plus a display-safe list of per-provider enumeration errors
/// (architecture.md Section 8.2). Enumeration is fault-tolerant, so both vecs can
/// be non-empty at once: a single failing provider populates `errors` while the
/// healthy providers still populate `models`.
///
/// DISPLAY-SAFE: this is the exact shape that crosses the Tauri IPC boundary; it
/// carries no secret material (Section 9.1 / 9.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AvailableModelsResult {
    /// Every selectable model across all healthy provider instances.
    pub models: Vec<AvailableModel>,
    /// Per-provider enumeration failures (display-safe), empty when all healthy.
    pub errors: Vec<ProviderEnumerationError>,
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
    registry.register_factory(Box::new(OllamaFactory));
    registry.register_factory(Box::new(EmbeddedFactory));
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
/// Enumeration is fault-tolerant: an instance whose `list_models` errors (for
/// example an unreachable or misconfigured local endpoint) is skipped rather
/// than aborting the whole call, so one offline provider degrades gracefully
/// and the remaining providers still populate the picker. Instead of swallowing
/// the failure to stderr, the failing provider's id and display-safe error are
/// collected into [`AvailableModelsResult::errors`] so the UI can tell the user
/// why a provider contributed nothing.
///
/// A configured row whose instance was never built (its factory build failed or
/// was skipped, so `registry.get(&cfg.id)` is `None`) is likewise recorded as an
/// [`AvailableModelsResult::errors`] entry rather than silently dropped. A silent
/// drop was a prime clean-empty cause: the row contributed neither a model nor an
/// error, so the picker showed "No models available yet" with nothing to explain
/// why. The recorded message is a static, display-safe string (no secret
/// material).
///
/// This is the thin, backwards-compatible entry point: it delegates to
/// [`list_available_models_with_build_errors`] with an EMPTY build-error map, so
/// every existing caller (and the external
/// `crates/providers/tests/demo_provider_extension.rs`) keeps its 3-argument
/// signature and its behavior (an unbuilt row falls back to the static generic
/// message). Callers that captured the real per-row build error at build time
/// (the tauri-app diagnostics and picker) should call
/// [`list_available_models_with_build_errors`] instead so the REAL cause is
/// surfaced.
///
/// Because it drives the trait method, tests inject a fake [`ChatProvider`]
/// whose `list_models` returns a fixed list, so no live network is required.
pub async fn list_available_models(
    registry: &ProviderRegistry,
    configs: &[ProviderConfig],
    pricing: &PricingTable,
) -> Result<AvailableModelsResult, ProviderError> {
    list_available_models_with_build_errors(registry, configs, pricing, &BTreeMap::new()).await
}

/// The full enumeration, parameterised by a map of per-row build errors captured
/// by the caller when it built the registry (keyed by `ProviderConfig::id`, value
/// = the display-safe `ProviderError` Display string).
///
/// It behaves exactly like [`list_available_models`] EXCEPT for the unbuilt-row
/// branch: when `registry.get(&cfg.id)` is `None`, it prefers a real captured
/// build error from `build_errors` over the static generic message. That is the
/// fix for the "provider is configured but no instance was built" diagnosis being
/// unhelpfully generic: the tauri-app callers used to discard the
/// `Err(ProviderError)` from `build_from_config` (via `let _ =`), so the only
/// thing left to report was the generic string. By threading the captured error
/// through here, a Gemini/generic build failure surfaces its REAL cause (a bad
/// key ref, a missing base_url, an auth resolve failure) in the picker's
/// enumeration errors and the diagnostics panel.
///
/// DISPLAY-SAFE: `build_errors` values are `ProviderError` Display strings, which
/// carry only a reason (never resolved key material); this function copies them
/// verbatim into [`ProviderEnumerationError::message`].
pub async fn list_available_models_with_build_errors(
    registry: &ProviderRegistry,
    configs: &[ProviderConfig],
    pricing: &PricingTable,
    build_errors: &BTreeMap<String, String>,
) -> Result<AvailableModelsResult, ProviderError> {
    let mut models_out = Vec::new();
    let mut errors_out = Vec::new();
    for cfg in configs {
        let Some(instance) = registry.get(&cfg.id) else {
            // No live instance for this config row: its factory build failed or
            // was skipped. Prefer the REAL build error the caller captured for
            // this row (the discarded `Err(ProviderError)` from
            // `build_from_config`), falling back to a static, display-safe
            // generic string only when the caller supplied none. Either way the
            // row is recorded rather than silently dropped, so a
            // configured-but-unbuilt provider is diagnosable instead of an
            // invisible clean-empty cause, and neither message carries secret
            // material.
            let message = build_errors.get(&cfg.id).cloned().unwrap_or_else(|| {
                "provider is configured but no instance was built \
                 (build failed or was skipped); it cannot be enumerated"
                    .to_string()
            });
            errors_out.push(ProviderEnumerationError {
                provider_id: cfg.id.clone(),
                message,
            });
            continue;
        };
        let models = match instance.list_models().await {
            Ok(models) => models,
            Err(err) => {
                // This provider could not be enumerated (e.g. an unreachable
                // local endpoint, a bad key, or a wrong URL); skip it so the
                // rest of the picker still populates instead of blanking the
                // entire list, and record the failure so the UI can surface why
                // this provider contributed nothing. DISPLAY-SAFE: only the
                // config id and the provider error's Display string are kept,
                // never any resolved secret or key material (every
                // ProviderError variant is display-safe).
                errors_out.push(ProviderEnumerationError {
                    provider_id: cfg.id.clone(),
                    message: err.to_string(),
                });
                continue;
            }
        };
        for model in models {
            // Prefer the per-model capability hint the adapter derived from the
            // provider's own model-listing metadata (e.g. Gemini's
            // supportedGenerationMethods + inputTokenLimit) over the coarse,
            // model-string-based `capabilities()` stamp, so a model's advertised
            // capabilities reflect its ACTUAL metadata (Section 4.4 / 6.1).
            let capabilities = model
                .capabilities
                .unwrap_or_else(|| instance.capabilities(&model.id));
            let price = pricing.price_for(cfg.kind, &model.id);
            let quality = quality_for(cfg.kind, &model.id);
            models_out.push(AvailableModel {
                provider_id: cfg.id.clone(),
                model: model.id,
                capabilities,
                price,
                quality,
            });
        }
    }
    Ok(AvailableModelsResult {
        models: models_out,
        errors: errors_out,
    })
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
        /// When true, `list_models` returns an error instead of the fixed list,
        /// modelling an unreachable/misconfigured endpoint so the softened
        /// enumeration path (skip-on-error) can be exercised.
        list_models_errors: bool,
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
            if self.list_models_errors {
                return Err(ProviderError::Transport("endpoint unreachable".to_string()));
            }
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
    fn builtin_registry_registers_all_nine_kinds() {
        let registry = builtin_registry();
        for kind in [
            ProviderKind::OpenAI,
            ProviderKind::LmStudio,
            ProviderKind::GenericOpenAI,
            ProviderKind::Azure,
            ProviderKind::Anthropic,
            ProviderKind::Gemini,
            ProviderKind::Bedrock,
            ProviderKind::Ollama,
            ProviderKind::Embedded,
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
            list_models_errors: false,
        }));
        registry.insert_instance(Arc::new(FakeProvider {
            id: "local".to_string(),
            models: vec![ModelInfo::new("llama-3.1-8b")],
            caps: Capabilities::default(),
            list_models_errors: false,
        }));

        let configs = vec![
            config("openai", ProviderKind::OpenAI),
            config("local", ProviderKind::LmStudio),
        ];
        let pricing = PricingTable::bundled_defaults();

        let result = list_available_models(&registry, &configs, &pricing)
            .await
            .unwrap();
        // All providers enumerated successfully, so there are no errors.
        assert!(result.errors.is_empty());
        let mut models = result.models;
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

        // Every row carries a resolved quality tier in 0.0..=1.0 (the generic
        // per-kind/per-family map). The cloud gpt-4o row reads as top tier; the
        // local llama-3.1-8b reads as the lean 8b tier.
        for m in &models {
            assert!((0.0..=1.0).contains(&m.quality));
        }
        assert_eq!(quality_for(ProviderKind::OpenAI, "gpt-4o"), 0.9);
        assert_eq!(
            local.quality,
            quality_for(ProviderKind::LmStudio, "llama-3.1-8b")
        );

        // DISPLAY-SAFE: the serialized rows carry only provider/model/caps/price/
        // quality labels, never secret material. The quality tier serializes
        // under the camelCase JSON key `quality`.
        let json = serde_json::to_string(&models).unwrap();
        assert!(json.contains("\"providerId\""));
        assert!(json.contains("\"capabilities\""));
        assert!(json.contains("\"price\""));
        assert!(json.contains("\"quality\""));
        assert!(!json.contains("apiKey"));
        assert!(!json.contains("secret"));
    }

    #[test]
    fn quality_for_resolves_generic_family_tiers() {
        // Top tier: pro / opus / 4o / gpt-4 / ultra families.
        assert_eq!(quality_for(ProviderKind::Gemini, "gemini-2.5-pro"), 0.9);
        assert_eq!(quality_for(ProviderKind::OpenAI, "gpt-4o"), 0.9);
        assert_eq!(quality_for(ProviderKind::Anthropic, "claude-3-opus"), 0.9);
        // Lean tier is checked BEFORE the broader flash rule so flash-lite and
        // mini/nano read lean, not mid.
        assert_eq!(
            quality_for(ProviderKind::Gemini, "gemini-2.5-flash-lite"),
            0.4
        );
        assert_eq!(quality_for(ProviderKind::OpenAI, "gpt-4o-mini"), 0.4);
        // The `8b` size marker is anchored to `:8b` / `-8b`, so the real local
        // ids the app enumerates read lean...
        assert_eq!(quality_for(ProviderKind::Ollama, "qwen3:8b"), 0.4);
        assert_eq!(quality_for(ProviderKind::Ollama, "llama-3.1-8b"), 0.4);
        // ...but an unrelated id that merely CONTAINS `8b` as a bare substring
        // (e.g. a 128b model) is NOT captured by the lean rule.
        assert_eq!(quality_for(ProviderKind::Ollama, "model-128b"), 0.5);
        // Mid tier: flash / sonnet / turbo.
        assert_eq!(quality_for(ProviderKind::Gemini, "gemini-2.5-flash"), 0.6);
        assert_eq!(
            quality_for(ProviderKind::Anthropic, "claude-3-5-sonnet"),
            0.6
        );
        // Local kind with no family match falls to the middle local tier.
        assert_eq!(quality_for(ProviderKind::LmStudio, "some-local-model"), 0.5);
        // Unknown cloud kind/model falls to the neutral default.
        assert_eq!(
            quality_for(ProviderKind::Bedrock, "mystery"),
            DEFAULT_QUALITY
        );
    }

    #[tokio::test]
    async fn instances_without_a_config_row_are_skipped() {
        let mut registry = builtin_registry();
        registry.insert_instance(Arc::new(FakeProvider {
            id: "ghost".to_string(),
            models: vec![ModelInfo::new("m")],
            caps: Capabilities::default(),
            list_models_errors: false,
        }));
        // No config row references "ghost", so nothing is listed.
        let result = list_available_models(&registry, &[], &PricingTable::new())
            .await
            .unwrap();
        assert!(result.models.is_empty());
        assert!(result.errors.is_empty());
    }

    /// A single provider whose `list_models` errors (an unreachable/offline
    /// local endpoint) is skipped rather than aborting the whole enumeration:
    /// `list_available_models` still returns the healthy provider's models and
    /// does not error, and captures the failing provider's id and a display-safe
    /// message in the errors vec. This proves the softened, fault-tolerant loop.
    #[tokio::test]
    async fn enumeration_skips_a_provider_that_errors_and_returns_the_rest() {
        let mut registry = builtin_registry();

        // A healthy provider that enumerates fine.
        registry.insert_instance(Arc::new(FakeProvider {
            id: "healthy".to_string(),
            models: vec![ModelInfo::new("gpt-4o")],
            caps: Capabilities::default(),
            list_models_errors: false,
        }));
        // An offline local endpoint whose enumeration fails.
        registry.insert_instance(Arc::new(FakeProvider {
            id: "offline-local".to_string(),
            models: vec![ModelInfo::new("llama-3.1-8b")],
            caps: Capabilities::default(),
            list_models_errors: true,
        }));

        let configs = vec![
            config("healthy", ProviderKind::OpenAI),
            config("offline-local", ProviderKind::LmStudio),
        ];

        let result = list_available_models(&registry, &configs, &PricingTable::new())
            .await
            .expect("one failing provider must not abort the whole enumeration");

        // Only the healthy provider's model is returned; the failing one is
        // skipped from the models list.
        assert_eq!(result.models.len(), 1);
        assert_eq!(result.models[0].provider_id, "healthy");
        assert_eq!(result.models[0].model, "gpt-4o");
        assert!(!result
            .models
            .iter()
            .any(|m| m.provider_id == "offline-local"));

        // The failing provider is captured as a display-safe enumeration error
        // with a non-empty message and its config id.
        assert_eq!(result.errors.len(), 1);
        let err = &result.errors[0];
        assert_eq!(err.provider_id, "offline-local");
        assert!(!err.message.is_empty());

        // DISPLAY-SAFE: the serialized error uses camelCase and never carries
        // secret/key material.
        let json = serde_json::to_string(&result.errors).unwrap();
        assert!(json.contains("\"providerId\""));
        assert!(json.contains("\"message\""));
        assert!(!json.contains("apiKey"));
        assert!(!json.contains("secret"));
    }

    /// The mirror image of `instances_without_a_config_row_are_skipped`: a
    /// configured row that references an id with NO built instance in the
    /// registry (its build failed or was skipped) is no longer silently dropped.
    /// It now yields exactly one display-safe `ProviderEnumerationError` carrying
    /// that provider id and a non-empty message, while `models` stays empty. This
    /// pins the invisible-skip fix: a configured-but-unbuilt provider is
    /// diagnosable instead of an invisible clean-empty cause.
    #[tokio::test]
    async fn a_config_row_without_a_built_instance_is_recorded_as_an_error() {
        // An empty registry has no instances, so the configured row cannot be
        // resolved to a built instance.
        let registry = builtin_registry();
        let configs = vec![config("unbuilt", ProviderKind::Ollama)];

        let result = list_available_models(&registry, &configs, &PricingTable::new())
            .await
            .expect("an unbuilt configured row must not abort the whole enumeration");

        // No model is produced for the unbuilt row.
        assert!(result.models.is_empty());
        // Exactly one display-safe error is recorded for that provider id.
        assert_eq!(result.errors.len(), 1);
        let err = &result.errors[0];
        assert_eq!(err.provider_id, "unbuilt");
        assert!(!err.message.is_empty());

        // DISPLAY-SAFE: the serialized error uses camelCase and never carries
        // secret/key material.
        let json = serde_json::to_string(&result.errors).unwrap();
        assert!(json.contains("\"providerId\""));
        assert!(json.contains("\"message\""));
        assert!(!json.contains("apiKey"));
        assert!(!json.contains("secret"));
    }

    /// `list_available_models_with_build_errors` prefers a caller-supplied real
    /// build error over the static generic "no instance was built" message for an
    /// unbuilt row, and the 3-arg `list_available_models` delegation (empty map)
    /// still yields the generic message. This pins the FEAT-001 fix: the tauri-app
    /// callers now capture the discarded `Err(ProviderError)` from
    /// `build_from_config` and thread it through here so the picker/diagnostics
    /// surface the REAL cause.
    #[tokio::test]
    async fn build_errors_are_preferred_over_the_generic_message() {
        // An empty registry has no instances, so the configured row is unbuilt.
        let registry = builtin_registry();
        let configs = vec![config("gemini-cloud", ProviderKind::Gemini)];
        let pricing = PricingTable::new();

        // With a captured build error for the row, the recorded enumeration error
        // is EXACTLY that real message (not the generic string).
        let real = "auth error: no secret found for handle 'gemini-cloud'".to_string();
        let mut build_errors = BTreeMap::new();
        build_errors.insert("gemini-cloud".to_string(), real.clone());
        let result =
            list_available_models_with_build_errors(&registry, &configs, &pricing, &build_errors)
                .await
                .unwrap();
        assert!(result.models.is_empty());
        assert_eq!(result.errors.len(), 1);
        assert_eq!(result.errors[0].provider_id, "gemini-cloud");
        assert_eq!(result.errors[0].message, real);
        // The generic fallback string is NOT used when a real error is supplied.
        assert!(!result.errors[0].message.contains("no instance was built"));

        // The empty-map delegation path (what the 3-arg fn uses) still falls back
        // to the static generic message, preserving the pre-fix behavior for
        // callers that captured no build error.
        let generic = list_available_models(&registry, &configs, &pricing)
            .await
            .unwrap();
        assert_eq!(generic.errors.len(), 1);
        assert!(generic.errors[0].message.contains("no instance was built"));
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
