//! `secrets` crate: keystore abstraction over the OS keychain (placeholder).
//!
//! Phase 0 scaffold. The real `SecretStore` trait and keyring-backed
//! implementation land in later phases. Secrets are never exposed across IPC.

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
