// Web Search settings section (FEAT-004).
//
// Configures the PLUGGABLE web-search provider used by the composer's 🌐 toggle.
// The user picks a provider KIND from a dropdown (Tavily is the recommended
// default - purpose-built for LLM/agent search, a simple JSON API, and a free
// tier) and enters an API key; the form calls `set_web_search_provider`
// (ipc/commands.ts), which stores the key as an opaque SecretRef under a stable
// keychain handle and persists the selection + result cap into the additive
// AppConfig.web_search field. The configured provider is the BACKEND source of
// truth, rehydrated on mount from `get_web_search_config`.
//
// SECRET HYGIENE (Section 9.1): the plaintext key flows IN and only an opaque
// SecretRef handle is stored server-side; no command returns key material, and
// this form NEVER displays or persists the key.
//
// LIVE VERIFICATION: web search needs the user's own key AND live network, so
// its real behavior can only be verified on the user's build (the sandbox has
// no network). The section states this explicitly. Brave / SerpApi are
// scaffolded backends and will report "not supported in this build" until wired.

import { useEffect, useState } from "react";
import {
  clearWebSearchProvider,
  getWebSearchConfig,
  setWebSearchProvider,
} from "../../ipc/commands";
import type { WebSearchConfigView, WebSearchKind } from "../../types";

/** Human labels for the provider dropdown; Tavily is the recommended default. */
const KIND_LABELS: Record<WebSearchKind, string> = {
  tavily: "Tavily (recommended)",
  brave: "Brave Search",
  serpApi: "SerpApi",
  custom: "Custom (your own endpoint)",
};

/** Dropdown order: Tavily first (the recommended default), Custom last. */
const KIND_ORDER: WebSearchKind[] = ["tavily", "brave", "serpApi", "custom"];

/** The default result cap the backend applies when none is entered. */
const DEFAULT_MAX_RESULTS = 5;

export function WebSearchSection() {
  const [kind, setKind] = useState<WebSearchKind>("tavily");
  const [secret, setSecret] = useState("");
  const [maxResults, setMaxResults] = useState<number>(DEFAULT_MAX_RESULTS);
  // The custom endpoint URL (the user's OWN provider). Only meaningful for the
  // `custom` kind; hydrated from config.baseUrl on mount and passed on save.
  const [endpointUrl, setEndpointUrl] = useState("");
  // The configured provider is the backend source of truth (rehydrated on mount
  // from `get_web_search_config`), so it survives unmount/remount instead of a
  // UI-only marker.
  const [config, setConfig] = useState<WebSearchConfigView | null>(null);
  const [error, setError] = useState<string | null>(null);

  // On mount, hydrate the configured provider from the backend so a provider
  // saved in a previous session is shown (and the form prefills its selection).
  useEffect(() => {
    getWebSearchConfig()
      .then((next) => {
        setConfig(next);
        if (next !== null) {
          setKind(next.kind);
          setMaxResults(next.maxResults);
          setEndpointUrl(next.baseUrl ?? "");
        }
      })
      .catch(() => setConfig(null));
  }, []);

  const save = () => {
    setError(null);
    if (secret === "") {
      setError("An API key is required.");
      return;
    }
    // The custom kind is the user's OWN endpoint, so its URL is required. Mirror
    // the empty-key guard with a client-side non-empty check before saving.
    const trimmedUrl = endpointUrl.trim();
    if (kind === "custom" && trimmedUrl === "") {
      setError("A search endpoint URL is required for a custom provider.");
      return;
    }
    // The key is stored server-side as an opaque SecretRef and never comes back;
    // the returned view reports only kind / hasApiKey / maxResults / baseUrl.
    // baseUrl is passed only for the custom kind (null otherwise).
    setWebSearchProvider(kind, secret, maxResults, kind === "custom" ? trimmedUrl : null)
      .then((next) => {
        setConfig(next);
        setKind(next.kind);
        setMaxResults(next.maxResults);
        setEndpointUrl(next.baseUrl ?? "");
        setSecret("");
      })
      .catch((err: unknown) => setError(String(err)));
  };

  const clear = () => {
    setError(null);
    clearWebSearchProvider()
      .then(() => {
        setConfig(null);
        setSecret("");
        setKind("tavily");
        setMaxResults(DEFAULT_MAX_RESULTS);
        setEndpointUrl("");
      })
      .catch((err: unknown) => setError(String(err)));
  };

  return (
    <section className="settings__panel" role="region" aria-label="Web Search">
      <h3 className="settings__section-title">Web Search</h3>
      <p className="settings__section-desc">
        Choose a web-search provider and add its API key so the composer&apos;s web-search toggle
        (🌐) can fetch results and add them as context before the model answers. The key is stored
        in the OS keychain and never displayed after saving; only an opaque reference is kept. Web
        search needs your own key and a live network connection, so it can only be verified on your
        build.
      </p>
      <div className="settings-form">
        <label>
          Provider
          <select
            aria-label="Web search provider"
            value={kind}
            onChange={(event) => setKind(event.target.value as WebSearchKind)}
          >
            {KIND_ORDER.map((option) => (
              <option key={option} value={option}>
                {KIND_LABELS[option]}
              </option>
            ))}
          </select>
        </label>
        {kind === "custom" && (
          <label>
            Search endpoint URL
            <input
              type="url"
              aria-label="Web search endpoint URL"
              value={endpointUrl}
              placeholder="https://your-search.example.com"
              required
              onChange={(event) => setEndpointUrl(event.target.value)}
            />
            <span className="settings__field-help" data-testid="web-search-endpoint-help">
              Your own search endpoint. It must expose a Tavily-compatible JSON <code>/search</code>{" "}
              API (a POST returning a <code>results</code> array of title / url / content).
            </span>
          </label>
        )}
        <label>
          API key
          <input
            type="password"
            aria-label="Web search API key"
            value={secret}
            placeholder="Paste the key"
            onChange={(event) => setSecret(event.target.value)}
          />
        </label>
        <label>
          Max results
          <input
            type="number"
            aria-label="Web search max results"
            min={1}
            max={20}
            value={maxResults}
            onChange={(event) => {
              const next = Number.parseInt(event.target.value, 10);
              setMaxResults(Number.isNaN(next) ? DEFAULT_MAX_RESULTS : next);
            }}
          />
          <span className="settings__field-help" data-testid="web-search-max-results-help">
            How many results to fetch per search (1-20).
          </span>
        </label>
        <button type="button" onClick={save}>
          Save
        </button>
        {config !== null && (
          <ul
            className="settings__list"
            data-testid="configured-web-search"
            aria-label="Configured web search"
          >
            <li data-testid={`web-search-configured-${config.kind}`}>
              <span className="settings__list-label">
                Key configured for <strong>{KIND_LABELS[config.kind]}</strong> ({config.maxResults}{" "}
                results)
                {config.kind === "custom" && config.baseUrl !== null && (
                  <>
                    {" "}
                    at{" "}
                    <strong data-testid="web-search-configured-endpoint">{config.baseUrl}</strong>
                  </>
                )}
                .
              </span>
              <button type="button" onClick={clear} data-testid="web-search-clear">
                Clear
              </button>
            </li>
          </ul>
        )}
        {error !== null && (
          <p className="settings__section-desc" role="alert">
            {error}
          </p>
        )}
      </div>
    </section>
  );
}
