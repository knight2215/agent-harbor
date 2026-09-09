// Providers & Keys settings section (FEAT-003).
//
// Registers a hosted CLOUD provider so its models are enumerated and appear
// under Cloud in the picker. The user picks a provider KIND from a dropdown
// (not a free-text id) and enters an API key; the form calls
// `set_cloud_provider` (ipc/commands.ts), which persists a REAL ProviderConfig
// row (correct domain::ProviderKind, stable per-kind id, api_key_ref = stored
// SecretRef) so the existing enumerate path picks it up. This replaces the old
// dead-end that only called `set_provider_secret` + a localStorage marker and
// never persisted a config row (the direct cause of "adding Kiro/Gemini keys
// does nothing").
//
// KIRO MAPPING (architecture decision, see spec FEAT-005): there is NO Kiro
// variant in domain::ProviderKind and adding one is out of scope. Kiro is an
// OpenAI-compatible endpoint, so the "Kiro (OpenAI-compatible)" choice maps to
// ProviderKind.genericOpenAI with a REQUIRED base URL (its OpenAI-compatible
// endpoint). The other cloud kinds default their base URL in the adapter, so
// the Base URL field is optional (and hidden) for them.
//
// SECRET HYGIENE (Section 9.1): the plaintext key flows IN and only an opaque
// SecretRef handle is stored server-side; no command returns key material, and
// this form NEVER displays or persists the key. The configured providers are
// the BACKEND source of truth (rehydrated on mount from `list_cloud_providers`),
// so there is no localStorage marker.

import { useEffect, useState } from "react";
import { ProviderEnumerationErrors } from "../model-selector/ProviderEnumerationErrors";
import { clearCloudProvider, listCloudProviders, setCloudProvider } from "../../ipc/commands";
import { useProvidersStore } from "../../state/providers";
import type { CloudProviderConfig } from "../../types";

/** The cloud provider kinds configurable here, in dropdown order. */
type CloudKind = "openAI" | "anthropic" | "gemini" | "bedrock" | "azure" | "genericOpenAI";

/** Human labels for the dropdown; "Kiro" is surfaced as the genericOpenAI kind. */
const KIND_LABELS: Record<CloudKind, string> = {
  openAI: "OpenAI",
  anthropic: "Anthropic",
  gemini: "Gemini",
  bedrock: "Bedrock",
  azure: "Azure",
  genericOpenAI: "Kiro (OpenAI-compatible)",
};

const KIND_ORDER: CloudKind[] = [
  "openAI",
  "anthropic",
  "gemini",
  "bedrock",
  "azure",
  "genericOpenAI",
];

