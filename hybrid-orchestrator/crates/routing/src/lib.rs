//! `routing` crate: routing policy engine (placeholder).
//!
//! Phase 0 scaffold. The real policy trait, signals, and policies land in later
//! phases.

pub mod policy;
pub mod signals;

pub mod policies {
    //! Routing policy implementations (placeholders).
    pub mod auto_default;
    pub mod manual_override;
    pub mod registry;
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
