//! The automatic default routing policy (architecture.md Section 6.2).
//!
//! [`AutoDefaultPolicy`] (`id = "autoDefault"`) is the built-in policy the
//! [`crate::policies::registry::PolicyRegistry`] activates by default. Its
//! [`RoutingPolicy::decide`] runs in two phases:
//!
//! 1. HARD-CONSTRAINT FILTER (first, never negotiable): drop every candidate
//!    that cannot satisfy the request's required capabilities (Section 4.4),
//!    and when a `LocalOnly`/`Confidential` tag applies, keep ONLY local
//!    candidates. If that leaves no candidate under a privacy tag, FAIL CLOSED
//!    with [`RoutingError::PrivacyConstraintUnsatisfiable`] - never fall back to
//!    a cloud model.
//! 2. RANK the survivors by a weighted blend of quality-for-complexity and cost
//!    (Section 6.2), biased by the persona `routing_hint` and the `CostBudget`,
//!    then pick the best and record a human-readable rationale.
//!
//! ## Determining locality
//!
//! [`providers::AvailableModel`] has no explicit `is_local` marker, and adding
//! one would change the `providers` public API. Instead we use the ROBUST price
//! signal already guaranteed by the pricing table: local providers (LM Studio,
//! and any user-unpriced GenericOpenAI endpoint) resolve to
//! [`providers::TokenPrice::ZERO`] (architecture.md Section 6.2:
//! "Local providers such as LM Studio default to zero token cost"), whereas
//! cloud kinds seed nonzero bundled defaults. So `price == ZERO` is treated as
//! "local" (see [`crate::signals::is_local_price`]). This needs no change to the
//! `providers` API and degrades safely: if a user deliberately zero-prices a
//! cloud model they have declared it free/local for routing purposes, which is
//! exactly the intent a zero price expresses.

use async_trait::async_trait;

use crate::policy::{
    AvailableModel, RouteSource, RoutingDecision, RoutingError, RoutingPolicy, RoutingRequest,
};
use crate::signals::{
    estimate_complexity, estimate_cost, is_local_price, required_capabilities, requires_local,
    within_budget, RequiredCapabilities, TaskComplexity,
};
use domain::RoutingHint;

/// The stable id of the automatic default policy.
pub const AUTO_DEFAULT_ID: &str = "autoDefault";

/// The built-in automatic routing policy (architecture.md Section 6.2).
#[derive(Debug, Default, Clone, Copy)]
pub struct AutoDefaultPolicy;

impl AutoDefaultPolicy {
    /// Construct the policy.
    pub fn new() -> Self {
        AutoDefaultPolicy
    }
}

/// A candidate paired with its computed ranking inputs, kept together so the
/// rationale can name the signals that drove the choice.
struct Ranked<'a> {
    model: &'a AvailableModel,
    quality: f64,
    cost: f64,
    local: bool,
    /// The blended score; higher is better.
    score: f64,
}

/// A 0.0..=1.0 quality proxy for a candidate at a given complexity. Larger
/// context windows and richer capabilities read as higher quality; when the
/// task is complex, capability weight matters more.
fn quality_for_complexity(model: &AvailableModel, complexity: TaskComplexity) -> f64 {
    let caps = &model.capabilities;
    // Capability breadth as a coarse strength proxy.
    let mut cap_score = 0.0;
    if caps.tools {
        cap_score += 0.25;
    }
    if caps.vision {
        cap_score += 0.15;
    }
    if caps.json_mode {
        cap_score += 0.1;
    }
    if caps.streaming {
        cap_score += 0.05;
    }
    // Context window as a strength proxy, normalized against a 128k reference.
    let ctx_score = match caps.max_context {
        Some(ctx) => (ctx as f64 / 128_000.0).min(1.0) * 0.45,
        None => 0.1,
    };
    let base = (cap_score + ctx_score).min(1.0);
    // For higher complexity, weight capability strength more heavily; for low
    // complexity, a modest model already suffices, so compress the spread.
    match complexity {
        TaskComplexity::Low => 0.5 + base * 0.5,
        TaskComplexity::Medium => 0.3 + base * 0.7,
        TaskComplexity::High => base,
    }
}

