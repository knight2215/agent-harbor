//! Mocked-HTTP proof of the LAN-peer CONSUME path (FEAT-006).
//!
//! A LAN peer is represented as an ordinary generic OpenAI-compatible
//! [`ProviderConfig`] row whose `base_url` points at the peer's `host:port`.
//! Because it is a normal provider row of a supported kind, it enumerates and
//! routes through the EXISTING path with no pipeline change: this test proves an
//! OpenAI-compatible endpoint at an ARBITRARY (mock) base_url has its models
//! enumerated via the shared native adapter by
//! [`providers::list_available_models`] - the same call the tauri-app picker and
//! send path use.
//!
//! NO live network: the peer's base_url points at a local [`wiremock`] server
//! (the sandbox cannot reach a real LAN peer). Live cross-machine reachability is
//! user-only.

use domain::{ProviderConfig, ProviderKind};
use providers::{
    builtin_registry, ensure_crypto_provider, list_available_models, PricingTable, SharedModel,
};
use secrets::InMemorySecretStore;
use serde_json::{json, Value};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// An OpenAI `GET /models` listing body a peer would return.
fn peer_models_fixture() -> Value {
    json!({
        "object": "list",
        "data": [
            { "id": "qwen3:8b", "object": "model", "owned_by": "agent-harbor" },
            { "id": "phi-3", "object": "model", "owned_by": "agent-harbor" }
        ]
    })
}

#[tokio::test]
async fn lan_peer_models_enumerate_via_generic_openai_at_arbitrary_base_url() {
    ensure_crypto_provider();
    let peer = MockServer::start().await;

    // The peer serves the OpenAI-compatible model listing at `/models`.
    Mock::given(method("GET"))
        .and(path("/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(peer_models_fixture()))
        .mount(&peer)
        .await;

    // A LAN peer is a generic OpenAI-compatible row pointed at the peer's
    // arbitrary base_url (here the mock server's URI; in production e.g.
    // http://192.168.1.50:11435/v1). It is keyless, so no auth header is sent.
    let cfg = ProviderConfig {
        id: "network-peer-abc123".to_string(),
        kind: ProviderKind::GenericOpenAI,
        base_url: Some(peer.uri()),
        api_key_ref: None,
        extra: Value::Null,
    };

    // Build the registry from the peer row exactly as the app does, then
    // enumerate. The models come back attributed to the peer's provider id.
    let store = InMemorySecretStore::new();
    let mut registry = builtin_registry();
    registry
        .build_from_config(&cfg, &store)
        .expect("build the peer provider");

    let configs = [cfg];
    let result = list_available_models(&registry, &configs, &PricingTable::new())
        .await
        .expect("enumerate peer models");

    assert!(
        result.errors.is_empty(),
        "the peer should enumerate cleanly, got errors: {:?}",
        result.errors
    );
    let models: Vec<_> = result
        .models
        .iter()
        .map(|m| (m.provider_id.as_str(), m.model.as_str()))
        .collect();
    assert!(models.contains(&("network-peer-abc123", "qwen3:8b")));
    assert!(models.contains(&("network-peer-abc123", "phi-3")));
    // A peer's models price at the generic-openai default (zero) since no
    // pricing row was configured; that is fine - the point is they enumerate.
    assert_eq!(result.models.len(), 2);
}

#[tokio::test]
async fn share_server_renders_the_same_openai_listing_shape_a_peer_consumes() {
    // The SHARE side re-exposes local models in exactly the OpenAI listing shape
    // the CONSUME side above reads, so a peer can enumerate them. This is the
    // offline proof of the share server's models-listing serialization; live LAN
    // binding is user-only.
    let body = providers::render_models_response(&[
        SharedModel::new("qwen3:8b"),
        SharedModel::new("phi-3"),
    ]);
    assert_eq!(body["object"], "list");
    assert_eq!(body["data"][0]["id"], "qwen3:8b");
    assert_eq!(body["data"][0]["object"], "model");
    assert_eq!(body["data"][1]["id"], "phi-3");
}
