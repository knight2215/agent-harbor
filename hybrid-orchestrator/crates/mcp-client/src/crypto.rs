//! Process-wide rustls crypto-provider initialization for the HTTP/SSE
//! transport.
//!
//! This mirrors `providers::crypto` exactly (Phase 2). reqwest 0.12 with the
//! `rustls-tls` feature is built on rustls 0.23, which REQUIRES a process-default
//! [`CryptoProvider`](rustls::crypto::CryptoProvider) to be installed before any
//! rustls-backed client is used. When more than one provider is linked in,
//! rustls has no unambiguous default and connector setup fails on the first
//! request.
//!
//! To remove the ambiguity we install `ring` as the process default exactly
//! once, before any client is constructed. [`ensure_crypto_provider`] is called
//! from the HTTP/SSE transport's client-construction path.
//!
//! NOTE: `providers` exposes the same installer, but this crate installs its own
//! copy so the HTTP/SSE transport does not depend on a provider being linked or
//! having run first; `install_default` is idempotent (it returns `Err` when a
//! default is already set, which is fine and expected), so both installers
//! calling it is harmless.

use std::sync::Once;

static INIT: Once = Once::new();

/// Install the `ring` rustls [`CryptoProvider`](rustls::crypto::CryptoProvider)
/// as the process default, exactly once.
///
/// Idempotent and cheap after the first call. `install_default` returns `Err`
/// if a default is already set (e.g. installed by `providers` or a test
/// harness); that is expected, so the result is ignored. The only requirement
/// is that a single, unambiguous default exists before a rustls-backed reqwest
/// client is built.
pub fn ensure_crypto_provider() {
    INIT.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}
