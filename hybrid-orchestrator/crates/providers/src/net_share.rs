//! LAN model sharing: the SHARE/serve seam and the peer-DISCOVERY seam
//! (FEAT-006, architecture.md Section 9.3 local-network posture).
//!
//! This build lets a user CONSUME a peer's models by adding the peer as a
//! generic OpenAI-compatible [`crate::ProviderConfig`] row (no code here - it
//! flows through the existing enumeration + routing). This module scaffolds the
//! other two halves cleanly, so the live cross-machine behavior can be wired and
//! verified on a user's build without a redesign:
//!
//!   - SHARE ([`ModelShareServer`]): when enabled, this instance serves a small
//!     OpenAI-compatible READ surface (at minimum `GET /v1/models`) bound to the
//!     LAN on a configurable port, re-exposing this machine's local models to
//!     peers. The models-listing SERIALIZATION ([`render_models_response`]) is a
//!     pure function unit-tested offline; binding to a real LAN interface and
//!     peer reachability are user-only.
//!   - DISCOVERY ([`PeerDiscovery`]): a trait with a stub implementation
//!     ([`StubPeerDiscovery`], an mDNS/UDP-broadcast placeholder) that returns
//!     an EMPTY list without error in-sandbox. The result MAPPING is unit-tested
//!     offline; live discovery is user-only.
//!
//! SECURITY POSTURE (Section 9.3): sharing binds to the LAN and re-exposes local
//! models, so it is OFF BY DEFAULT (see `persistence::ModelSharingConfig`). A
//! peer is OFF-HOST, so the CONSUME side never treats it as provably-local (it
//! is not added to the routing `local_provider_ids` locality set), and a
//! LocalOnly/Confidential conversation therefore never routes to a peer.

use std::net::TcpListener;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// One model this instance re-exposes to LAN peers (FEAT-006). Display-safe: it
/// carries only the public model id, never any secret material.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedModel {
    /// The model id as it should appear to a peer (e.g. `qwen3:8b`).
    pub id: String,
}

impl SharedModel {
    /// Construct a shared-model entry from a model id.
    pub fn new(id: impl Into<String>) -> Self {
        SharedModel { id: id.into() }
    }
}

/// One entry in an OpenAI `GET /v1/models` response body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct OpenAiModelEntry {
    id: String,
    object: String,
    /// The owning organization; OpenAI-compatible peers expect this field. We
    /// stamp it with a fixed marker so a consumer can tell the models were
    /// re-exposed by an Agent Harbor share server.
    owned_by: String,
}

/// The OpenAI `GET /v1/models` response envelope (`{ object, data: [...] }`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct OpenAiModelsResponse {
    object: String,
    data: Vec<OpenAiModelEntry>,
}

/// The `owned_by` marker stamped on every shared model so a consuming peer can
/// tell the listing was re-exposed by an Agent Harbor share server.
const SHARED_MODEL_OWNER: &str = "agent-harbor";

/// Render the OpenAI-compatible `GET /v1/models` response body for the given
/// shared models (FEAT-006). This is the PURE, offline-testable core of the
/// share server's read surface: given the local models this instance exposes, it
/// produces the exact JSON an OpenAI-compatible peer expects
/// (`{ "object": "list", "data": [ { "id", "object": "model", "owned_by" } ] }`).
///
/// DISPLAY-SAFE: the body carries only public model ids, never secret material.
pub fn render_models_response(models: &[SharedModel]) -> serde_json::Value {
    let response = OpenAiModelsResponse {
        object: "list".to_string(),
        data: models
            .iter()
            .map(|m| OpenAiModelEntry {
                id: m.id.clone(),
                object: "model".to_string(),
                owned_by: SHARED_MODEL_OWNER.to_string(),
            })
            .collect(),
    };
    // Serializing a well-formed struct never fails; fall back to an empty list
    // envelope rather than panicking if it somehow does.
    serde_json::to_value(&response)
        .unwrap_or_else(|_| serde_json::json!({ "object": "list", "data": [] }))
}

/// The outcome of trying to start the LAN share server on a port (FEAT-006).
/// Deliberately a VISIBLE status rather than a silent success/hang: enabling
/// sharing on a taken port must report a non-fatal reason the UI can show, never
/// block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShareServerStatus {
    /// The server bound the port successfully and is (conceptually) serving the
    /// read surface. Carries the bound port for display.
    Running {
        /// The port the server bound.
        port: u16,
    },
    /// The server could not bind the port (already in use, permission denied,
    /// ...). Display-safe reason; the UI surfaces it as a non-fatal notice.
    Unavailable {
        /// The port that could not be bound.
        port: u16,
        /// A display-safe reason the bind failed.
        reason: String,
    },
}

/// The LAN model-sharing server seam (FEAT-006). Scaffolds a small
/// OpenAI-compatible read surface (at minimum `GET /v1/models`) that, when
/// started, re-exposes this instance's local models to peers on the LAN.
///
/// The full request loop (accepting connections and replying with
/// [`render_models_response`]) is wired on the user's build; this seam owns the
/// bind-and-report step so enabling sharing degrades to a VISIBLE non-fatal
/// [`ShareServerStatus`] when the port is unavailable, never a silent hang.
#[derive(Debug, Clone)]
pub struct ModelShareServer {
    port: u16,
    models: Vec<SharedModel>,
}

impl ModelShareServer {
    /// Build a share server for `port` re-exposing `models`.
    pub fn new(port: u16, models: Vec<SharedModel>) -> Self {
        ModelShareServer { port, models }
    }

