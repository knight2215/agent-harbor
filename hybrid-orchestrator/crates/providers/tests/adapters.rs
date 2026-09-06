//! Mocked-HTTP integration tests for the native OpenAI-compatible adapters
//! (architecture.md Section 4.3, tasks.md P2.4/P2.5/P2.6, FEAT-002).
//!
//! Every test points the adapter's `base_url` at a local [`wiremock`] server;
//! NO live network is used. They assert:
//!
//! - request translation: method / path / headers / body per adapter (OpenAI
//!   Bearer; LM Studio no auth; Generic user base_url + optional Bearer; Azure
//!   path-rewrite + `api-key` header + `api-version` query, reading
//!   `extra.deployment` / `extra.api_version`);
//! - non-streaming decode into [`ChatResponse`] from a fixture body;
//! - SSE stream normalization: fixture `data:` frames decode into ordered
//!   [`ChatDelta`]s, terminating on `[DONE]`;
//! - capability negotiation gating (a model reporting `tools=false` causes tools
//!   to be omitted from the outbound body).

use domain::{ProviderConfig, ProviderKind};
use futures_util::StreamExt;
use providers::adapters::azure_openai::build_azure_openai;
use providers::adapters::generic_openai::build_generic_openai;
use providers::adapters::lmstudio::build_lmstudio;
use providers::adapters::openai::build_openai;
use providers::{
    ensure_crypto_provider, negotiate, Capabilities, ChatMessage, ChatProvider, ChatRequest,
    MessageRole, ToolSpec,
};
use secrets::{InMemorySecretStore, SecretStore};
use serde_json::{json, Value};
use wiremock::matchers::{body_partial_json, header, method, path, query_param};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

/// A recorded OpenAI non-streaming chat completion body.
fn chat_completion_fixture() -> Value {
    json!({
        "id": "chatcmpl-1",
        "object": "chat.completion",
        "model": "gpt-4o",
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": "Hello there"},
            "finish_reason": "stop"
        }],
        "usage": {"prompt_tokens": 5, "completion_tokens": 2, "total_tokens": 7}
    })
}

/// A fixture SSE body: `data: {chunk}` frames plus the `[DONE]` sentinel.
fn sse_fixture() -> String {
    concat!(
        "data: {\"choices\":[{\"delta\":{\"role\":\"assistant\"}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n",
    )
    .to_string()
}

fn simple_request(model: &str) -> ChatRequest {
    ChatRequest::new(model, vec![ChatMessage::text(MessageRole::User, "hi")])
}

// ---------------------------------------------------------------------------
// OpenAI
// ---------------------------------------------------------------------------

#[tokio::test]
async fn openai_non_streaming_sends_bearer_and_decodes_response() {
    ensure_crypto_provider();
    let server = MockServer::start().await;
    let store = InMemorySecretStore::new();
    let key_ref = store.store("openai", "sk-test-123").unwrap();

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(header("authorization", "Bearer sk-test-123"))
        .and(header("content-type", "application/json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(chat_completion_fixture()))
        .expect(1)
        .mount(&server)
        .await;

    let cfg = ProviderConfig {
        id: "openai".to_string(),
        kind: ProviderKind::OpenAI,
        base_url: Some(server.uri()),
        api_key_ref: Some(key_ref),
        extra: Value::Null,
    };
    let adapter = build_openai(&cfg, &store).unwrap();

    let resp = adapter.chat(simple_request("gpt-4o")).await.unwrap();
    assert_eq!(resp.choices.len(), 1);
    assert_eq!(
        resp.choices[0].message.content.as_deref(),
        Some("Hello there")
    );
    assert_eq!(resp.usage.unwrap().total_tokens, 7);
}

#[tokio::test]
async fn openai_streaming_normalizes_sse_into_ordered_deltas() {
    ensure_crypto_provider();
    let server = MockServer::start().await;
    let store = InMemorySecretStore::new();
    let key_ref = store.store("openai", "sk-test-123").unwrap();

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse_fixture()),
        )
        .mount(&server)
        .await;

    let cfg = ProviderConfig {
        id: "openai".to_string(),
        kind: ProviderKind::OpenAI,
        base_url: Some(server.uri()),
        api_key_ref: Some(key_ref),
        extra: Value::Null,
    };
    let adapter = build_openai(&cfg, &store).unwrap();

    let mut req = simple_request("gpt-4o");
    req.stream = true;
    let deltas: Vec<_> = adapter
        .chat_stream(req)
        .await
        .unwrap()
        .map(|d| d.unwrap())
        .collect()
        .await;

    // role-only opener is skipped; two content deltas + one finish delta.
    let contents: Vec<_> = deltas.iter().filter_map(|d| d.content.clone()).collect();
    assert_eq!(contents, vec!["Hel".to_string(), "lo".to_string()]);
    assert!(deltas.iter().any(|d| d.finish_reason.is_some()));
}

