// Providers & Keys settings section (FEAT-003).
//
// A thin wrapper over the existing `set_provider_secret` command
// (ipc/commands.ts). The plaintext key flows IN and only an opaque SecretRef
// handle comes back (architecture.md Section 9.1); this form NEVER displays or
// stores the key. It is the sanctioned UI entry point for the provider-secret
// path that previously had no surface.

import { useState } from "react";
import { setProviderSecret } from "../../ipc/commands";

export function ProviderKeysSection() {
  const [providerId, setProviderId] = useState("");
  const [secret, setSecret] = useState("");
  const [savedRef, setSavedRef] = useState<string | null>(null);
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
        setSavedRef(ref);
        // Never retain the plaintext key in component state.
        setSecret("");
      })
      .catch(() => setError("Failed to store the key."));
  };

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
        {savedRef !== null && (
          <p className="settings__section-desc" data-testid="provider-key-saved">
            Key stored (reference: {savedRef}).
          </p>
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
