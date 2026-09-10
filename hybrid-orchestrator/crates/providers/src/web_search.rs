//! Pluggable web-search abstraction (FEAT-004, architecture.md Section 8.1 chat
//! surface web-search toggle).
//!
//! The composer's 🌐 toggle, when ON, runs a web search for the user's query and
//! injects the results as context before the model answers. The provider is
//! PLUGGABLE behind a small trait + enum so a user can pick (and later swap) a
//! search backend in Settings without touching the send path:
//!
//!   - [`WebSearchProvider`]: the async `search` contract, returning a bounded
//!     `Vec<WebSearchResult>` (title/url/snippet) or a display-safe
//!     [`WebSearchError`].
//!   - [`WebSearchKind`]: the selectable backends. [`WebSearchKind::Tavily`] is
//!     the RECOMMENDED DEFAULT (purpose-built for LLM/agent search, a simple
//!     JSON API, and a free tier). [`WebSearchKind::Brave`] and
//!     [`WebSearchKind::SerpApi`] are scaffolded so the pluggability is real
//!     (their `build` returns an [`WebSearchError::Unsupported`] until wired).
//!   - [`TavilyProvider`]: the working default. It POSTs to a CONFIGURABLE
//!     `base_url` (default [`TAVILY_DEFAULT_BASE_URL`]) so the unit tests can
//!     point it at a local `wiremock` MockServer (no live network).
//!
//! SECRET HYGIENE (Section 9.1 / 9.2): the API key is passed to the provider at
//! construction and is sent only inside the outbound request body per the
//! backend's API. [`WebSearchError`] is display-safe and NEVER carries the key;
//! nothing here logs the key.

use async_trait::async_trait;
use serde::Deserialize;

use crate::capability::HttpSseClient;
use crate::contract::ProviderError;

/// The default Tavily API base URL. Overridable at construction so tests can
/// point the provider at a local mock server (Section: no live network in the
/// sandbox).
pub const TAVILY_DEFAULT_BASE_URL: &str = "https://api.tavily.com";

/// One web-search hit, normalized across providers (FEAT-004). Display-safe:
/// carries only public result fields, never the API key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebSearchResult {
    /// The result title / page headline.
    pub title: String,
    /// The result URL.
    pub url: String,
    /// A short snippet / summary of the page content.
    pub snippet: String,
}

/// Options for a single search call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WebSearchOptions {
    /// Upper bound on the number of results to request.
    pub max_results: u32,
}

impl Default for WebSearchOptions {
    fn default() -> Self {
        WebSearchOptions { max_results: 5 }
    }
}

/// A display-safe web-search error (FEAT-004). It intentionally carries only a
/// human-readable, key-free message: it is surfaced to the user as the reason a
/// search could not run and must NEVER echo the API key.
#[derive(Debug, thiserror::Error)]
pub enum WebSearchError {
    /// A network / connection-level failure before an HTTP response.
    #[error("web search transport error: {0}")]
    Transport(String),
    /// The provider returned a non-success HTTP status (body summarized, never
    /// the key).
    #[error("web search failed with http status {status}")]
    HttpStatus {
        /// The HTTP status code the provider returned.
        status: u16,
    },
    /// The response body could not be parsed into the expected shape.
    #[error("web search response could not be parsed: {0}")]
    Decode(String),
    /// The selected provider kind is scaffolded but not yet implemented.
    #[error("web search provider '{0}' is not supported in this build")]
    Unsupported(String),
    /// The provider was misconfigured (e.g. empty key). Display-safe: names the
    /// problem, never the key value.
    #[error("web search is misconfigured: {0}")]
    Config(String),
}

impl WebSearchError {
    /// Map a shared-client [`ProviderError`] onto a display-safe
    /// [`WebSearchError`]. The `HttpStatus` body from the client is DROPPED (it
    /// could echo request context); only the numeric status is kept.
    fn from_provider_error(err: ProviderError) -> Self {
        match err {
            ProviderError::Transport(msg) => WebSearchError::Transport(msg),
            ProviderError::HttpStatus { status, .. } => WebSearchError::HttpStatus { status },
            ProviderError::Decode(msg) => WebSearchError::Decode(msg),
            ProviderError::UnsupportedCapability(msg) => WebSearchError::Unsupported(msg),
            ProviderError::Auth(msg) => WebSearchError::Config(msg),
            ProviderError::Other(msg) => WebSearchError::Transport(msg),
        }
    }
}