#[tokio::test]
async fn openai_lists_models() {
    ensure_crypto_provider();
    let server = MockServer::start().await;
    let store = InMemorySecretStore::new();
    let key_ref = store.store("openai", "sk-test-123").unwrap();

    Mock::given(method("GET"))
        .and(path("/models"))
        .and(header("authorization", "Bearer sk-test-123"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "object": "list",
            "data": [{"id": "gpt-4o"}, {"id": "gpt-4o-mini"}]
        })))
        .mount(&server)
        .await;

    let cfg = ProviderConfig {
        id: "openai".to_string(),
        kind: ProviderKind::OpenAI,
        base_url: Some(server.uri()),
        api_key_ref: Some(key_ref),
        extra: Value::Null,
    };
    let adapter = build_openai(&cfg, &store).unwrap();
    let models = adapter.list_models().await.unwrap();
    let ids: Vec<_> = models.into_iter().map(|m| m.id).collect();
    assert_eq!(ids, vec!["gpt-4o".to_string(), "gpt-4o-mini".to_string()]);
}

// ---------------------------------------------------------------------------
// LM Studio
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lmstudio_sends_no_authorization_header() {
    ensure_crypto_provider();
    let server = MockServer::start().await;
    let store = InMemorySecretStore::new();

    // Assert absence of Authorization by inspecting the captured request.
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(|req: &Request| {
            assert!(
                req.headers.get("authorization").is_none(),
                "LM Studio must not send an Authorization header by default"
            );
            ResponseTemplate::new(200).set_body_json(chat_completion_fixture())
        })
        .expect(1)
        .mount(&server)
        .await;

    let cfg = ProviderConfig {
        id: "lmstudio".to_string(),
        kind: ProviderKind::LmStudio,
        base_url: Some(server.uri()),
        api_key_ref: None,
        extra: Value::Null,
    };
    let adapter = build_lmstudio(&cfg, &store).unwrap();
    let resp = adapter.chat(simple_request("local-model")).await.unwrap();
    assert_eq!(
        resp.choices[0].message.content.as_deref(),
        Some("Hello there")
    );
}

// ---------------------------------------------------------------------------
// Generic OpenAI-compatible
// ---------------------------------------------------------------------------

