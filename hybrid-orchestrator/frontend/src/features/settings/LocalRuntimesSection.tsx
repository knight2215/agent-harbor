// Local Runtimes settings section (FEAT-003).
//
// Configures locally-hosted, OpenAI-compatible runtimes: LM Studio and a
// generic OpenAI-compatible endpoint. Both are just a provider secret + base
// URL from the frontend's perspective, so this reuses the existing
// `set_provider_secret` command (ipc/commands.ts); the base URL is a display
// value the user records for their own configuration.
//
// It also renders a clearly-labeled EMPTY-STATE placeholder for FUTURE runtimes
// (Ollama / Hugging Face). Those are intentionally NOT implemented here.

import { useState } from "react";
import { setProviderSecret } from "../../ipc/commands";

type LocalKind = "lmStudio" | "genericOpenAI";

const KIND_LABELS: Record<LocalKind, string> = {
  lmStudio: "LM Studio",
  genericOpenAI: "Generic OpenAI-compatible",
};

export function LocalRuntimesSection() {
  const [kind, setKind] = useState<LocalKind>("lmStudio");
  const [baseUrl, setBaseUrl] = useState("http://localhost:1234/v1");
  const [secret, setSecret] = useState("");
  const [saved, setSaved] = useState(false);

  const save = () => {
    setSaved(false);
    // Local runtimes often accept any key; store whatever the user provided so
    // the OpenAI-compatible adapter can authenticate if the endpoint requires it.
    setProviderSecret(kind, secret === "" ? "local" : secret)
      .then(() => {
        setSaved(true);
        setSecret("");
      })
      .catch(() => setSaved(false));
  };

  return (
    <section className="settings__panel" role="region" aria-label="Local Runtimes">
      <h3 className="settings__section-title">Local Runtimes</h3>
      <p className="settings__section-desc">
        Connect a locally-hosted, OpenAI-compatible runtime such as LM Studio or any generic
        OpenAI-compatible endpoint.
      </p>
      <div className="settings-form">
        <label>
          Runtime
          <select value={kind} onChange={(event) => setKind(event.target.value as LocalKind)}>
            <option value="lmStudio">{KIND_LABELS.lmStudio}</option>
            <option value="genericOpenAI">{KIND_LABELS.genericOpenAI}</option>
          </select>
        </label>
        <label>
          Base URL
          <input
            type="text"
            value={baseUrl}
            placeholder="http://localhost:1234/v1"
            onChange={(event) => setBaseUrl(event.target.value)}
          />
        </label>
        <label>
          API key (optional)
          <input
            type="password"
            value={secret}
            placeholder="Leave blank for keyless local endpoints"
            onChange={(event) => setSecret(event.target.value)}
          />
        </label>
        <button type="button" onClick={save}>
          Save runtime
        </button>
        {saved && (
          <p className="settings__section-desc" data-testid="local-runtime-saved">
            {KIND_LABELS[kind]} runtime saved.
          </p>
        )}
      </div>

      <div
        className="settings__empty-state"
        role="region"
        aria-label="More local runtimes coming soon"
        data-testid="local-runtimes-empty-state"
      >
        <p>
          More local runtimes are coming soon. Support for <strong>Ollama</strong> and{" "}
          <strong>Hugging Face</strong> will appear here.
        </p>
      </div>
    </section>
  );
}
