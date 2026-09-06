//! The manual-override precedence resolver (architecture.md Section 6.3).
//!
//! [`ManualOverrideResolver`] WRAPS whatever automatic [`RoutingPolicy`] is
//! active and applies the Section 6.3 precedence in front of it:
//!
//! 1. a per-message `manual_override` wins (`source = Manual`);
//! 2. else a per-conversation `conversation_pref` applies
//!    (`source = ConversationPin`);
//! 3. else it delegates to the wrapped automatic policy (`source = Automatic`).
//!
//! Manual routes STILL pass hard-constraint validation for safety: a manual
//! choice that would send `LocalOnly`/`Confidential` data to a non-local model
//! is rejected with [`RoutingError::ManualRouteRejected`], and a manual choice
//! that does not resolve to a known candidate (or lacks a required capability)
//! is rejected with [`RoutingError::ManualRouteUnavailable`] /
//! [`RoutingError::CapabilityUnsatisfiable`]. Aside from that gate, a manual
//! selection overrides all automatic scoring.

use std::sync::Arc;

use async_trait::async_trait;

use crate::policy::{
    AvailableModel, ManualRoute, RouteSource, RoutingDecision, RoutingError, RoutingPolicy,
    RoutingRequest,
};
use crate::signals::{is_local_price, required_capabilities, requires_local};

/// Wraps an active automatic [`RoutingPolicy`] and enforces the Section 6.3
/// manual-override precedence in front of it.
pub struct ManualOverrideResolver {
    automatic: Arc<dyn RoutingPolicy>,
}

impl ManualOverrideResolver {
    /// The stable id of the resolver.
    pub const ID: &'static str = "manualOverride";

    /// Wrap `automatic` (the currently-active automatic policy).
    pub fn new(automatic: Arc<dyn RoutingPolicy>) -> Self {
        ManualOverrideResolver { automatic }
    }

    /// Validate a manual `route` against the request's hard constraints and,
    /// when valid, build the [`RoutingDecision`] with the given `source` and
    /// `label` (used in the rationale). Returns an explanatory `Err` otherwise.
    fn resolve_manual(
        req: &RoutingRequest,
        route: &ManualRoute,
        source: RouteSource,
        label: &str,
    ) -> Result<RoutingDecision, RoutingError> {
        // The manual route must resolve to a known candidate so we can inspect
        // its capabilities/price.
        let candidate: Option<&AvailableModel> = req
            .available
            .iter()
            .find(|m| m.provider_id == route.provider_id && m.model == route.model);

        // Privacy gate (Section 6.3): under a LocalOnly/Confidential tag, the
        // manual route must resolve to a LOCAL candidate.
        if requires_local(&req.privacy_tags) {
            let is_local = candidate.is_some_and(|c| is_local_price(&c.price));
            if !is_local {
                return Err(RoutingError::ManualRouteRejected(format!(
                    "manual route {}/{} would send local-only data to the cloud",
                    route.provider_id, route.model
                )));
            }
        }

        // Capability gate (Section 4.4): if tools/vision are required, the
        // manual route must resolve to a candidate that supports them.
        let required = required_capabilities(req);
        match candidate {
            Some(c) => {
                if !required.satisfied_by(&c.capabilities) {
                    return Err(RoutingError::CapabilityUnsatisfiable(format!(
                        "manual route {}/{} lacks a required capability (tools={}, vision={})",
                        route.provider_id, route.model, required.tools, required.vision
                    )));
                }
            }
            None => {
                // No candidate row. If a privacy tag applied we already rejected
                // above; otherwise a manual route to a model that is not in the
                // available list cannot be validated for capabilities. Reject as
                // unavailable so the caller surfaces a clear error rather than
                // routing blind.
                return Err(RoutingError::ManualRouteUnavailable(format!(
                    "manual route {}/{} does not match any available model",
                    route.provider_id, route.model
                )));
            }
        }

        Ok(RoutingDecision {
            provider_id: route.provider_id.clone(),
            model: route.model.clone(),
            rationale: format!("{label}: {}/{}", route.provider_id, route.model),
            source,
        })
    }
}

#[async_trait]
impl RoutingPolicy for ManualOverrideResolver {
    fn id(&self) -> &str {
        Self::ID
    }