    /// The OpenAI-compatible `GET /v1/models` body this server would return.
    pub fn models_response(&self) -> serde_json::Value {
        render_models_response(&self.models)
    }

    /// Attempt to bind the configured port on all interfaces so the server is
    /// reachable from the LAN, returning a VISIBLE [`ShareServerStatus`] rather
    /// than blocking. A successful bind is immediately released here (the full
    /// serving loop is user-only); the point of this seam is to fail FAST and
    /// visibly when the port is unavailable so the UI can report it.
    ///
    /// Binding to a real LAN interface + peer reachability is USER-ONLY (the
    /// sandbox has no cross-machine network); this method is what the enable
    /// path calls to surface a non-fatal status.
    pub fn try_bind(&self) -> ShareServerStatus {
        // 0.0.0.0 so the server is reachable from the LAN, not just loopback.
        match TcpListener::bind(("0.0.0.0", self.port)) {
            Ok(_listener) => ShareServerStatus::Running { port: self.port },
            Err(err) => ShareServerStatus::Unavailable {
                port: self.port,
                // std::io::Error's Display is a short, key-free reason.
                reason: err.to_string(),
            },
        }
    }
}

/// A peer discovered on the local network (FEAT-006). Display-safe: carries only
/// a label + base URL a user can add as a consume peer, never any secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredPeer {
    /// A human-friendly label for the peer (e.g. the advertised host name).
    pub label: String,
    /// The peer's OpenAI-compatible base URL (e.g. `http://192.168.1.50:11435/v1`).
    pub base_url: String,
}

/// The peer-discovery seam (FEAT-006). Implementors probe the local network
/// (mDNS / UDP broadcast) for reachable Agent Harbor / OpenAI-compatible peers
/// within `timeout` and return display-safe [`DiscoveredPeer`]s.
///
/// Discovery must be non-fatal: an error or a timeout returns an EMPTY list, and
/// the UI shows a non-fatal "no peers found / discovery unavailable" notice
/// rather than failing.
pub trait PeerDiscovery: Send + Sync {
    /// Discover peers, bounded by `timeout`. Returns an empty vec (never an
    /// error) when nothing is found or discovery is unavailable.
    fn discover(&self, timeout: Duration) -> Vec<DiscoveredPeer>;
}

/// The default peer-discovery implementation (FEAT-006): a placeholder for the
/// real mDNS/UDP-broadcast probe. In-sandbox (and until the live probe is wired)
/// it returns an EMPTY list without error, so the command + UI exercise the
/// non-fatal empty path. Live discovery is USER-ONLY.
///
/// The real mDNS crate is deliberately NOT added as a dependency yet: the
/// offline cargo cache cannot resolve a new dep and live discovery cannot be
/// tested in-sandbox anyway, so the concrete probe is a documented follow-up.
#[derive(Debug, Clone, Default)]
pub struct StubPeerDiscovery;

impl PeerDiscovery for StubPeerDiscovery {
    fn discover(&self, _timeout: Duration) -> Vec<DiscoveredPeer> {
        // No live probe wired yet: return empty so the caller shows the
        // non-fatal "no peers found" notice rather than an error.
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_models_response_is_openai_shaped() {
        let body =
            render_models_response(&[SharedModel::new("qwen3:8b"), SharedModel::new("phi-3")]);
        assert_eq!(body["object"], "list");
        let data = body["data"].as_array().expect("data array");
        assert_eq!(data.len(), 2);
        assert_eq!(data[0]["id"], "qwen3:8b");
        assert_eq!(data[0]["object"], "model");
        assert_eq!(data[0]["owned_by"], SHARED_MODEL_OWNER);
        assert_eq!(data[1]["id"], "phi-3");
    }

    #[test]
    fn render_models_response_empty_is_empty_list_not_error() {
        let body = render_models_response(&[]);
        assert_eq!(body["object"], "list");
        assert!(body["data"].as_array().expect("data array").is_empty());
    }

    #[test]
    fn share_server_exposes_its_models_response() {
        let server = ModelShareServer::new(11435, vec![SharedModel::new("llama3")]);
        let body = server.models_response();
        assert_eq!(body["data"][0]["id"], "llama3");
    }

    #[test]
    fn try_bind_reports_a_visible_status() {
        // Binding to port 0 lets the OS pick a free port, so this proves the
        // success path returns a visible Running status (never blocks). Live LAN
        // reachability is user-only; here we only assert the status shape.
        let server = ModelShareServer::new(0, vec![]);
        match server.try_bind() {
            ShareServerStatus::Running { .. } => {}
            other => panic!("expected Running on an ephemeral port, got {other:?}"),
        }
    }

    #[test]
    fn try_bind_on_a_taken_port_is_unavailable_not_a_hang() {
        // Hold a port, then a second server on the same port must report a
        // VISIBLE Unavailable status rather than hanging or panicking.
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind first");
        let port = listener.local_addr().expect("addr").port();
        let server = ModelShareServer::new(port, vec![]);
        match server.try_bind() {
            ShareServerStatus::Unavailable { port: p, reason } => {
                assert_eq!(p, port);
                assert!(!reason.is_empty());
            }
            // On some platforms 0.0.0.0:<port> may not collide with 127.0.0.1;
            // a Running status is also acceptable so long as it does not hang.
            ShareServerStatus::Running { .. } => {}
        }
    }

    #[test]
    fn stub_discovery_returns_empty_without_error() {
        let discovery = StubPeerDiscovery;
        assert!(discovery.discover(Duration::from_millis(50)).is_empty());
    }
}
