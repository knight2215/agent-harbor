//! `routing` crate: the routing policy engine (architecture.md Section 6).
//!
//! This crate decides which provider/model should answer a message. It exposes:
//!
//! - [`RoutingPolicy`], the pluggable async trait, plus its [`RoutingRequest`],
//!   [`RoutingDecision`], [`CostBudget`], and [`RoutingError`] types (Section
//!   6.1);
//! - the three signal families in [`signals`] (task complexity, privacy
//!   hard-constraints, cost - Section 6.2);
//! - [`AutoDefaultPolicy`], the built-in automatic policy that filters by hard
//!   constraints first and then ranks by a quality-for-complexity + cost blend
//!   (Section 6.2);
//! - [`ManualOverrideResolver`], the per-message-override > pin > automatic
//!   precedence resolver that wraps the active automatic policy (Section 6.3);
//! - [`PolicyRegistry`], which holds the selectable/persisted active policy and
//!   whose [`PolicyRegistry::resolve`] applies the precedence (Section 6.4).
//!
//! ## Reused DTOs
//!
//! The request/decision types build on the leaf `domain` DTOs
//! ([`domain::ManualRoute`], [`domain::PrivacyTag`], [`domain::RouteSource`],
//! [`domain::AgentPersona`]) and the `providers` selector DTOs
//! ([`providers::AvailableModel`], [`providers::ChatMessage`]) rather than
//! duplicating them; those crates remain the single source of truth.
//!
//! ## Workspace arrangement
//!
//! Phase 4 gave this crate path deps on `providers` and `domain`, which carry
//! crates.io deps transitively (reqwest/sqlx/keyring/...). Those are unreachable
//! in the offline sandbox, so - like every other crate that carries crates.io
//! deps - this crate is under `[workspace] exclude` in the root Cargo.toml and
//! is built/tested/clippied in CI via
//! `--manifest-path crates/routing/Cargo.toml`. Its unit tests use no live
//! network (they construct in-memory `AvailableModel` candidates directly).

pub mod policy;
pub mod signals;

pub mod policies {
    //! Routing policy implementations (architecture.md Section 6.2-6.4).
    pub mod auto_default;
    pub mod manual_override;
    pub mod registry;
}

// Ergonomic crate-root re-exports (ALPHABETICALLY ORDERED), matching the
// convention in the other crates. The reused `domain`/`providers` types the
// public API surfaces are re-exported through [`policy`].
pub use policies::auto_default::AutoDefaultPolicy;
pub use policies::manual_override::ManualOverrideResolver;
pub use policies::registry::PolicyRegistry;
pub use policy::{
    AvailableModel, CostBudget, ManualRoute, PrivacyTag, RouteSource, RoutingDecision,
    RoutingError, RoutingPolicy, RoutingRequest,
};