/// The selectable web-search backends (FEAT-004). Serialized in camelCase to
/// match the TS string-literal union the frontend mirrors (`tavily` / `brave` /
/// `serpApi`). [`WebSearchKind::Tavily`] is the recommended default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WebSearchKind {
    /// Tavily (recommended default): purpose-built for LLM/agent search, simple
    /// JSON API, free tier.
    Tavily,
    /// Brave Search API (scaffolded; not yet implemented).
    Brave,
    /// SerpApi (scaffolded; not yet implemented).
    SerpApi,
    /// A user-supplied custom endpoint (the user's OWN search provider). It
    /// speaks the same simple Tavily JSON `/search` contract (the de-facto
    /// minimal shape) and is rooted at a user-entered `base_url`, so a user can
    /// point web search at any Tavily-compatible service instead of picking only
    /// from the preselected backends.
    Custom,
}

impl WebSearchKind {
    /// The recommended default backend.
    pub const DEFAULT: WebSearchKind = WebSearchKind::Tavily;

    /// A stable lowercase id for the kind (used for the keychain handle and
    /// display). Matches the camelCase serde tag.
    pub fn as_str(&self) -> &'static str {
        match self {
            WebSearchKind::Tavily => "tavily",
            WebSearchKind::Brave => "brave",
            WebSearchKind::SerpApi => "serpApi",
            WebSearchKind::Custom => "custom",
        }
    }

    /// Build the configured [`WebSearchProvider`] for this kind with the given
    /// API key and base URL (`None` uses the kind's default endpoint).
    ///
    /// [`WebSearchKind::Tavily`] and [`WebSearchKind::Custom`] are implemented;
    /// the scaffolded kinds return [`WebSearchError::Unsupported`] so the
    /// pluggability seam is real while the additional adapters are added later.
    /// [`WebSearchKind::Custom`] REQUIRES a non-empty `base_url` (the user's own
    /// endpoint) and returns [`WebSearchError::Config`] when it is missing.
    pub fn build(
        &self,
        api_key: &str,
        base_url: Option<&str>,
    ) -> Result<Box<dyn WebSearchProvider>, WebSearchError> {
        if api_key.is_empty() {
            return Err(WebSearchError::Config("an API key is required".to_string()));
        }
        match self {
            WebSearchKind::Tavily => Ok(Box::new(TavilyProvider::new(api_key, base_url))),
            WebSearchKind::Brave => Err(WebSearchError::Unsupported("brave".to_string())),
            WebSearchKind::SerpApi => Err(WebSearchError::Unsupported("serpApi".to_string())),
            WebSearchKind::Custom => {
                // The custom kind is a Tavily-JSON-compatible provider rooted at
                // the user's OWN endpoint, so it MUST carry a non-empty
                // base_url; without one there is nowhere to search.
                let base_url = base_url.filter(|u| !u.trim().is_empty()).ok_or_else(|| {
                    WebSearchError::Config(
                        "a custom web-search endpoint URL is required".to_string(),
                    )
                })?;
                Ok(Box::new(TavilyProvider::new(api_key, Some(base_url))))
            }
        }
    }
}

/// The pluggable web-search contract (FEAT-004). Implementors run a query and
/// return normalized [`WebSearchResult`]s; failures are display-safe and never
/// carry the API key.
///
/// `Debug` is a supertrait so `Box<dyn WebSearchProvider>` is `Debug` (which
/// `WebSearchKind::build(...).unwrap_err()` relies on in tests). Implementors
/// that hold a secret MUST provide a REDACTING `Debug` impl so the key is never
/// exposed via `Debug` (see [`TavilyProvider`]).
#[async_trait]
pub trait WebSearchProvider: std::fmt::Debug + Send + Sync {
    /// Run a search for `query`, returning up to `opts.max_results` results. An
    /// empty results list is a successful empty `Vec`, not an error.
    async fn search(
        &self,
        query: &str,
        opts: WebSearchOptions,
    ) -> Result<Vec<WebSearchResult>, WebSearchError>;
}

/// The Tavily web-search provider (the recommended default). POSTs to
/// `{base_url}/search` with a JSON body `{ api_key, query, max_results }` and
/// parses the `results` array (`title` / `url` / `content`) into
/// [`WebSearchResult`]s.
#[derive(Clone)]
pub struct TavilyProvider {
    client: HttpSseClient,
    api_key: String,
}

// A REDACTING `Debug` impl: `WebSearchProvider` requires `Debug`, but
// `TavilyProvider` holds the API key. A derived `Debug` would print the key, so
// we implement it by hand and redact the secret (SECRET HYGIENE, Section 9.1 /
// 9.2). The key value is NEVER written.
impl std::fmt::Debug for TavilyProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TavilyProvider")
            .field("client", &self.client)
            .field("api_key", &"<redacted>")
            .finish()
    }
}

