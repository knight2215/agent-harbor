//! The three routing signal families (architecture.md Section 6.2): task
//! complexity, privacy hard-constraints, and cost.
//!
//! These are pure, side-effect-free helpers the automatic policy
//! ([`crate::policies::auto_default`]) combines into a ranked candidate list.
//! Keeping them here makes each one independently unit-testable.
//!
//! ## Pricing source
//!
//! The cost signal reads [`providers::TokenPrice`] straight off each
//! [`providers::AvailableModel::price`]. That price is NOT a live feed: the
//! Tauri command derives it from the user-entered per-provider rates persisted
//! in the versioned `AppConfig` pricing table (architecture.md Section 6.2 /
//! 10.4), seeded with bundled defaults per `ProviderKind`. Local providers
//! (LM Studio) default to [`providers::TokenPrice::ZERO`], so a zero price is a
//! reasonable "this is a no-cost model" signal for the COST dimension (see
//! [`is_local_price`]).
//!
//! ## Locality is COST-independent for the privacy gate
//!
//! A zero price is NOT trusted as proof of locality for the privacy hard
//! constraint (architecture.md Section 6.2). Price is a cost signal, and a
//! misconfigured or deliberately zero-priced CLOUD provider would otherwise
//! read as "local" and be allowed under a `LocalOnly`/`Confidential` tag, which
//! is exactly the leak the fail-closed rule exists to prevent. Instead the
//! caller supplies the set of provably-local provider instance ids
//! ([`RoutingRequest::local_provider_ids`], derived from the concrete
//! `ProviderKind` at candidate-enumeration time), and [`is_provably_local`]
//! keys off THAT set. Locality is therefore fail-closed: a candidate whose id
//! is not in the provably-local set is treated as non-local regardless of its
//! price.

use std::collections::BTreeSet;

use providers::{AvailableModel, ChatMessage, MessageRole, TokenPrice};

use crate::policy::{CostBudget, PrivacyTag, RoutingRequest};

/// A coarse task-difficulty scale estimated from the request (architecture.md
/// Section 6.2). Low-complexity turns prefer a capable local model;
/// high-complexity turns prefer a stronger cloud model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TaskComplexity {
    Low,
    Medium,
    High,
}

impl TaskComplexity {
    /// A 0.0..=1.0 numeric proxy used when blending signals: `Low = 0.0`,
    /// `Medium = 0.5`, `High = 1.0`.
    pub fn score(self) -> f64 {
        match self {
            TaskComplexity::Low => 0.0,
            TaskComplexity::Medium => 0.5,
            TaskComplexity::High => 1.0,
        }
    }
}

/// The hard capabilities a request demands (architecture.md Section 4.4). A
/// candidate model that lacks any required capability is filtered out BEFORE
/// ranking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RequiredCapabilities {
    /// The request carries tool specs or the persona demands tools, so the
    /// model must support function calling.
    pub tools: bool,
    /// The request carries image inputs, so the model must support vision.
    pub vision: bool,
}

impl RequiredCapabilities {
    /// Whether `caps` satisfies every requirement in `self`.
    pub fn satisfied_by(self, caps: &providers::Capabilities) -> bool {
        (!self.tools || caps.tools) && (!self.vision || caps.vision)
    }
}

