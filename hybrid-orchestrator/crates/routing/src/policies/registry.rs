//! The routing policy registry (architecture.md Section 6.4).
//!
//! [`PolicyRegistry`] holds the set of registered automatic [`RoutingPolicy`]s
//! keyed by id and a selectable `active` id (the policy used for automatic
//! decisions). Its [`PolicyRegistry::resolve`] entry point wraps whichever
//! automatic policy is active in the Section 6.3 manual-override precedence
//! ([`crate::policies::manual_override::ManualOverrideResolver`]) and returns
//! the [`RoutingDecision`].
//!
//! ## Persisted active id
//!
//! The `active` id is the persisted selection surfaced in settings
//! (`AppConfig.active_routing_policy`, architecture.md Section 6.4 / 10.4). The
//! registry itself only holds the selectable id in memory; the Tauri layer /
//! pipeline is responsible for reading the persisted value into
//! [`PolicyRegistry::set_active`] at startup and writing it back when the user
//! changes it. Custom policies only need to implement the AUTOMATIC decision -
//! the manual-override precedence wraps whatever is active (Section 6.4).

use std::collections::HashMap;
use std::sync::Arc;

use crate::policies::auto_default::{AutoDefaultPolicy, AUTO_DEFAULT_ID};
use crate::policies::manual_override::ManualOverrideResolver;
use crate::policy::{RoutingDecision, RoutingError, RoutingPolicy, RoutingRequest};

/// Registration + swapping of automatic routing policies (architecture.md
/// Section 6.4).
#[derive(Clone)]
pub struct PolicyRegistry {
    policies: HashMap<String, Arc<dyn RoutingPolicy>>,
    /// Id of the automatic policy used for automatic decisions (persisted
    /// selection, Section 6.4).
    active: String,
}

impl PolicyRegistry {
    /// Construct a registry with the built-in [`AutoDefaultPolicy`] registered
    /// and set active (architecture.md Section 6.4: "built-in policies are
    /// registered at startup").
    pub fn new() -> Self {
        let mut policies: HashMap<String, Arc<dyn RoutingPolicy>> = HashMap::new();
        let auto = Arc::new(AutoDefaultPolicy::new());
        policies.insert(AUTO_DEFAULT_ID.to_string(), auto);
        PolicyRegistry {
            policies,
            active: AUTO_DEFAULT_ID.to_string(),
        }
    }

    /// Register (or replace) an automatic policy under its [`RoutingPolicy::id`].
    pub fn register(&mut self, policy: Arc<dyn RoutingPolicy>) {
        self.policies.insert(policy.id().to_string(), policy);
    }

    /// Select the active automatic policy by id. Errors if `id` is not
    /// registered (the persisted selection must name a known policy).
    pub fn set_active(&mut self, id: &str) -> Result<(), RoutingError> {
        if !self.policies.contains_key(id) {
            return Err(RoutingError::NoCandidate(format!(
                "no routing policy registered with id {id:?}"
            )));
        }
        self.active = id.to_string();
        Ok(())
    }

    /// The id of the currently-active automatic policy.
    pub fn active_id(&self) -> &str {
        &self.active
    }

    /// Whether a policy with `id` is registered.
    pub fn contains(&self, id: &str) -> bool {
        self.policies.contains_key(id)
    }

    /// Resolve a routing decision for `req`: wrap the currently-active automatic
    /// policy in the Section 6.3 manual-override precedence and delegate
    /// (architecture.md Section 6.4). A per-message override or per-conversation
    /// pin short-circuits the automatic policy; otherwise the active automatic
    /// policy decides.
    pub async fn resolve(&self, req: &RoutingRequest) -> Result<RoutingDecision, RoutingError> {
        // `active` is always a registered id (guarded by `set_active`).
        let automatic = self
            .policies
            .get(&self.active)
            .expect("active id is always a registered policy")
            .clone();
        let resolver = ManualOverrideResolver::new(automatic);
        resolver.decide(req).await
    }
}

impl Default for PolicyRegistry {
    fn default() -> Self {
        PolicyRegistry::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::{ManualRoute, RouteSource, RoutingRequest};
    use providers::{AvailableModel, Capabilities, ChatMessage, MessageRole, TokenPrice};

    fn model(provider_id: &str, model: &str, price: TokenPrice) -> AvailableModel {
        AvailableModel {
            provider_id: provider_id.to_string(),
            model: model.to_string(),
            capabilities: Capabilities {
                streaming: true,
                tools: true,
                vision: true,
                json_mode: true,
                max_context: Some(128_000),
            },
            price,
            quality: 0.5,
        }
    }

    fn request() -> RoutingRequest {
        RoutingRequest {
            messages: vec![ChatMessage::text(MessageRole::User, "hi")],
            privacy_tags: Vec::new(),
            persona: None,
            routing_hint: None,
            manual_override: None,
            conversation_pref: None,
            available: vec![
                model("openai", "gpt-4o", TokenPrice::new(2.5, 10.0)),
                model("lmstudio", "llama", TokenPrice::ZERO),
            ],
            // The local LM Studio row is provably local here.
            local_provider_ids: ["lmstudio".to_string()].into_iter().collect(),
            budget: None,
        }
    }

    #[test]
    fn default_active_is_auto_default() {
        let registry = PolicyRegistry::default();
        assert_eq!(registry.active_id(), AUTO_DEFAULT_ID);
        assert!(registry.contains(AUTO_DEFAULT_ID));
    }

    #[test]
    fn set_active_to_unknown_errors() {
        let mut registry = PolicyRegistry::new();
        let err = registry.set_active("nope").unwrap_err();
        assert!(matches!(err, RoutingError::NoCandidate(_)));
        // The active id is unchanged after a failed switch.
        assert_eq!(registry.active_id(), AUTO_DEFAULT_ID);
    }

    #[tokio::test]
    async fn resolve_honors_manual_override_over_active_automatic() {
        let registry = PolicyRegistry::new();
        let mut req = request();
        req.manual_override = Some(ManualRoute {
            provider_id: "openai".to_string(),
            model: "gpt-4o".to_string(),
        });
        // autoDefault at this (low) complexity would pick the local model, but
        // the manual override wraps the active policy and wins.
        let decision = registry.resolve(&req).await.unwrap();
        assert_eq!(decision.provider_id, "openai");
        assert_eq!(decision.source, RouteSource::Manual);
    }

    #[tokio::test]
    async fn resolve_delegates_to_active_automatic_without_manual() {
        let registry = PolicyRegistry::new();
        let req = request();
        let decision = registry.resolve(&req).await.unwrap();
        assert_eq!(decision.source, RouteSource::Automatic);
    }
}
