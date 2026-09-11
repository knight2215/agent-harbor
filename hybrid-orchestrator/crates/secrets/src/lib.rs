//! `secrets` crate: keystore abstraction over the OS keychain.
//!
//! This is the canonical home of [`SecretRef`] (architecture.md Section 3.3):
//! an opaque, serde-serializable handle that identifies a secret in the OS
//! keychain WITHOUT ever carrying the secret material itself. `orchestrator-core`
//! re-exports this type for its domain models (dependency direction is
//! `orchestrator-core -> secrets`, never the reverse).
//!
//! Security invariants (architecture.md Sections 9.1 / 9.2):
//!   - The raw key material is written into the OS keychain (macOS Keychain,
//!     Windows Credential Manager, Linux Secret Service) under a namespaced
//!     service id, and referenced elsewhere only by a [`SecretRef`] handle.
//!   - `ProviderConfig` and the SQLite store hold only [`SecretRef`], never the
//!     key. See `orchestrator_core::ProviderConfig::api_key_ref`.
//!   - The ONLY seam that returns key material is [`SecretStore::resolve`], and
//!     it is documented as core-internal: it is used inside the core at request
//!     time to attach a key to an outbound provider request. NO command exposes
//!     it across IPC; no other accessor returns plaintext.
//!
//! Two [`SecretStore`] implementations are provided:
//!   - [`KeyringSecretStore`]: backed by the `keyring` crate, for production.
//!   - [`InMemorySecretStore`]: a process-local map, for tests and headless CI
//!     (Linux CI has no Secret Service, so keyring-backed tests would hang or
//!     fail; the CI-run assertions use this store instead).

use std::collections::HashMap;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// The keychain service id under which all secrets for this app are namespaced
/// (architecture.md Section 9.1: "stored under a namespaced service id").
pub const KEYCHAIN_SERVICE: &str = "com.agent-harbor.hybrid-orchestrator";

/// Opaque reference into the OS keychain.
///
/// A `SecretRef` NEVER contains secret material. It is only a stable handle
/// (the account/entry name under [`KEYCHAIN_SERVICE`]) used to look the secret
/// up in the keystore at call time. It is safe to persist in SQLite, embed in
/// `ProviderConfig`, and serialize - none of those carry the key.
///
/// Canonical home (architecture.md Section 3.3). `orchestrator-core` re-exports
/// this type; do not define a second `SecretRef` elsewhere.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SecretRef(pub String);

impl SecretRef {
    /// Construct a handle from a stable account/entry id.
    pub fn new(handle: impl Into<String>) -> Self {
        SecretRef(handle.into())
    }

    /// The opaque handle string. This is NOT the secret; it is only the
    /// keychain entry name.
    pub fn handle(&self) -> &str {
        &self.0
    }
}

/// Errors from keystore operations.
#[derive(Debug, thiserror::Error)]
pub enum SecretError {
    /// No secret is stored under the given handle.
    #[error("no secret found for handle '{0}'")]
    NotFound(String),
    /// The underlying OS keychain backend failed.
    #[error("keystore backend error: {0}")]
    Backend(String),
}

/// Abstraction over a secret store (architecture.md Section 9.1).
///
/// The trait is deliberately small. Its `resolve` method is the SINGLE internal
/// seam that returns key material, and it is documented as core-only: callers
/// outside the core's controlled request-building path must not invoke it, and
/// NO Tauri command surfaces it. There is intentionally no other accessor that
/// returns plaintext.
pub trait SecretStore {
    /// Store `secret` under a stable `handle` and return the [`SecretRef`] that
    /// references it. Overwrites any existing secret for that handle (this is
    /// also the rotation path - the reference is stable, per Section 9.1).
    fn store(&self, handle: &str, secret: &str) -> Result<SecretRef, SecretError>;

    /// Resolve a [`SecretRef`] to its secret material.
    ///
    /// CORE-INTERNAL ONLY. This is the one method that returns plaintext. It is
    /// called inside the core at request time to attach a key to an outbound
    /// provider request (architecture.md Section 9.1: "keys are read inside the
    /// core at request time"). It must NEVER be wired to a Tauri command or
    /// have its result cross the IPC boundary (Section 9.2).
    fn resolve(&self, secret_ref: &SecretRef) -> Result<String, SecretError>;

    /// Delete the secret referenced by `secret_ref`. Idempotent: deleting a
    /// missing entry is not an error.
    fn delete(&self, secret_ref: &SecretRef) -> Result<(), SecretError>;