/// Estimate the [`TaskComplexity`] of `req` from cheap heuristics over its
/// messages (architecture.md Section 6.2): total text length, message count,
/// code/structured cues, and whether tools/vision are required.
pub fn estimate_complexity(req: &RoutingRequest) -> TaskComplexity {
    let mut score: i32 = 0;

    let total_len: usize = req
        .messages
        .iter()
        .filter_map(|m| m.content.as_deref())
        .map(|c| c.len())
        .sum();

    // Length: long prompts tend to be harder.
    if total_len > 4000 {
        score += 2;
    } else if total_len > 800 {
        score += 1;
    }

    // Multi-turn / many-message conversations tend to be harder.
    let turns = req
        .messages
        .iter()
        .filter(|m| matches!(m.role, MessageRole::User | MessageRole::Assistant))
        .count();
    if turns >= 6 {
        score += 2;
    } else if turns >= 3 {
        score += 1;
    }

    // Code / structured-reasoning cues bump complexity.
    if has_code_or_structured_cues(&req.messages) {
        score += 2;
    }

    // Required context window: a request that needs a large context is harder.
    // We proxy "needs large context" by the total prompt length in chars mapped
    // to a rough token estimate (see `estimate_prompt_tokens`).
    if estimate_prompt_tokens(&req.messages) > 8000 {
        score += 1;
    }

    // Tools / vision requirements signal a more capable model is warranted.
    let required = required_capabilities(req);
    if required.tools {
        score += 1;
    }
    if required.vision {
        score += 1;
    }

    if score >= 4 {
        TaskComplexity::High
    } else if score >= 2 {
        TaskComplexity::Medium
    } else {
        TaskComplexity::Low
    }
}

/// Whether any message carries code or structured-reasoning cues: fenced code
/// blocks, JSON/brace density, or keywords like `function`/`class`/`algorithm`/
/// `refactor`.
fn has_code_or_structured_cues(messages: &[ChatMessage]) -> bool {
    const KEYWORDS: [&str; 8] = [
        "function",
        "class ",
        "algorithm",
        "refactor",
        "impl ",
        "def ",
        "async ",
        "struct ",
    ];
    for msg in messages {
        let Some(content) = msg.content.as_deref() else {
            continue;
        };
        if content.contains("```") {
            return true;
        }
        // Structured JSON/brace cues: an opening brace/bracket paired with a
        // close is a cheap "looks like code/JSON" proxy.
        if (content.contains('{') && content.contains('}'))
            || (content.contains('[') && content.contains(']'))
        {
            return true;
        }
        let lowered = content.to_lowercase();
        if KEYWORDS.iter().any(|kw| lowered.contains(kw)) {
            return true;
        }
    }
    false
}

/// Derive the [`RequiredCapabilities`] of `req` (architecture.md Section 4.4):
/// tools are required when a `tool`-role message is present (a tool loop is in
/// flight) or the persona pins a tool-capable workflow; vision is required when
/// any message carries a non-text/image cue. Extensible.
pub fn required_capabilities(req: &RoutingRequest) -> RequiredCapabilities {
    // Tools: a `tool`-role message means a function-calling loop is underway,
    // so the answering model must support tools.
    let tools = req
        .messages
        .iter()
        .any(|m| matches!(m.role, MessageRole::Tool) || !m.tool_calls.is_empty());

    // Vision: image inputs are surfaced as a data-URI / image cue in message
    // text (the internal contract is text-only, so we detect the marker the
    // pipeline injects). Kept conservative so we never over-require vision.
    let vision = req.messages.iter().any(|m| {
        m.content
            .as_deref()
            .is_some_and(|c| c.contains("data:image/"))
    });

    RequiredCapabilities { tools, vision }
}

/// Whether a `LocalOnly`/`Confidential` tag makes local routing a HARD
/// constraint (architecture.md Section 6.2). `Custom` tags do not force local
/// on their own.
pub fn requires_local(privacy_tags: &[PrivacyTag]) -> bool {
    privacy_tags
        .iter()
        .any(|t| matches!(t, PrivacyTag::LocalOnly | PrivacyTag::Confidential))
}

/// Whether `price` marks a zero-COST model (architecture.md Section 6.2:
/// "Local providers such as LM Studio default to zero token cost"). This drives
/// the COST signal only. It is deliberately NOT used to decide locality for the
/// privacy hard constraint - see [`is_provably_local`] and the module doc for
/// why a zero price is not trusted as proof of locality.
pub fn is_local_price(price: &TokenPrice) -> bool {
    *price == TokenPrice::ZERO
}