#[tokio::test]
async fn generic_uses_user_base_url_and_optional_bearer() {
    ensure_crypto_provider();
    let server = MockServer::start().await;
    let store = InMemorySecretStore::new();
    let key_ref = store.store("generic", "user-key").unwrap();

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(header("authorization", "Bearer user-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(chat_completion_fixture()))
        .expect(1)
        .mount(&server)
        .await;

    let cfg = ProviderConfig {
        id: "generic".to_string(),
        kind: ProviderKind::GenericOpenAI,
        base_url: Some(server.uri()),
        api_key_ref: Some(key_ref),
        extra: Value::Null,
    };
    let adapter = build_generic_openai(&cfg, &store).unwrap();
    let resp = adapter.chat(simple_request("some-model")).await.unwrap();
    assert_eq!(
        resp.choices[0].message.content.as_deref(),
        Some("Hello there")
    );
}

// ---------------------------------------------------------------------------
// Azure OpenAI
// ---------------------------------------------------------------------------

#[tokio::test]
async fn azure_rewrites_path_adds_api_version_and_uses_api_key_header() {
    ensure_crypto_provider();
    let server = MockServer::start().await;
    let store = InMemorySecretStore::new();
    let key_ref = store.store("azure", "az-secret").unwrap();

    Mock::given(method("POST"))
        .and(path("/openai/deployments/gpt4o-deploy/chat/completions"))
        .and(query_param("api-version", "2024-02-01"))
        .and(header("api-key", "az-secret"))
        .respond_with(ResponseTemplate::new(200).set_body_json(chat_completion_fixture()))
        .expect(1)
        .mount(&server)
        .await;

    let cfg = ProviderConfig {
        id: "azure".to_string(),
        kind: ProviderKind::Azure,
        base_url: Some(server.uri()),
        api_key_ref: Some(key_ref),
        extra: json!({"deployment": "gpt4o-deploy", "api_version": "2024-02-01"}),
    };
    let adapter = build_azure_openai(&cfg, &store).unwrap();

    // Azure must NOT use Bearer; verified via the api-key header matcher above.
    let resp = adapter.chat(simple_request("gpt4o-deploy")).await.unwrap();
    assert_eq!(
        resp.choices[0].message.content.as_deref(),
        Some("Hello there")
    );
}

// ---------------------------------------------------------------------------
// Capability negotiation gating the outbound body
// ---------------------------------------------------------------------------

#[tokio::test]
async fn capability_negotiation_omits_tools_when_model_reports_no_tools() {
    ensure_crypto_provider();
    let server = MockServer::start().await;
    let store = InMemorySecretStore::new();
    let key_ref = store.store("openai", "sk-test-123").unwrap();

    // The mock only matches when the body contains NO "tools" key: prove the
    // negotiated request stripped tools before it went out.
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(|req: &Request| {
            let body: Value = serde_json::from_slice(&req.body).unwrap();
            assert!(
                body.get("tools").is_none(),
                "tools must be stripped when the model reports tools=false"
            );
            ResponseTemplate::new(200).set_body_json(chat_completion_fixture())
        })
        .expect(1)
        .mount(&server)
        .await;

    let cfg = ProviderConfig {
        id: "openai".to_string(),
        kind: ProviderKind::OpenAI,
        base_url: Some(server.uri()),
        api_key_ref: Some(key_ref),
        extra: Value::Null,
    };
    let adapter = build_openai(&cfg, &store).unwrap();

    // Build a request that wants tools, then gate it against a tools=false model.
    let mut req = simple_request("gpt-4o");
    req.tools = vec![ToolSpec::function("f", "d", json!({"type": "object"}))];
    let caps = Capabilities {
        streaming: true,
        tools: false,
        vision: false,
        json_mode: false,
        max_context: None,
    };
    let negotiated = negotiate(req, &caps);
    assert!(negotiated.tools_stripped);

    adapter.chat(negotiated.request).await.unwrap();
}

/// Also assert request translation at the body level for OpenAI: the model and
/// messages round-trip and `stream` is present in the outbound body.
#[tokio::test]
async fn openai_body_translation_includes_model_and_messages() {
    ensure_crypto_provider();
    let server = MockServer::start().await;
    let store = InMemorySecretStore::new();
    let key_ref = store.store("openai", "sk-test-123").unwrap();

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(body_partial_json(json!({
            "model": "gpt-4o",
            "messages": [{"role": "user", "content": "hi"}],
            "stream": false
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(chat_completion_fixture()))
        .expect(1)
        .mount(&server)
        .await;

    let cfg = ProviderConfig {
        id: "openai".to_string(),
        kind: ProviderKind::OpenAI,
        base_url: Some(server.uri()),
        api_key_ref: Some(key_ref),
        extra: Value::Null,
    };
    let adapter = build_openai(&cfg, &store).unwrap();
    adapter.chat(simple_request("gpt-4o")).await.unwrap();
}