export function ProviderKeysSection() {
  const [kind, setKind] = useState<CloudKind>("openAI");
  const [secret, setSecret] = useState("");
  const [baseUrl, setBaseUrl] = useState("");
  // The configured cloud providers are the backend source of truth (rehydrated
  // on mount from `list_cloud_providers`), so they survive unmount/remount
  // instead of a UI-only marker.
  const [providers, setProviders] = useState<CloudProviderConfig[]>([]);
  // `warning` holds the last non-blocking base-url advisory (e.g. a non-loopback
  // plaintext http:// Kiro endpoint recommending TLS); `error` holds a rejection.
  const [warning, setWarning] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  // Kiro (genericOpenAI) has no default endpoint, so its base URL is required
  // and shown; the other cloud kinds default it in the adapter (field hidden).
  const requiresBaseUrl = kind === "genericOpenAI";

  // Surface per-provider enumeration failures from the providers store so a user
  // who just added a key sees WHY a provider still contributed no models
  // (display-safe; never secret material). Refresh on mount so the list reflects
  // the current provider configuration when this section is opened.
  const enumerationErrors = useProvidersStore((s) => s.errors);
  const loadProviders = useProvidersStore((s) => s.load);

  // On mount, hydrate the configured cloud providers from the backend (so a
  // provider saved in a previous session / before navigating away is shown) and
  // re-enumerate so any enumeration error surfaces.
  useEffect(() => {
    listCloudProviders()
      .then((next) => setProviders(Array.isArray(next) ? next : []))
      .catch(() => setProviders([]));
    void loadProviders();
  }, [loadProviders]);

  // Merge a freshly-configured provider into the list, replacing any existing
  // row for the same kind (the backend upserts by a stable per-kind id).
  const upsertProvider = (config: CloudProviderConfig) => {
    setProviders((prev) => [...prev.filter((entry) => entry.kind !== config.kind), config]);
  };

  const save = () => {
    setWarning(null);
    setError(null);
    if (secret === "") {
      setError("An API key is required.");
      return;
    }
    const trimmedBaseUrl = baseUrl.trim();
    if (requiresBaseUrl && trimmedBaseUrl === "") {
      setError("A base URL is required for a Kiro (OpenAI-compatible) provider.");
      return;
    }
    // A blocked/invalid base URL (or any backend validation error) surfaces in
    // the error region; an accepted plaintext non-loopback base URL resolves
    // with a non-blocking warning recommending TLS. The key is stored
    // server-side as an opaque SecretRef and never comes back.
    setCloudProvider(kind, secret, trimmedBaseUrl === "" ? null : trimmedBaseUrl)
      .then((config) => {
        upsertProvider(config);
        setSecret("");
        if (config.warning !== null) setWarning(config.warning);
        // Re-enumerate so any error from the just-configured provider surfaces.
        void loadProviders();
      })
      .catch((err: unknown) => setError(String(err)));
  };

  // Prefill the form from a configured provider so re-saving edits the same row.
  const editProvider = (config: CloudProviderConfig) => {
    setKind(config.kind as CloudKind);
    setBaseUrl(config.baseUrl ?? "");
    setSecret("");
    setWarning(null);
    setError(null);
  };

  // Remove a configured provider, then refresh the list from the backend.
  const clearProvider = (target: CloudKind) => {
    setWarning(null);
    setError(null);
    clearCloudProvider(target)
      .then(() => listCloudProviders())
      .then((next) => setProviders(Array.isArray(next) ? next : []))
      .then(() => loadProviders())
      .catch((err: unknown) => setError(String(err)));
  };

  return (
    <section className="settings__panel" role="region" aria-label="Providers & Keys">
      <h3 className="settings__section-title">Providers &amp; Keys</h3>
      <p className="settings__section-desc">
        Add a hosted provider API key so its models appear under Cloud. The key is stored in the OS
        keychain and never displayed after saving; only an opaque reference is kept.
      </p>
      <div className="settings-form">
        <label>
          Provider
          <select value={kind} onChange={(event) => setKind(event.target.value as CloudKind)}>
            {KIND_ORDER.map((option) => (
              <option key={option} value={option}>
                {KIND_LABELS[option]}
              </option>
            ))}
          </select>
        </label>
        {requiresBaseUrl && (
          <label>
            Base URL
            <input
              type="text"
              value={baseUrl}
              placeholder="https://your-kiro-endpoint/v1"
              aria-label="Base URL"
              onChange={(event) => setBaseUrl(event.target.value)}
            />
          </label>
        )}
        <label>
          API key
          <input
            type="password"
            value={secret}
            placeholder="Paste the key"
            onChange={(event) => setSecret(event.target.value)}
          />
        </label>
        <button type="button" onClick={save}>
          Save key
        </button>
        {providers.length > 0 && (
          <ul
            className="settings__list"
            data-testid="configured-providers"
            aria-label="Configured providers"
          >
            {providers.map((entry) => (
              <li key={entry.id} data-testid={`provider-key-configured-${entry.kind}`}>
                <span className="settings__list-label">
                  Key configured for <strong>{KIND_LABELS[entry.kind as CloudKind]}</strong>
                  {entry.baseUrl !== null ? ` at ${entry.baseUrl}` : ""}.
                </span>
                <button
                  type="button"
                  onClick={() => editProvider(entry)}
                  data-testid={`provider-key-edit-${entry.kind}`}
                >
                  Edit
                </button>
                <button
                  type="button"
                  onClick={() => clearProvider(entry.kind as CloudKind)}
                  data-testid={`provider-key-clear-${entry.kind}`}
                >
                  Clear
                </button>
              </li>
            ))}
          </ul>
        )}
        {warning !== null && (
          <p className="settings__section-desc" role="status" data-testid="cloud-provider-warning">
            {warning}
          </p>
        )}
        {error !== null && (
          <p className="settings__section-desc" role="alert">
            {error}
          </p>
        )}
        <ProviderEnumerationErrors errors={enumerationErrors} />
      </div>
    </section>
  );
}
