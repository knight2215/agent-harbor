//! `providers` crate: `ChatProvider` contract and adapters (placeholder).
//!
//! Phase 0 scaffold. The real contract, registry, capability negotiation, and
//! adapters land in later phases.

pub mod capability;
pub mod contract;
pub mod registry;

pub mod adapters {
    //! Provider adapter implementations (placeholders).
    pub mod anthropic;
    pub mod azure_openai;
    pub mod bedrock;
    pub mod gemini;
    pub mod generic_openai;
    pub mod lmstudio;
    pub mod openai;
}

/// Placeholder marker so dependents can reference an item from this crate,
/// proving the dependency edge compiles. Replaced with real API in later phases.
pub fn placeholder() {}

#[cfg(test)]
mod tests {
    #[test]
    fn smoke() {
        // Exercises the placeholder marker so the smoke test does real work.
        super::placeholder();
    }
}
