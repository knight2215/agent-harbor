//! Extensibility validation for the routing-policy seam (architecture.md
//! Section 10, tasks.md P6.6). This is a TEST-ONLY artifact: it ships nothing to
//! production.
//!
//! It proves the "add a routing policy without touching the core" claim from
//! Section 10: a brand-new [`RoutingPolicy`], defined entirely here in the test,
//! can be registered on a [`PolicyRegistry`] via its public
//! [`PolicyRegistry::register`] seam, selected active via
//! [`PolicyRegistry::set_active`], and driven by [`PolicyRegistry::resolve`].
//!
//! Two behaviors are asserted. With no manual override, `resolve` delegates to
//! the demo policy's decision. With a manual override, the Section 6.3
//! manual-override precedence wrapper ([`ManualOverrideResolver`]) still
//! short-circuits to the manual route. The second case is load-bearing: it
//! proves the registry keeps wrapping the active automatic policy in the
//! manual-override precedence unchanged, i.e. the engine wrapper/pipeline is
//! untouched by swapping in a custom policy.
//!
//! ZERO-EDIT PROOF: this file is the ONLY thing added for the demo policy. No
//! file under `routing/src` or `orchestrator-core` changes to make the demo
//! policy register, activate, or resolve. The demo policy consumes the
//! `providers::AvailableModel` candidates the provider seam (P6.5) surfaces
//! entirely unmodified, which also proves routing consumes a demo
//! `AvailableModel` without any routing edit.

use std::collections::BTreeSet;
use std::sync::Arc;

use async_trait::async_trait;

use providers::{AvailableModel, Capabilities, ChatMessage, MessageRole, TokenPrice};
use routing::{
    ManualRoute, PolicyRegistry, RouteSource, RoutingDecision, RoutingError, RoutingPolicy,
    RoutingRequest,
};

/// The stable id of the demo policy.
const DEMO_POLICY_ID: &str = "demoAlwaysFirst";

/// A demo "always first" [`RoutingPolicy`]: it ignores complexity, price, and
/// persona signals and simply picks the FIRST candidate in the enumerated
/// `available` list. Deliberately trivial so the test can assert an outcome
/// distinct from the built-in `autoDefault` policy, proving `resolve` delegates
/// to THIS policy's decision. A real custom policy only needs to implement the
/// AUTOMATIC decision; the manual-override precedence wraps it automatically.
#[derive(Debug)]
struct AlwaysFirstPolicy;

#[async_trait]
impl RoutingPolicy for AlwaysFirstPolicy {
    fn id(&self) -> &str {
        DEMO_POLICY_ID
    }

    async fn decide(&self, req: &RoutingRequest) -> Result<RoutingDecision, RoutingError> {
        let first = req.available.first().ok_or_else(|| {
            RoutingError::NoCandidate("demo policy needs at least one candidate".to_string())
        })?;
        Ok(RoutingDecision {
            provider_id: first.provider_id.clone(),
            model: first.model.clone(),
            rationale: format!("demo always-first: {}/{}", first.provider_id, first.model),
            source: RouteSource::Automatic,
        })
    }
}

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

/// Candidate order matters for the demo policy: `cloud-first` is deliberately
/// FIRST so "always-first" selects it, whereas `autoDefault` at this low
/// complexity would prefer the cheaper local model. That divergence is what
/// proves `resolve` delegates to the demo policy rather than the built-in.
fn request() -> RoutingRequest {
    RoutingRequest {
        messages: vec![ChatMessage::text(MessageRole::User, "hi")],
        privacy_tags: Vec::new(),
        persona: None,
        routing_hint: None,
        manual_override: None,
        conversation_pref: None,
        available: vec![
            model("cloud-first", "gpt-4o", TokenPrice::new(2.5, 10.0)),
            model("local", "llama", TokenPrice::ZERO),
        ],
        local_provider_ids: ["local".to_string()].into_iter().collect::<BTreeSet<_>>(),
        budget: None,
    }
}

/// The demo policy registers and can be selected active through the public seam.
#[test]
fn demo_policy_registers_and_activates() {
    let mut registry = PolicyRegistry::new();
    assert!(!registry.contains(DEMO_POLICY_ID));
    registry.register(Arc::new(AlwaysFirstPolicy));
    assert!(registry.contains(DEMO_POLICY_ID));

    registry
        .set_active(DEMO_POLICY_ID)
        .expect("demo policy can be selected active");
    assert_eq!(registry.active_id(), DEMO_POLICY_ID);
}

/// (a) With no manual override, `resolve` delegates to the demo policy: it picks
/// the FIRST candidate (`cloud-first`), which the built-in `autoDefault` would
/// NOT do at this complexity, proving the active custom policy is what decided.
#[tokio::test]
async fn resolve_delegates_to_active_demo_policy_without_manual() {
    let mut registry = PolicyRegistry::new();
    registry.register(Arc::new(AlwaysFirstPolicy));
    registry.set_active(DEMO_POLICY_ID).unwrap();

    let req = request();
    let decision = registry.resolve(&req).await.unwrap();

    assert_eq!(decision.source, RouteSource::Automatic);
    assert_eq!(
        decision.provider_id, "cloud-first",
        "the active demo policy (always-first) must decide, not autoDefault"
    );
    assert!(decision.rationale.contains("demo always-first"));
}

/// (b) With a manual override, the manual-override precedence wrapper STILL
/// short-circuits to the manual route even though a CUSTOM policy is active,
/// proving the engine wrapper/pipeline is untouched by swapping the policy.
#[tokio::test]
async fn manual_override_still_wins_over_active_demo_policy() {
    let mut registry = PolicyRegistry::new();
    registry.register(Arc::new(AlwaysFirstPolicy));
    registry.set_active(DEMO_POLICY_ID).unwrap();

    let mut req = request();
    req.manual_override = Some(ManualRoute {
        provider_id: "local".to_string(),
        model: "llama".to_string(),
    });

    let decision = registry.resolve(&req).await.unwrap();
    // The demo policy would have picked `cloud-first`; the manual override wins.
    assert_eq!(decision.source, RouteSource::Manual);
    assert_eq!(decision.provider_id, "local");
    assert_eq!(decision.model, "llama");
}

/// The conversation pin also short-circuits the active custom policy (the middle
/// precedence tier), further proving the wrapper is intact around a custom
/// active policy.
#[tokio::test]
async fn conversation_pin_still_wins_over_active_demo_policy() {
    let mut registry = PolicyRegistry::new();
    registry.register(Arc::new(AlwaysFirstPolicy));
    registry.set_active(DEMO_POLICY_ID).unwrap();

    let mut req = request();
    req.conversation_pref = Some(ManualRoute {
        provider_id: "local".to_string(),
        model: "llama".to_string(),
    });

    let decision = registry.resolve(&req).await.unwrap();
    assert_eq!(decision.source, RouteSource::ConversationPin);
    assert_eq!(decision.provider_id, "local");
}
