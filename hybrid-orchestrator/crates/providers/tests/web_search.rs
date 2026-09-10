//! Mocked-HTTP tests for the pluggable web-search abstraction (FEAT-004).
//!
//! Every test points the [`TavilyProvider`]'s `base_url` at a local [`wiremock`]
//! server; NO live network is used (the sandbox cannot reach api.tavily.com).
//! They assert:
//!
//!   - the Tavily request is well-formed (method POST, path `/search`, body
//!     carries `query` + `max_results`; the key is sent per Tavily's API but is
//!     asserted present in the body only, never logged);
//!   - a mocked 200 JSON response parses into the expected `WebSearchResult`
//!     vec (Tavily's `content` maps to `snippet`);
//!   - a mocked non-200 / malformed body yields a display-safe `WebSearchError`
//!     that never echoes the key;
//!   - an empty `results` array yields an empty vec (not an error).

use providers::{
    ensure_crypto_provider, TavilyProvider, WebSearchError, WebSearchOptions, WebSearchProvider,
};
use serde_json::{json, Value};
use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

/// A recorded Tavily `/search` success body (title/url/content per result).
fn tavily_fixture() -> Value {
    json!({
        "query": "rust async",
        "results": [
            {
                "title": "Async Rust",
                "url": "https://example.com/async",
                "content": "A guide to async/await in Rust."
            },
            {
                "title": "Tokio",
                "url": "https://tokio.rs",
                "content": "An async runtime for Rust."
            }
        ]
    })
}

#[tokio::test]
async fn tavily_request_is_well_formed_and_carries_query_and_max_results() {
    ensure_crypto_provider();
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/search"))
        .and(body_partial_json(json!({
            "query": "rust async",
            "max_results": 3
        })))
        .respond_with(|req: &Request| {
            // The key is sent per Tavily's API (in the JSON body), and is
            // asserted here on the captured request WITHOUT logging it.
            let body: Value = serde_json::from_slice(&req.body).unwrap();
            assert_eq!(
                body.get("api_key").and_then(Value::as_str),
                Some("tvly-test-key")
            );
            ResponseTemplate::new(200).set_body_json(tavily_fixture())
        })
        .expect(1)
        .mount(&server)
        .await;

    let provider = TavilyProvider::new("tvly-test-key", Some(&server.uri()));
    let results = provider
        .search("rust async", WebSearchOptions { max_results: 3 })
        .await
        .unwrap();

    assert_eq!(results.len(), 2);
    assert_eq!(results[0].title, "Async Rust");
    assert_eq!(results[0].url, "https://example.com/async");
    assert_eq!(results[0].snippet, "A guide to async/await in Rust.");
    assert_eq!(results[1].title, "Tokio");
}

#[tokio::test]
async fn tavily_200_parses_into_expected_results() {
    ensure_crypto_provider();
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(tavily_fixture()))
        .mount(&server)
        .await;

    let provider = TavilyProvider::new("tvly-test-key", Some(&server.uri()));
    let results = provider
        .search("anything", WebSearchOptions::default())
        .await
        .unwrap();

    // Tavily's `content` field maps to our normalized `snippet`.
    let snippets: Vec<_> = results.into_iter().map(|r| r.snippet).collect();
    assert_eq!(
        snippets,
        vec![
            "A guide to async/await in Rust.".to_string(),
            "An async runtime for Rust.".to_string()
        ]
    );
}

#[tokio::test]
async fn tavily_non_200_yields_display_safe_error_without_key() {
    ensure_crypto_provider();
    let server = MockServer::start().await;

    // A 401 whose body embeds a secret-looking token: the mapped error must
    // keep only the numeric status and NEVER echo the body/key.
    Mock::given(method("POST"))
        .and(path("/search"))
        .respond_with(
            ResponseTemplate::new(401)
                .set_body_string("unauthorized: api_key tvly-test-key is invalid"),
        )
        .mount(&server)
        .await;

    let provider = TavilyProvider::new("tvly-test-key", Some(&server.uri()));
    let err = provider
        .search("q", WebSearchOptions::default())
        .await
        .unwrap_err();

    assert!(matches!(err, WebSearchError::HttpStatus { status: 401 }));
    let shown = err.to_string();
    assert!(shown.contains("401"));
    assert!(
        !shown.contains("tvly-test-key"),
        "error must never echo the API key: {shown}"
    );
}

#[tokio::test]
async fn tavily_malformed_body_yields_decode_error() {
    ensure_crypto_provider();
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_string("this is not json"))
        .mount(&server)
        .await;

    let provider = TavilyProvider::new("tvly-test-key", Some(&server.uri()));
    let err = provider
        .search("q", WebSearchOptions::default())
        .await
        .unwrap_err();
    assert!(matches!(err, WebSearchError::Decode(_)));
}

#[tokio::test]
async fn tavily_empty_results_is_empty_vec_not_error() {
    ensure_crypto_provider();
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "results": [] })))
        .mount(&server)
        .await;

    let provider = TavilyProvider::new("tvly-test-key", Some(&server.uri()));
    let results = provider
        .search("q", WebSearchOptions::default())
        .await
        .unwrap();
    assert!(results.is_empty());
}