impl AutoDefaultPolicy {
    /// Phase 1: filter candidates by hard constraints (capabilities + privacy).
    /// Returns the survivors, or an `Err` that FAILS CLOSED for privacy.
    fn filter_candidates<'a>(
        req: &'a RoutingRequest,
        required: RequiredCapabilities,
        local_required: bool,
    ) -> Result<Vec<&'a AvailableModel>, RoutingError> {
        if req.available.is_empty() {
            return Err(RoutingError::NoCandidate(
                "the available-model list is empty".to_string(),
            ));
        }

        // Capability filter (Section 4.4).
        let cap_survivors: Vec<&AvailableModel> = req
            .available
            .iter()
            .filter(|m| required.satisfied_by(&m.capabilities))
            .collect();
        if cap_survivors.is_empty() {
            return Err(RoutingError::CapabilityUnsatisfiable(format!(
                "no candidate supports the required capabilities (tools={}, vision={})",
                required.tools, required.vision
            )));
        }

        if local_required {
            let local: Vec<&AvailableModel> = cap_survivors
                .iter()
                .copied()
                .filter(|m| is_local_price(&m.price))
                .collect();
            if local.is_empty() {
                // FAIL CLOSED: never fall back to a cloud model under a
                // LocalOnly/Confidential tag (Section 6.2).
                return Err(RoutingError::PrivacyConstraintUnsatisfiable(
                    "a local-only/confidential tag applies but no local model \
                     (zero-priced) is available"
                        .to_string(),
                ));
            }
            return Ok(local);
        }

        Ok(cap_survivors)
    }
}

#[async_trait]
impl RoutingPolicy for AutoDefaultPolicy {
    fn id(&self) -> &str {
        AUTO_DEFAULT_ID
    }

    async fn decide(&self, req: &RoutingRequest) -> Result<RoutingDecision, RoutingError> {
        let complexity = estimate_complexity(req);
        let required = required_capabilities(req);
        let local_required = requires_local(&req.privacy_tags);

        // Phase 1: hard-constraint filter (fails closed on privacy).
        let survivors = Self::filter_candidates(req, required, local_required)?;

        // Persona hint biases the quality/cost blend (Section 6.2 / 7.1).
        let hint = req.persona.as_ref().and_then(|p| p.routing_hint);

        // Phase 2: rank survivors by a weighted quality-for-complexity + cost
        // blend. Higher complexity weights quality more; hints/budget bias the
        // weights.
        let complexity_score = complexity.score();
        // Base weights: as complexity rises, quality matters more and cost less.
        let mut quality_weight = 0.4 + 0.4 * complexity_score;
        let mut cost_weight = 1.0 - quality_weight;
        match hint {
            Some(RoutingHint::PreferQuality) => {
                quality_weight = (quality_weight + 0.25).min(1.0);
                cost_weight = 1.0 - quality_weight;
            }
            Some(RoutingHint::PreferCheap) | Some(RoutingHint::PreferLocal) => {
                cost_weight = (cost_weight + 0.25).min(1.0);
                quality_weight = 1.0 - cost_weight;
            }
            // PreferSpeed has no dedicated latency signal yet; treat it as a
            // mild lean toward cheaper/local models (typically faster to reach).
            Some(RoutingHint::PreferSpeed) => {
                cost_weight = (cost_weight + 0.1).min(1.0);
                quality_weight = 1.0 - cost_weight;
            }
            None => {}
        }

        // The most expensive candidate normalizes cost into 0.0..=1.0 so it can
        // be blended with quality. Cheaper => higher cost_score.
        let costs: Vec<f64> = survivors
            .iter()
            .map(|m| estimate_cost(req, m, complexity))
            .collect();
        let max_cost = costs.iter().cloned().fold(0.0_f64, f64::max);

        let mut ranked: Vec<Ranked> = survivors
            .iter()
            .zip(costs.iter())
            .map(|(model, &cost)| {
                let quality = quality_for_complexity(model, complexity);
                let cost_score = if max_cost > 0.0 {
                    1.0 - (cost / max_cost)
                } else {
                    1.0
                };
                let local = is_local_price(&model.price);
                let mut score = quality_weight * quality + cost_weight * cost_score;
                // A PreferLocal hint gives local models a small explicit nudge.
                if matches!(hint, Some(RoutingHint::PreferLocal)) && local {
                    score += 0.1;
                }
                // Out-of-budget candidates are strongly penalized (but not hard
                // filtered - a request may have no in-budget option and we still
                // want the least-bad choice unless a privacy tag forbids it).
                if !within_budget(cost, req.budget.as_ref()) {
                    score -= 1.0;
                }
                Ranked {
                    model,
                    quality,
                    cost,
                    local,
                    score,
                }
            })
            .collect();

        // Highest score wins; break ties by lower cost then provider/model id
        // for determinism.
        ranked.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(
                    a.cost
                        .partial_cmp(&b.cost)
                        .unwrap_or(std::cmp::Ordering::Equal),
                )
                .then_with(|| a.model.provider_id.cmp(&b.model.provider_id))
                .then_with(|| a.model.model.cmp(&b.model.model))
        });

        let best = ranked.first().ok_or_else(|| {
            RoutingError::NoCandidate("no candidate survived ranking".to_string())
        })?;

        let rationale = build_rationale(complexity, local_required, &required, hint, best, req);

        Ok(RoutingDecision {
            provider_id: best.model.provider_id.clone(),
            model: best.model.model.clone(),
            rationale,
            source: RouteSource::Automatic,
        })
    }
}