impl TavilyProvider {
    /// Build a Tavily provider with `api_key`, rooted at `base_url` (or
    /// [`TAVILY_DEFAULT_BASE_URL`] when `None`).
    pub fn new(api_key: impl Into<String>, base_url: Option<&str>) -> Self {
        let base = base_url.unwrap_or(TAVILY_DEFAULT_BASE_URL);
        TavilyProvider {
            client: HttpSseClient::new(base.to_string()),
            api_key: api_key.into(),
        }
    }
}

/// The Tavily `/search` response envelope. Only the `results` array is needed;
/// any other fields (answer, images, ...) are ignored so Tavily can add fields
/// without breaking parsing.
#[derive(Debug, Deserialize)]
struct TavilyResponse {
    #[serde(default)]
    results: Vec<TavilyResult>,
}

/// One Tavily result row. `content` is Tavily's snippet field. Missing string
/// fields default to empty so a partial row does not fail the whole parse.
#[derive(Debug, Deserialize)]
struct TavilyResult {
    #[serde(default)]
    title: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    content: String,
}

#[async_trait]
impl WebSearchProvider for TavilyProvider {
    async fn search(
        &self,
        query: &str,
        opts: WebSearchOptions,
    ) -> Result<Vec<WebSearchResult>, WebSearchError> {
        // Tavily authenticates via the `api_key` field in the JSON body (not a
        // header). The key is sent ONLY here inside the request body and is
        // never logged.
        let body = serde_json::json!({
            "api_key": self.api_key,
            "query": query,
            "max_results": opts.max_results,
        });

        let response: TavilyResponse = self
            .client
            .post_json("/search", &[], &body)
            .await
            .map_err(WebSearchError::from_provider_error)?;

        let results = response
            .results
            .into_iter()
            .map(|r| WebSearchResult {
                title: r.title,
                url: r.url,
                snippet: r.content,
            })
            .collect();
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_default_is_tavily_and_serializes_camelcase() {
        assert_eq!(WebSearchKind::DEFAULT, WebSearchKind::Tavily);
        assert_eq!(WebSearchKind::Tavily.as_str(), "tavily");
        assert_eq!(WebSearchKind::SerpApi.as_str(), "serpApi");
        assert_eq!(WebSearchKind::Custom.as_str(), "custom");
        // The serde tag must match the TS string-literal union the frontend
        // mirrors (`tavily` / `brave` / `serpApi` / `custom`).
        assert_eq!(
            serde_json::to_string(&WebSearchKind::Tavily).unwrap(),
            "\"tavily\""
        );
        assert_eq!(
            serde_json::to_string(&WebSearchKind::SerpApi).unwrap(),
            "\"serpApi\""
        );
        assert_eq!(
            serde_json::to_string(&WebSearchKind::Custom).unwrap(),
            "\"custom\""
        );
    }

    #[test]
    fn build_rejects_empty_key_and_scaffolded_kinds() {
        // Empty key is a display-safe config error (never echoes the key).
        let err = WebSearchKind::Tavily.build("", None).unwrap_err();
        assert!(matches!(err, WebSearchError::Config(_)));

        // The scaffolded kinds report Unsupported so the pluggability is real.
        let err = WebSearchKind::Brave.build("k", None).unwrap_err();
        assert!(matches!(err, WebSearchError::Unsupported(_)));
        let err = WebSearchKind::SerpApi.build("k", None).unwrap_err();
        assert!(matches!(err, WebSearchError::Unsupported(_)));

        // Tavily builds a usable provider.
        assert!(WebSearchKind::Tavily.build("k", None).is_ok());

        // Custom builds a usable (Tavily-JSON-compatible) provider ONLY when a
        // non-empty base_url (the user's own endpoint) is supplied; without one
        // it is a display-safe config error (never echoes the key).
        assert!(WebSearchKind::Custom
            .build("k", Some("https://search.example.com"))
            .is_ok());
        let err = WebSearchKind::Custom.build("k", None).unwrap_err();
        assert!(matches!(err, WebSearchError::Config(_)));
        // A blank/whitespace base_url is treated as missing for Custom.
        let err = WebSearchKind::Custom.build("k", Some("   ")).unwrap_err();
        assert!(matches!(err, WebSearchError::Config(_)));
    }

    #[test]
    fn error_is_display_safe_and_key_free() {
        // The mapped HttpStatus drops the body (which could carry context) and
        // keeps only the numeric status; no variant carries the key.
        let mapped = WebSearchError::from_provider_error(ProviderError::HttpStatus {
            status: 401,
            body: "unauthorized: token sk-SECRET".to_string(),
        });
        let shown = mapped.to_string();
        assert!(shown.contains("401"));
        assert!(!shown.contains("sk-SECRET"));
    }
}
