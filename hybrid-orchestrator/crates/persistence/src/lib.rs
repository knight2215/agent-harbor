//! `persistence` crate: SQLite storage and versioned config schema (placeholder).
//!
//! Phase 0 scaffold. The real database pool, repositories, and config
//! migration land in later phases.

pub mod config;
pub mod db;
pub mod repositories;

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
