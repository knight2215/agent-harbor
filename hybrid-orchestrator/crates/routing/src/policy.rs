//! The routing policy trait and its request/decision/error types
//! (architecture.md Section 6.1).
//!
//! A [`RoutingPolicy`] takes a [`RoutingRequest`] (context about the pending
//! message) and returns a [`RoutingDecision`] naming the provider/model that
//! should answer plus a human-readable rationale. The concrete policies live in
//! [`crate::policies`]; the shared signal helpers live in [`crate::signals`].
//!
//! REUSE, do NOT duplicate: the request/decision types build on the leaf
//! `domain` DTOs ([`domain::ManualRoute`], [`domain::PrivacyTag`],
//! [`domain::RouteSource`], [`domain::AgentPersona`]) and the `providers`
//! selector DTOs ([`providers::AvailableModel`], [`providers::ChatMessage`]).
//! Those crates remain the single source of truth; this module only re-exports
//! them for ergonomics.

use std::collections::BTreeSet;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

// Re-export the reused DTOs so P4.1's named types are all reachable directly
// from `routing` (single source of truth stays in `domain` / `providers`).
pub use domain::{ManualRoute, PrivacyTag, RouteSource};
pub use providers::AvailableModel;

/// Context about the pending message a [`RoutingPolicy`] scores
/// (architecture.md Section 6.1).
///
/// Not `Serialize`/`Deserialize`: it embeds live provider DTOs and is built
/// in-process by the pipeline from the session/persona/config, never sent over
/// IPC. It derives `Debug`/`Clone` so tests can `{:?}` and reuse it.
#[derive(Debug, Clone)]
pub struct RoutingRequest {
    /// The conversation so far (system/user/assistant/tool messages) that the
    /// complexity + capability signals inspect.
    pub messages: Vec<providers::ChatMessage>,
    /// Data-handling constraints on the conversation or message. `LocalOnly` /
    /// `Confidential` are hard local-only constraints (Section 6.2).
    pub privacy_tags: Vec<PrivacyTag>,
    /// The active persona, if any: supplies a `routing_hint` bias and a
    /// `default_route`.
    pub persona: Option<domain::AgentPersona>,
    /// Per-message manual override (Section 6.3, highest precedence).
    pub manual_override: Option<ManualRoute>,
    /// Per-conversation pin (Section 6.3, applies when no per-message override).
    pub conversation_pref: Option<ManualRoute>,
    /// The candidate provider/model list with capabilities + price
    /// (`providers::list_available_models`, Section 6.1 / 8.2).
    pub available: Vec<AvailableModel>,
    /// The provider instance ids that are PROVABLY local (architecture.md
    /// Section 6.2). The caller derives this from each candidate's concrete
    /// `ProviderKind` (LM Studio, and a loopback GenericOpenAI endpoint) when it
    /// enumerates `available`, so routing can decide the privacy hard constraint
    /// on provable locality rather than trusting a zero price. Locality is
    /// FAIL-CLOSED: an id absent from this set is treated as non-local under a
    /// `LocalOnly`/`Confidential` tag.
    pub local_provider_ids: BTreeSet<String>,
    /// Optional token/price budget signal (Section 6.1).
    pub budget: Option<CostBudget>,
}

/// The outcome of a routing decision (architecture.md Section 6.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutingDecision {
    /// The chosen provider instance id (`AvailableModel::provider_id`).
    pub provider_id: String,
    /// The chosen model id.
    pub model: String,
    /// Human-readable explanation shown in the UI ("why this model").
    pub rationale: String,
    /// How the route was decided (Manual | ConversationPin | Automatic).
    pub source: RouteSource,
}

impl RoutingDecision {
    /// Convert this decision into the persisted [`domain::RouteMetadata`] the
    /// pipeline attaches to the answered message (Section 7.1). The two types
    /// carry the same fields; this is the single conversion point so the
    /// pipeline does not hand-copy them.
    pub fn into_route_metadata(self) -> domain::RouteMetadata {
        domain::RouteMetadata {
            provider_id: self.provider_id,
            model: self.model,
            rationale: self.rationale,
            source: self.source,
        }
    }
}