/// Whether `candidate` is PROVABLY local for the privacy hard constraint
/// (architecture.md Section 6.2). Locality is decided by membership in the
/// caller-supplied `local_provider_ids` set (derived from the concrete
/// `ProviderKind` when the candidate list is enumerated), NOT by price. This is
/// fail-closed: a candidate whose provider id is not known to be local is
/// treated as non-local, so a mispriced or zero-priced cloud model can never
/// satisfy a `LocalOnly`/`Confidential` tag.
pub fn is_provably_local(
    candidate: &AvailableModel,
    local_provider_ids: &BTreeSet<String>,
) -> bool {
    local_provider_ids.contains(&candidate.provider_id)
}

/// A rough token estimate for the prompt: ~4 chars per token (the common OpenAI
/// rule of thumb). Used both by the complexity heuristic and the cost signal so
/// the two agree on prompt size.
pub fn estimate_prompt_tokens(messages: &[ChatMessage]) -> u32 {
    let chars: usize = messages
        .iter()
        .filter_map(|m| m.content.as_deref())
        .map(|c| c.chars().count())
        .sum();
    (chars / 4).max(1) as u32
}

/// Estimate the cost of answering `req` with `candidate`, in the same currency
/// units as [`TokenPrice`] (architecture.md Section 6.2). Input tokens come from
/// the prompt estimate; output tokens are assumed to scale with complexity (a
/// harder task tends to produce a longer answer). LOCAL models are zero cost.
pub fn estimate_cost(
    req: &RoutingRequest,
    candidate: &AvailableModel,
    complexity: TaskComplexity,
) -> f64 {
    if is_local_price(&candidate.price) {
        return 0.0;
    }
    let input_tokens = estimate_prompt_tokens(&req.messages) as f64;
    // Assume the answer grows with complexity: Low ~256, Medium ~512, High ~1024
    // output tokens.
    let output_tokens = match complexity {
        TaskComplexity::Low => 256.0,
        TaskComplexity::Medium => 512.0,
        TaskComplexity::High => 1024.0,
    };
    let per_mtok = 1_000_000.0;
    (input_tokens / per_mtok) * candidate.price.input_per_mtok
        + (output_tokens / per_mtok) * candidate.price.output_per_mtok
}