    async fn decide(&self, req: &RoutingRequest) -> Result<RoutingDecision, RoutingError> {
        // (1) Per-message manual override wins (Section 6.3).
        if let Some(route) = &req.manual_override {
            return Self::resolve_manual(req, route, RouteSource::Manual, "per-message override");
        }
        // (2) Per-conversation pin applies when there is no per-message override.
        if let Some(route) = &req.conversation_pref {
            return Self::resolve_manual(
                req,
                route,
                RouteSource::ConversationPin,
                "per-conversation pin",
            );
        }
        // (3) Otherwise delegate to the wrapped automatic policy.
        self.automatic.decide(req).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policies::auto_default::AutoDefaultPolicy;
    use crate::policy::{ManualRoute, PrivacyTag, RoutingRequest};
    use providers::{AvailableModel, Capabilities, ChatMessage, MessageRole, TokenPrice};

    fn caps() -> Capabilities {
        Capabilities {
            streaming: true,
            tools: true,
            vision: true,
            json_mode: true,
            max_context: Some(128_000),
        }
    }

    fn model(provider_id: &str, model: &str, price: TokenPrice) -> AvailableModel {
        AvailableModel {
            provider_id: provider_id.to_string(),
            model: model.to_string(),
            capabilities: caps(),
            price,
        }
    }

    fn resolver() -> ManualOverrideResolver {
        ManualOverrideResolver::new(Arc::new(AutoDefaultPolicy::new()))
    }

    fn request(available: Vec<AvailableModel>) -> RoutingRequest {
        RoutingRequest {
            messages: vec![ChatMessage::text(MessageRole::User, "hi")],
            privacy_tags: Vec::new(),
            persona: None,
            manual_override: None,
            conversation_pref: None,
            available,
            budget: None,
        }
    }

    #[tokio::test]
    async fn per_message_override_beats_pin_and_automatic() {
        let mut req = request(vec![
            model("openai", "gpt-4o", TokenPrice::new(2.5, 10.0)),
            model("lmstudio", "llama", TokenPrice::ZERO),
        ]);
        req.manual_override = Some(ManualRoute {
            provider_id: "openai".to_string(),
            model: "gpt-4o".to_string(),
        });
        req.conversation_pref = Some(ManualRoute {
            provider_id: "lmstudio".to_string(),
            model: "llama".to_string(),
        });
        let decision = resolver().decide(&req).await.unwrap();
        assert_eq!(decision.provider_id, "openai");
        assert_eq!(decision.source, RouteSource::Manual);
        assert!(decision.rationale.contains("per-message override"));
    }

    #[tokio::test]
    async fn pin_beats_automatic() {
        let mut req = request(vec![
            model("openai", "gpt-4o", TokenPrice::new(2.5, 10.0)),
            model("lmstudio", "llama", TokenPrice::ZERO),
        ]);
        req.conversation_pref = Some(ManualRoute {
            provider_id: "openai".to_string(),
            model: "gpt-4o".to_string(),
        });
        let decision = resolver().decide(&req).await.unwrap();
        assert_eq!(decision.provider_id, "openai");
        assert_eq!(decision.source, RouteSource::ConversationPin);
    }

    #[tokio::test]
    async fn no_manual_delegates_to_automatic() {
        let req = request(vec![
            model("openai", "gpt-4o", TokenPrice::new(2.5, 10.0)),
            model("lmstudio", "llama", TokenPrice::ZERO),
        ]);
        let decision = resolver().decide(&req).await.unwrap();
        assert_eq!(decision.source, RouteSource::Automatic);
    }

    #[tokio::test]
    async fn manual_route_violating_privacy_is_rejected() {
        let mut req = request(vec![
            model("openai", "gpt-4o", TokenPrice::new(2.5, 10.0)),
            model("lmstudio", "llama", TokenPrice::ZERO),
        ]);
        req.privacy_tags = vec![PrivacyTag::LocalOnly];
        req.manual_override = Some(ManualRoute {
            provider_id: "openai".to_string(),
            model: "gpt-4o".to_string(),
        });
        let err = resolver().decide(&req).await.unwrap_err();
        assert!(matches!(err, RoutingError::ManualRouteRejected(_)));
        assert!(err.to_string().contains("local-only"));
    }

    #[tokio::test]
    async fn valid_local_manual_route_under_local_only_is_accepted() {
        let mut req = request(vec![
            model("openai", "gpt-4o", TokenPrice::new(2.5, 10.0)),
            model("lmstudio", "llama", TokenPrice::ZERO),
        ]);
        req.privacy_tags = vec![PrivacyTag::LocalOnly];
        req.manual_override = Some(ManualRoute {
            provider_id: "lmstudio".to_string(),
            model: "llama".to_string(),
        });
        let decision = resolver().decide(&req).await.unwrap();
        assert_eq!(decision.provider_id, "lmstudio");
        assert_eq!(decision.source, RouteSource::Manual);
    }

    #[tokio::test]
    async fn manual_route_to_unknown_model_is_unavailable() {
        let mut req = request(vec![model("openai", "gpt-4o", TokenPrice::new(2.5, 10.0))]);
        req.manual_override = Some(ManualRoute {
            provider_id: "ghost".to_string(),
            model: "nope".to_string(),
        });
        let err = resolver().decide(&req).await.unwrap_err();
        assert!(matches!(err, RoutingError::ManualRouteUnavailable(_)));
    }
}