/// A token/price budget signal biasing selection toward cheaper/local models
/// (architecture.md Section 6.1 / 6.2). Both fields are optional so an absent
/// budget is a valid "no budget" signal.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CostBudget {
    /// A soft ceiling on the estimated cost of answering this single message,
    /// in the same currency units as [`providers::TokenPrice`] (per 1M tokens).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_cost_per_message: Option<f64>,
    /// Remaining budget for the conversation/period, if tracked. A near-zero
    /// remaining budget biases hard toward zero-cost local models.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remaining_budget: Option<f64>,
}

/// Why a routing decision could not be produced (architecture.md Section 6.2 /
/// 6.3). Every variant carries a display-safe message.
#[derive(Debug, thiserror::Error)]
pub enum RoutingError {
    /// No candidate models were supplied (or all were filtered out for reasons
    /// other than privacy/capability, e.g. an empty `available` list).
    #[error("no candidate models available: {0}")]
    NoCandidate(String),
    /// A `LocalOnly`/`Confidential` tag applies but no local candidate can
    /// satisfy the request. Routing FAILS CLOSED here rather than falling back
    /// to the cloud (Section 6.2).
    #[error("privacy constraint cannot be satisfied locally: {0}")]
    PrivacyConstraintUnsatisfiable(String),
    /// No surviving candidate has a required capability (e.g. the request needs
    /// tools but every candidate is tool-incapable, Section 4.4).
    #[error("required capability cannot be satisfied: {0}")]
    CapabilityUnsatisfiable(String),
    /// A manual route was rejected by hard-constraint validation (e.g. it would
    /// send local-only data to the cloud, Section 6.3).
    #[error("manual route rejected: {0}")]
    ManualRouteRejected(String),
    /// A manual route does not resolve to any known [`AvailableModel`].
    #[error("manual route unavailable: {0}")]
    ManualRouteUnavailable(String),
}

/// A pluggable routing policy (architecture.md Section 6.1).
///
/// `id` is a stable identifier used by the [`crate::policies::registry`]; the
/// built-in automatic policy is [`crate::policies::auto_default::AutoDefaultPolicy`]
/// (`"autoDefault"`).
#[async_trait]
pub trait RoutingPolicy: Send + Sync {
    /// Stable policy id, e.g. `"autoDefault"`.
    fn id(&self) -> &str;

    /// Decide which provider/model should answer `req`, or fail with a
    /// display-safe [`RoutingError`].
    async fn decide(&self, req: &RoutingRequest) -> Result<RoutingDecision, RoutingError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decision_converts_to_route_metadata() {
        let decision = RoutingDecision {
            provider_id: "openai".to_string(),
            model: "gpt-4o".to_string(),
            rationale: "test".to_string(),
            source: RouteSource::Automatic,
        };
        let meta = decision.clone().into_route_metadata();
        assert_eq!(meta.provider_id, decision.provider_id);
        assert_eq!(meta.model, decision.model);
        assert_eq!(meta.rationale, decision.rationale);
        assert_eq!(meta.source, RouteSource::Automatic);
    }

    #[test]
    fn cost_budget_omits_absent_fields() {
        let budget = CostBudget::default();
        let json = serde_json::to_string(&budget).unwrap();
        assert_eq!(json, "{}");
        let with = CostBudget {
            max_cost_per_message: Some(0.5),
            remaining_budget: None,
        };
        let json = serde_json::to_string(&with).unwrap();
        assert!(json.contains("\"maxCostPerMessage\""));
        assert!(!json.contains("remainingBudget"));
    }

    #[test]
    fn decision_serializes_camel_case() {
        let decision = RoutingDecision {
            provider_id: "local".to_string(),
            model: "llama".to_string(),
            rationale: "why".to_string(),
            source: RouteSource::Manual,
        };
        let json = serde_json::to_string(&decision).unwrap();
        assert!(json.contains("\"providerId\""));
        assert!(json.contains("\"source\":\"manual\""));
    }
}
