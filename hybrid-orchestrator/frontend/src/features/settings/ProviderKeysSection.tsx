// Providers & Keys settings section (FEAT-003).
//
// A thin wrapper over the existing `set_provider_secret` command
// (ipc/commands.ts). The plaintext key flows IN and only an opaque SecretRef
// handle comes back (architecture.md Section 9.1); this form NEVER displays or
// stores the key. It is the sanctioned UI entry point for the provider-secret
// path that previously had no surface.
//
// PERSISTED "configured" MARKER: the backend has no command to query which
// providers already hold a key (the only secret command is `set_provider_secret`,
// which writes the plaintext and returns an opaque `SecretRef` handle). So the
// "a key is configured for provider X" indication is kept alive across
// navigation by persisting a NON-SECRET marker: a map of provider id -> its
// opaque `SecretRef` handle. The ref is explicitly non-secret (it never carries
// key material per the command's own contract), and the plaintext key is never
// stored anywhere. On mount we reload this map so the indicator re-renders even
// after the component was unmounted (e.g. navigating to Chat and back). We
// mirror app.tsx's guarded `window.localStorage` try/catch pattern so a
// constrained/private-mode environment degrades gracefully instead of throwing.

import { useState } from "react";
import { setProviderSecret } from "../../ipc/commands";
import type { SecretRef } from "../../types";

/** localStorage key persisting the non-secret map of configured provider refs. */
const CONFIGURED_PROVIDERS_KEY = "ah-configured-providers";

/** A provider id mapped to the opaque (non-secret) SecretRef handle it stored. */
type ConfiguredProviders = Record<string, SecretRef>;

/** Read the persisted configured-providers map (guarded for constrained envs). */
function readConfiguredProviders(): ConfiguredProviders {
  try {
    const raw = window.localStorage.getItem(CONFIGURED_PROVIDERS_KEY);
    if (raw === null) return {};
    const parsed: unknown = JSON.parse(raw);
    if (typeof parsed !== "object" || parsed === null) return {};
    // Keep only string->string entries; ignore anything malformed.
    const result: ConfiguredProviders = {};
    for (const [id, ref] of Object.entries(parsed as Record<string, unknown>)) {
      if (typeof ref === "string") result[id] = ref;
    }
    return result;
  } catch {
    return {};
  }
}

/** Persist the configured-providers map (guarded; never stores key material). */
function writeConfiguredProviders(map: ConfiguredProviders): void {
  try {
    window.localStorage.setItem(CONFIGURED_PROVIDERS_KEY, JSON.stringify(map));
  } catch {
    // Ignore storage write errors (private mode / constrained env).
  }
}

export function ProviderKeysSection() {
  const [providerId, setProviderId] = useState("");
  const [secret, setSecret] = useState("");
  const [configured, setConfigured] = useState<ConfiguredProviders>(readConfiguredProviders);
  const [error, setError] = useState<string | null>(null);

  const save = () => {
    setError(null);
    const id = providerId.trim();
    if (id === "" || secret === "") {
      setError("Provider id and key are required.");
      return;
    }
    setProviderSecret(id, secret)
      .then((ref) => {
        // Persist a NON-SECRET marker so the "configured" indication survives
        // navigation (the backend exposes no query for this). Never retain the
        // plaintext key.
        setConfigured((prev) => {
          const next = { ...prev, [id]: ref };
          writeConfiguredProviders(next);
          return next;
        });
        setSecret("");
      })
      .catch(() => setError("Failed to store the key."));
  };

  const configuredEntries = Object.entries(configured);

  return (
    <section className="settings__panel" role="region" aria-label="Providers & Keys">
      <h3 className="settings__section-title">Providers &amp; Keys</h3>
      <p className="settings__section-desc">
        Store a provider API key in the OS keychain. The key is never displayed after saving; only
        an opaque reference is kept.
      </p>
      <div className="settings-form">
        <label>
          Provider id
          <input
            type="text"
            value={providerId}
            placeholder="e.g. openai"
            onChange={(event) => setProviderId(event.target.value)}
          />
        </label>
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
        {configuredEntries.length > 0 && (
          <ul
            className="settings__section-desc"
            data-testid="configured-providers"
            aria-label="Configured providers"
          >
            {configuredEntries.map(([id, ref]) => (
              <li key={id} data-testid={`provider-key-configured-${id}`}>
                Key configured for <strong>{id}</strong> (reference: {ref}).
              </li>
            ))}
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