    /// Rotate the secret behind an existing handle to `new_secret`. The
    /// [`SecretRef`] is stable across rotation (Section 9.1).
    fn rotate(&self, secret_ref: &SecretRef, new_secret: &str) -> Result<SecretRef, SecretError> {
        self.store(secret_ref.handle(), new_secret)
    }

    /// Round-trip a sentinel value through the store to verify the backing
    /// keychain actually persists and returns secrets.
    ///
    /// This is the health check that exposes the v0.8.1 keychain bug from the
    /// user's own build: a mock/no-op credential store (or any broken backend)
    /// accepts a `store()` yet fails or mismatches on the following `resolve()`.
    /// The check stores a fixed sentinel under a dedicated temp handle,
    /// resolves it, asserts round-trip equality, then deletes the temp entry.
    ///
    /// It returns `Result<(), SecretError>` and NEVER returns the sentinel
    /// across its signature, so it does not add a plaintext accessor and the
    /// "resolve is the only plaintext seam" invariant is preserved. On any
    /// mismatch it returns a display-safe [`SecretError`] that carries no
    /// secret material.
    fn self_test(&self) -> Result<(), SecretError> {
        const SELF_TEST_HANDLE: &str = "harbor-selftest";
        const SELF_TEST_SENTINEL: &str = "harbor-selftest-sentinel";

        let secret_ref = self.store(SELF_TEST_HANDLE, SELF_TEST_SENTINEL)?;
        let resolved = self.resolve(&secret_ref);
        // Always attempt to clean up the temp entry, even if the resolve
        // mismatched, so a self-test never leaves a lingering sentinel.
        let _ = self.delete(&secret_ref);
        match resolved {
            Ok(value) if value == SELF_TEST_SENTINEL => Ok(()),
            Ok(_) => Err(SecretError::Backend(
                "keychain self-test round-trip mismatch: stored and resolved values differ"
                    .to_string(),
            )),
            Err(e) => Err(e),
        }
    }
}

/// Production [`SecretStore`] backed by the OS keychain via the `keyring` crate
/// (architecture.md Section 9.1). Entries live under [`KEYCHAIN_SERVICE`] keyed
/// by the [`SecretRef`] handle as the account name.
///
/// NOTE: on headless Linux CI there is no Secret Service, so operations here
/// would fail or block. Tests that exercise this store are `#[ignore]`d; use
/// [`InMemorySecretStore`] for automated tests.
#[derive(Debug, Default, Clone)]
pub struct KeyringSecretStore;

impl KeyringSecretStore {
    pub fn new() -> Self {
        KeyringSecretStore
    }

    fn entry(handle: &str) -> Result<keyring::Entry, SecretError> {
        keyring::Entry::new(KEYCHAIN_SERVICE, handle)
            .map_err(|e| SecretError::Backend(e.to_string()))
    }
}

impl SecretStore for KeyringSecretStore {
    fn store(&self, handle: &str, secret: &str) -> Result<SecretRef, SecretError> {
        let entry = Self::entry(handle)?;
        entry
            .set_password(secret)
            .map_err(|e| SecretError::Backend(e.to_string()))?;
        Ok(SecretRef::new(handle))
    }

    fn resolve(&self, secret_ref: &SecretRef) -> Result<String, SecretError> {
        let entry = Self::entry(secret_ref.handle())?;
        match entry.get_password() {
            Ok(secret) => Ok(secret),
            Err(keyring::Error::NoEntry) => {
                Err(SecretError::NotFound(secret_ref.handle().to_string()))
            }
            Err(e) => Err(SecretError::Backend(e.to_string())),
        }
    }

    fn delete(&self, secret_ref: &SecretRef) -> Result<(), SecretError> {
        let entry = Self::entry(secret_ref.handle())?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(SecretError::Backend(e.to_string())),
        }
    }
}

/// Process-local [`SecretStore`] for tests and headless CI.
///
/// Keeps secrets in an in-memory map so tests do NOT require a real OS keychain
/// (headless Linux CI has no Secret Service). This is the store used by the
/// CI-run assertions.
#[derive(Debug, Default)]
pub struct InMemorySecretStore {
    inner: Mutex<HashMap<String, String>>,
}

impl InMemorySecretStore {
    pub fn new() -> Self {
        InMemorySecretStore {
            inner: Mutex::new(HashMap::new()),
        }
    }
}

