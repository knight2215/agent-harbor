//! `orchestrator-core` crate: domain models and message pipeline (placeholder).
//!
//! Phase 0 scaffold. Framework-agnostic (no Tauri dependency). Coordinates
//! routing, providers, MCP, and persistence. The real session manager,
//! pipeline, and events land in later phases.

pub mod events;
pub mod pipeline;
pub mod session;

/// Placeholder marker so dependents can reference an item from this crate,
/// proving the dependency edge compiles. Replaced with real API in later phases.
pub fn placeholder() {}

#[cfg(test)]
mod tests {
    /// Smoke test that references an item from each of the five library crates
    /// orchestrator-core depends on, proving the dependency edges compile.
    #[test]
    fn smoke() {
        providers::placeholder();
        mcp_client::placeholder();
        routing::placeholder();
        persistence::placeholder();
        secrets::placeholder();
    }
}