/// Whether `cost` fits within the optional [`CostBudget`] (architecture.md
/// Section 6.1). An absent budget always fits. A per-message ceiling is a hard
/// gate; a near-exhausted remaining budget also gates.
pub fn within_budget(cost: f64, budget: Option<&CostBudget>) -> bool {
    let Some(budget) = budget else {
        return true;
    };
    if budget.max_cost_per_message.is_some_and(|max| cost > max) {
        return false;
    }
    if budget
        .remaining_budget
        .is_some_and(|remaining| cost > remaining)
    {
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::RoutingRequest;
    use providers::{AvailableModel, Capabilities, ChatMessage, MessageRole, TokenPrice};

    fn request(messages: Vec<ChatMessage>) -> RoutingRequest {
        RoutingRequest {
            messages,
            privacy_tags: Vec::new(),
            persona: None,
            manual_override: None,
            conversation_pref: None,
            available: Vec::new(),
            local_provider_ids: BTreeSet::new(),
            budget: None,
        }
    }

    fn model(provider_id: &str, price: TokenPrice, caps: Capabilities) -> AvailableModel {
        AvailableModel {
            provider_id: provider_id.to_string(),
            model: "m".to_string(),
            capabilities: caps,
            price,
        }
    }

    #[test]
    fn short_plain_prompt_is_low_complexity() {
        let req = request(vec![ChatMessage::text(MessageRole::User, "hi there")]);
        assert_eq!(estimate_complexity(&req), TaskComplexity::Low);
    }

    #[test]
    fn code_cue_raises_complexity() {
        let req = request(vec![ChatMessage::text(
            MessageRole::User,
            "please refactor this ```rust\nfn main() {}\n```",
        )]);
        // Code fence + keyword + braces push it above Low.
        assert!(estimate_complexity(&req) >= TaskComplexity::Medium);
    }

    #[test]
    fn long_multi_turn_is_high_complexity() {
        let big = "x".repeat(5000);
        let mut msgs = vec![ChatMessage::text(MessageRole::User, big)];
        for _ in 0..6 {
            msgs.push(ChatMessage::text(MessageRole::Assistant, "reply"));
            msgs.push(ChatMessage::text(MessageRole::User, "follow up"));
        }
        let req = request(msgs);
        assert_eq!(estimate_complexity(&req), TaskComplexity::High);
    }

    #[test]
    fn privacy_local_only_and_confidential_force_local() {
        assert!(requires_local(&[PrivacyTag::LocalOnly]));
        assert!(requires_local(&[PrivacyTag::Confidential]));
        assert!(!requires_local(&[PrivacyTag::Custom("team".to_string())]));
        assert!(!requires_local(&[]));
    }

    #[test]
    fn required_capabilities_detects_tools() {
        let mut msg = ChatMessage::text(MessageRole::Tool, "result");
        msg.tool_call_id = Some("c1".to_string());
        let req = request(vec![msg]);
        assert!(required_capabilities(&req).tools);
    }

    #[test]
    fn required_capabilities_default_is_none() {
        let req = request(vec![ChatMessage::text(MessageRole::User, "hi")]);
        let required = required_capabilities(&req);
        assert!(!required.tools);
        assert!(!required.vision);
    }

    #[test]
    fn capability_satisfaction() {
        let required = RequiredCapabilities {
            tools: true,
            vision: false,
        };
        let mut caps = Capabilities::default();
        assert!(!required.satisfied_by(&caps));
        caps.tools = true;
        assert!(required.satisfied_by(&caps));
    }

    #[test]
    fn local_model_is_zero_cost() {
        let req = request(vec![ChatMessage::text(MessageRole::User, "hi")]);
        let local = model("local", TokenPrice::ZERO, Capabilities::default());
        assert_eq!(estimate_cost(&req, &local, TaskComplexity::High), 0.0);
        assert!(is_local_price(&local.price));
    }

    #[test]
    fn provable_locality_keys_off_the_set_not_price() {
        // A zero-priced CLOUD model absent from the local set is NOT provably
        // local (fail-closed); a model present in the set is, regardless of price.
        let cloud_zero = model("cloud", TokenPrice::ZERO, Capabilities::default());
        let local_priced = model(
            "lmstudio",
            TokenPrice::new(0.1, 0.2),
            Capabilities::default(),
        );
        let local_ids: BTreeSet<String> = ["lmstudio".to_string()].into_iter().collect();
        assert!(!is_provably_local(&cloud_zero, &local_ids));
        assert!(is_provably_local(&local_priced, &local_ids));
        // Empty set => nothing is provably local (fail-closed).
        assert!(!is_provably_local(&local_priced, &BTreeSet::new()));
    }

    #[test]
    fn cloud_model_cost_scales_with_complexity() {
        let req = request(vec![ChatMessage::text(MessageRole::User, "hi there")]);
        let cloud = model(
            "openai",
            TokenPrice::new(2.5, 10.0),
            Capabilities::default(),
        );
        let low = estimate_cost(&req, &cloud, TaskComplexity::Low);
        let high = estimate_cost(&req, &cloud, TaskComplexity::High);
        assert!(high > low);
        assert!(low > 0.0);
    }

    #[test]
    fn budget_gates_cost() {
        assert!(within_budget(0.5, None));
        let budget = CostBudget {
            max_cost_per_message: Some(1.0),
            remaining_budget: None,
        };
        assert!(within_budget(0.5, Some(&budget)));
        assert!(!within_budget(1.5, Some(&budget)));
        let remaining = CostBudget {
            max_cost_per_message: None,
            remaining_budget: Some(0.2),
        };
        assert!(!within_budget(0.5, Some(&remaining)));
    }
}
