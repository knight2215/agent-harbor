//! Process-wide rustls crypto-provider initialization.
//!
//! reqwest 0.12 with the `rustls-tls` feature is built on rustls 0.23, which
//! REQUIRES a process-default [`CryptoProvider`](rustls::crypto::CryptoProvider)
//! to be installed before any rustls-backed client is used. When more than one
//! provider is linked in, rustls has no unambiguous default and connector setup
//! fails on the first request. This crate links two providers indirectly:
//!
//! - reqwest's `rustls-tls` pulls in `ring`;
//! - the AWS crates used by the Bedrock adapter (`aws-sigv4` /
//!   `aws-credential-types` and their transitive rustls users) pull in
//!   `aws-lc-rs`.
//!
//! With both present and no default selected, building the reqwest connector
//! fails at first request and surfaces as a bare `reqwest::Error` transport
//! error, even for plain-HTTP requests (reqwest builds one connector that
//! supports both http and https, and its TLS init runs regardless of scheme).
//!
//! To remove the ambiguity we install `ring` as the process default exactly
//! once, before any client is constructed. [`ensure_crypto_provider`] is called
//! from every client-construction path in this crate.

use std::sync::Once;

static INIT: Once = Once::new();

/// Install the `ring` rustls [`CryptoProvider`](rustls::crypto::CryptoProvider)
/// as the process default, exactly once.
///
/// This is idempotent and cheap after the first call. `install_default` returns
/// `Err` if a default is already set (e.g. installed by another crate or a test
/// harness); that is fine and expected, so the result is ignored. The only
/// requirement is that a single, unambiguous default exists before a
/// rustls-backed reqwest client is built.
pub fn ensure_crypto_provider() {
    INIT.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}