/// Build the human-readable rationale naming the signals that drove the choice
/// (architecture.md Section 6.2: the UI shows "why this model").
fn build_rationale(
    complexity: TaskComplexity,
    local_required: bool,
    required: &RequiredCapabilities,
    hint: Option<RoutingHint>,
    best: &Ranked<'_>,
    req: &RoutingRequest,
) -> String {
    let complexity_str = match complexity {
        TaskComplexity::Low => "low task complexity",
        TaskComplexity::Medium => "medium task complexity",
        TaskComplexity::High => "high task complexity",
    };
    let mut parts = vec![complexity_str.to_string()];
    if local_required {
        parts.push(
            "local-only privacy constraint restricted candidates to local models".to_string(),
        );
    } else {
        parts.push("no privacy constraint".to_string());
    }
    if required.tools {
        parts.push("tools required".to_string());
    }
    if required.vision {
        parts.push("vision required".to_string());
    }
    if let Some(hint) = hint {
        parts.push(format!("persona hint {hint:?}"));
    }
    if best.local {
        parts.push("chose a local (zero-cost) model".to_string());
    } else {
        parts.push(format!("estimated cost {:.4}", best.cost));
        if req.budget.is_some() {
            parts.push("within budget".to_string());
        }
    }
    format!(
        "{}; selected {}/{} (quality score {:.2})",
        parts.join("; "),
        best.model.provider_id,
        best.model.model,
        best.quality
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::{CostBudget, PrivacyTag, RoutingRequest};
    use providers::{AvailableModel, Capabilities, ChatMessage, MessageRole, TokenPrice};

    fn caps(tools: bool, vision: bool, max_context: Option<u32>) -> Capabilities {
        Capabilities {
            streaming: true,
            tools,
            vision,
            json_mode: true,
            max_context,
        }
    }

    fn model(
        provider_id: &str,
        model: &str,
        price: TokenPrice,
        capabilities: Capabilities,
    ) -> AvailableModel {
        AvailableModel {
            provider_id: provider_id.to_string(),
            model: model.to_string(),
            capabilities,
            price,
        }
    }

    fn request(messages: Vec<ChatMessage>, available: Vec<AvailableModel>) -> RoutingRequest {
        RoutingRequest {
            messages,
            privacy_tags: Vec::new(),
            persona: None,
            manual_override: None,
            conversation_pref: None,
            available,
            budget: None,
        }
    }

    fn cloud() -> AvailableModel {
        model(
            "openai",
            "gpt-4o",
            TokenPrice::new(2.5, 10.0),
            caps(true, true, Some(128_000)),
        )
    }

    fn local() -> AvailableModel {
        model(
            "lmstudio",
            "llama-3.1-8b",
            TokenPrice::ZERO,
            caps(true, false, Some(8_000)),
        )
    }

    #[tokio::test]
    async fn low_complexity_prefers_local() {
        let req = request(
            vec![ChatMessage::text(MessageRole::User, "hi")],
            vec![cloud(), local()],
        );
        let decision = AutoDefaultPolicy::new().decide(&req).await.unwrap();
        assert_eq!(decision.provider_id, "lmstudio");
        assert_eq!(decision.source, RouteSource::Automatic);
    }

    #[tokio::test]
    async fn high_complexity_prefers_cloud() {
        let big = "refactor this ```rust\nfn f(){}\n``` ".repeat(200);
        let mut msgs = vec![ChatMessage::text(MessageRole::User, big)];
        for _ in 0..6 {
            msgs.push(ChatMessage::text(MessageRole::Assistant, "ok"));
            msgs.push(ChatMessage::text(MessageRole::User, "keep going"));
        }
        let req = request(msgs, vec![cloud(), local()]);
        assert_eq!(estimate_complexity(&req), TaskComplexity::High);
        let decision = AutoDefaultPolicy::new().decide(&req).await.unwrap();
        assert_eq!(decision.provider_id, "openai");
    }

    #[tokio::test]
    async fn local_only_forces_local() {
        let mut req = request(
            vec![ChatMessage::text(MessageRole::User, "secret data")],
            vec![cloud(), local()],
        );
        req.privacy_tags = vec![PrivacyTag::LocalOnly];
        let decision = AutoDefaultPolicy::new().decide(&req).await.unwrap();
        assert_eq!(decision.provider_id, "lmstudio");
    }

    #[tokio::test]
    async fn local_only_fails_closed_when_no_local() {
        let mut req = request(
            vec![ChatMessage::text(MessageRole::User, "secret")],
            vec![cloud()],
        );
        req.privacy_tags = vec![PrivacyTag::Confidential];
        let err = AutoDefaultPolicy::new().decide(&req).await.unwrap_err();
        assert!(matches!(
            err,
            RoutingError::PrivacyConstraintUnsatisfiable(_)
        ));
    }

    #[tokio::test]
    async fn tools_required_excludes_tool_incapable() {
        // A tool-role message requires tools; the only tool-incapable candidate
        // must be filtered, leaving the tool-capable one.
        let mut tool_msg = ChatMessage::text(MessageRole::Tool, "result");
        tool_msg.tool_call_id = Some("c1".to_string());
        let no_tools = model(
            "generic",
            "weak",
            TokenPrice::ZERO,
            caps(false, false, Some(4_000)),
        );
        let with_tools = model(
            "lmstudio",
            "tooly",
            TokenPrice::ZERO,
            caps(true, false, Some(8_000)),
        );
        let req = request(
            vec![ChatMessage::text(MessageRole::User, "use a tool"), tool_msg],
            vec![no_tools, with_tools],
        );
        let decision = AutoDefaultPolicy::new().decide(&req).await.unwrap();
        assert_eq!(decision.provider_id, "lmstudio");
    }

    #[tokio::test]
    async fn tools_required_with_no_capable_candidate_errors() {
        let mut tool_msg = ChatMessage::text(MessageRole::Tool, "result");
        tool_msg.tool_call_id = Some("c1".to_string());
        let no_tools = model(
            "generic",
            "weak",
            TokenPrice::ZERO,
            caps(false, false, Some(4_000)),
        );
        let req = request(vec![tool_msg], vec![no_tools]);
        let err = AutoDefaultPolicy::new().decide(&req).await.unwrap_err();
        assert!(matches!(err, RoutingError::CapabilityUnsatisfiable(_)));
    }

    #[tokio::test]
    async fn cost_bias_prefers_cheaper_when_quality_adequate() {
        // Two adequate cloud models at low complexity; the cheaper one wins.
        let cheap = model(
            "cheap",
            "mini",
            TokenPrice::new(0.5, 1.5),
            caps(true, true, Some(128_000)),
        );
        let pricey = model(
            "pricey",
            "max",
            TokenPrice::new(5.0, 20.0),
            caps(true, true, Some(128_000)),
        );
        let req = request(
            vec![ChatMessage::text(MessageRole::User, "hi")],
            vec![pricey, cheap],
        );
        let decision = AutoDefaultPolicy::new().decide(&req).await.unwrap();
        assert_eq!(decision.provider_id, "cheap");
    }

    #[tokio::test]
    async fn empty_candidates_errors() {
        let req = request(vec![ChatMessage::text(MessageRole::User, "hi")], vec![]);
        let err = AutoDefaultPolicy::new().decide(&req).await.unwrap_err();
        assert!(matches!(err, RoutingError::NoCandidate(_)));
    }

    #[tokio::test]
    async fn budget_penalizes_over_budget_cloud() {
        // A tight per-message budget makes the zero-cost local model win even at
        // a complexity that would otherwise lean cloud.
        let mut req = request(
            vec![ChatMessage::text(
                MessageRole::User,
                "refactor ```rust\nfn f(){}\n```",
            )],
            vec![cloud(), local()],
        );
        req.budget = Some(CostBudget {
            max_cost_per_message: Some(0.000_001),
            remaining_budget: None,
        });
        let decision = AutoDefaultPolicy::new().decide(&req).await.unwrap();
        assert_eq!(decision.provider_id, "lmstudio");
        assert!(decision.rationale.contains("local"));
    }
}