impl SecretStore for InMemorySecretStore {
    fn store(&self, handle: &str, secret: &str) -> Result<SecretRef, SecretError> {
        let mut map = self
            .inner
            .lock()
            .map_err(|e| SecretError::Backend(format!("lock poisoned: {e}")))?;
        map.insert(handle.to_string(), secret.to_string());
        Ok(SecretRef::new(handle))
    }

    fn resolve(&self, secret_ref: &SecretRef) -> Result<String, SecretError> {
        let map = self
            .inner
            .lock()
            .map_err(|e| SecretError::Backend(format!("lock poisoned: {e}")))?;
        map.get(secret_ref.handle())
            .cloned()
            .ok_or_else(|| SecretError::NotFound(secret_ref.handle().to_string()))
    }

    fn delete(&self, secret_ref: &SecretRef) -> Result<(), SecretError> {
        let mut map = self
            .inner
            .lock()
            .map_err(|e| SecretError::Backend(format!("lock poisoned: {e}")))?;
        map.remove(secret_ref.handle());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_then_resolve_round_trips() {
        let store = InMemorySecretStore::new();
        let secret_ref = store.store("openai", "sk-secret-key-123").unwrap();
        assert_eq!(secret_ref, SecretRef::new("openai"));
        // The single documented internal seam returns the material.
        assert_eq!(store.resolve(&secret_ref).unwrap(), "sk-secret-key-123");
    }

    #[test]
    fn rotate_keeps_ref_stable_and_replaces_material() {
        let store = InMemorySecretStore::new();
        let secret_ref = store.store("anthropic", "old-key").unwrap();
        let rotated = store.rotate(&secret_ref, "new-key").unwrap();
        // The handle (reference) is stable across rotation (Section 9.1).
        assert_eq!(rotated, secret_ref);
        assert_eq!(store.resolve(&secret_ref).unwrap(), "new-key");
    }

    #[test]
    fn delete_is_idempotent_and_removes_material() {
        let store = InMemorySecretStore::new();
        let secret_ref = store.store("gemini", "key").unwrap();
        store.delete(&secret_ref).unwrap();
        // Deleting again is not an error.
        store.delete(&secret_ref).unwrap();
        match store.resolve(&secret_ref) {
            Err(SecretError::NotFound(h)) => assert_eq!(h, "gemini"),
            other => panic!("expected NotFound after delete, got {other:?}"),
        }
    }

    /// The SecretRef, its Debug form, and its serialized form must expose only
    /// the opaque handle - NEVER the secret material. This test is written so
    /// that it would FAIL if someone added a plaintext getter that leaked the
    /// key into `SecretRef` or its representations.
    ///
    /// The type-level guarantee (there is no accessor on `SecretRef` returning
    /// plaintext, and the only plaintext seam is the documented core-internal
    /// `SecretStore::resolve`) is what actually prevents leaks; this asserts the
    /// observable representations respect it.
    #[test]
    fn secret_ref_never_exposes_plaintext() {
        let store = InMemorySecretStore::new();
        let plaintext = "sk-super-secret-DO-NOT-LEAK";
        let secret_ref = store.store("provider-1", plaintext).unwrap();

        // The handle accessor returns the handle, not the secret.
        assert_eq!(secret_ref.handle(), "provider-1");
        assert!(!secret_ref.handle().contains(plaintext));

        // Debug output contains the handle but not the plaintext.
        let debug = format!("{secret_ref:?}");
        assert!(debug.contains("provider-1"));
        assert!(
            !debug.contains(plaintext),
            "SecretRef Debug must not contain secret material"
        );

        // Serialized form contains the handle but not the plaintext.
        let json = serde_json::to_string(&secret_ref).unwrap();
        assert_eq!(json, "\"provider-1\"");
        assert!(
            !json.contains(plaintext),
            "SecretRef serialization must not contain secret material"
        );
    }

    /// P6.1 keychain audit regression: [`SecretStore::resolve`] is the SINGLE
    /// seam that returns plaintext. The other trait methods (`store`, `delete`,
    /// `rotate`) return only a [`SecretRef`] handle or `()` - never the secret
    /// material - so no accessor other than `resolve` can leak the key.
    ///
    /// This is a type-shape guard: the trait return types are what enforce the
    /// invariant (there is no `fn peek(&self) -> String` etc.), and this test
    /// exercises each method to assert its observable output never contains the
    /// planted secret. A regression that added a plaintext-returning accessor or
    /// made `store`/`rotate` echo the secret would fail here.
    #[test]
    fn resolve_is_the_only_plaintext_seam() {
        let store = InMemorySecretStore::new();
        let plaintext = "sk-ONLY-resolve-may-return-this";

        // `store` returns only the opaque handle.
        let secret_ref = store.store("provider-x", plaintext).unwrap();
        assert_eq!(secret_ref, SecretRef::new("provider-x"));
        assert!(!format!("{secret_ref:?}").contains(plaintext));
        assert!(!serde_json::to_string(&secret_ref)
            .unwrap()
            .contains(plaintext));

        // `rotate` returns only the (stable) handle, not the new material.
        let rotated = store.rotate(&secret_ref, "sk-rotated-secret").unwrap();
        assert_eq!(rotated, secret_ref);
        assert!(!format!("{rotated:?}").contains("sk-rotated-secret"));

        // `resolve` is the ONE method that returns the material.
        assert_eq!(store.resolve(&secret_ref).unwrap(), "sk-rotated-secret");

        // `delete` returns `Result<(), _>`, carrying nothing on success (the
        // `()` payload type itself is the proof that no material is returned).
        store.delete(&secret_ref).unwrap();
    }

    /// `self_test()` round-trips a sentinel through a working store and
    /// returns Ok, leaving no lingering temp entry behind.
    #[test]
    fn self_test_ok_on_working_store() {
        let store = InMemorySecretStore::new();
        assert!(store.self_test().is_ok());
        // The temp sentinel must be cleaned up after a successful self-test.
        match store.resolve(&SecretRef::new("harbor-selftest")) {
            Err(SecretError::NotFound(_)) => {}
            other => panic!("self-test must delete its temp entry, got {other:?}"),
        }
    }

    /// A store whose `resolve` cannot return what `store` accepted is exactly
    /// the v0.8.1 mock-store failure mode: `store()` succeeds but the value
    /// does not persist. `self_test()` must surface that as an `Err`, never a
    /// false Ok.
    #[derive(Default)]
    struct NoPersistStore;

    impl SecretStore for NoPersistStore {
        fn store(&self, handle: &str, _secret: &str) -> Result<SecretRef, SecretError> {
            // Accepts the write (like the mock/no-op store) but persists nothing.
            Ok(SecretRef::new(handle))
        }

        fn resolve(&self, secret_ref: &SecretRef) -> Result<String, SecretError> {
            Err(SecretError::NotFound(secret_ref.handle().to_string()))
        }

        fn delete(&self, _secret_ref: &SecretRef) -> Result<(), SecretError> {
            Ok(())
        }
    }

    #[test]
    fn self_test_err_when_round_trip_fails() {
        let store = NoPersistStore;
        match store.self_test() {
            Err(SecretError::NotFound(h)) => assert_eq!(h, "harbor-selftest"),
            other => panic!("self-test over a non-persisting store must Err, got {other:?}"),
        }
    }

    /// A store that persists but returns a DIFFERENT value must also fail the
    /// self-test (the mismatch branch), never returning Ok.
    #[derive(Default)]
    struct MismatchStore;

    impl SecretStore for MismatchStore {
        fn store(&self, handle: &str, _secret: &str) -> Result<SecretRef, SecretError> {
            Ok(SecretRef::new(handle))
        }

        fn resolve(&self, _secret_ref: &SecretRef) -> Result<String, SecretError> {
            Ok("a-different-value".to_string())
        }

        fn delete(&self, _secret_ref: &SecretRef) -> Result<(), SecretError> {
            Ok(())
        }
    }

    #[test]
    fn self_test_err_on_value_mismatch() {
        let store = MismatchStore;
        match store.self_test() {
            Err(SecretError::Backend(msg)) => assert!(msg.contains("mismatch")),
            other => panic!("self-test over a mismatching store must Err, got {other:?}"),
        }
    }

    /// Keyring-backed round-trip. IGNORED by default because headless Linux CI
    /// has no Secret Service (the call would hang or fail); run manually on a
    /// machine with a real OS keychain via `cargo test -- --ignored`.
    #[test]
    #[ignore = "requires a real OS keychain (no Secret Service on headless CI)"]
    fn keyring_store_round_trips() {
        let store = KeyringSecretStore::new();
        let handle = "test-keyring-entry";
        let secret_ref = store.store(handle, "keyring-secret").unwrap();
        assert_eq!(store.resolve(&secret_ref).unwrap(), "keyring-secret");
        store.delete(&secret_ref).unwrap();
    }
}
