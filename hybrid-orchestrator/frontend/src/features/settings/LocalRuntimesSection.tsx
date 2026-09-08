// Local Runtimes settings section (FEAT-003).
//
// Configures locally-hosted runtimes. Two shapes are supported:
//
//  1. OpenAI-compatible endpoints (LM Studio and a generic OpenAI-compatible
//     endpoint). From the frontend's perspective each is just a provider secret
//     + base URL, so this reuses the existing `set_provider_secret` command
//     (ipc/commands.ts); the base URL is a display value the user records for
//     their own configuration.
//
//  2. The embedded inference engine (Strategy B): an in-process llama.cpp
//     runtime that runs a local `.gguf` file with no separate install. It is a
//     first-class local provider behind the same ChatProvider contract, wired
//     here to the embedded model lifecycle commands (list/import/select/load/
//     unload/status) from ipc/commands.ts.
//
// Ollama already ships as a Local provider (Phase 8) and the embedded engine
// ships now, so neither is "coming soon"; the informational note at the bottom
// only lists genuinely-future runtimes (Hugging Face).

import { useEffect, useState } from "react";
import {
  embeddedModelStatus,
  importEmbeddedModel,
  loadEmbeddedModel,
  selectEmbeddedModel,
  setProviderSecret,
  unloadEmbeddedModel,
} from "../../ipc/commands";
import type { EmbeddedModelStatus, EmbeddedModelView } from "../../types";

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

  // Embedded engine state. `models` is the imported `.gguf` list; `status`
  // reflects which one (if any) is currently loaded; `error` surfaces the last
  // failed lifecycle call.
  const [ggufPath, setGgufPath] = useState("");
  const [embeddedModels, setEmbeddedModels] = useState<EmbeddedModelView[]>([]);
  const [status, setStatus] = useState<EmbeddedModelStatus | null>(null);
  const [embeddedError, setEmbeddedError] = useState<string | null>(null);

  // Load the current embedded status once on mount so the status line reflects
  // any model already loaded from a previous session.
  useEffect(() => {
    embeddedModelStatus()
      .then((next) => setStatus(next))
      .catch(() => setStatus(null));
  }, []);

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

  // Import (register) the entered `.gguf` path with the embedded engine and
  // refresh the imported-models list.
  const importModel = () => {
    setEmbeddedError(null);
    importEmbeddedModel(ggufPath)
      .then((next) => {
        setEmbeddedModels(next);
        setGgufPath("");
      })
      .catch((err: unknown) => setEmbeddedError(String(err)));
  };

  // Select the entered `.gguf` path: import if needed, then mark it active. The
  // refreshed list drives which entry shows as loaded; refresh the status line.
  const loadPath = () => {
    setEmbeddedError(null);
    selectEmbeddedModel(ggufPath)
      .then((next) => {
        setEmbeddedModels(next);
        setGgufPath("");
        return embeddedModelStatus();
      })
      .then((next) => setStatus(next))
      .catch((err: unknown) => setEmbeddedError(String(err)));
  };

  // Load an already-imported model by id and reflect the returned status.
  const loadModel = (modelId: string) => {
    setEmbeddedError(null);
    loadEmbeddedModel(modelId)
      .then((next) => setStatus(next))
      .catch((err: unknown) => setEmbeddedError(String(err)));
  };

  // Unload the active model and reflect the cleared status.
  const unloadModel = () => {
    setEmbeddedError(null);
    unloadEmbeddedModel()
      .then((next) => setStatus(next))
      .catch((err: unknown) => setEmbeddedError(String(err)));
  };

  const loadedModelId = status?.loadedModelId ?? null;

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
        className="settings__subpanel"
        role="region"
        aria-label="Embedded inference engine"
        data-testid="embedded-engine-section"
      >
        <h4 className="settings__section-title">Embedded engine</h4>
        <p className="settings__section-desc">
          Run a local <code>.gguf</code> model in-process with the built-in llama.cpp engine. No
          separate install is required; the model is treated as a Local provider.
        </p>
        <div className="settings-form">
          <label>
            Model file
            <input
              type="text"
              value={ggufPath}
              placeholder="/path/to/model.gguf"
              aria-label="Embedded model path"
              data-testid="embedded-model-path"
              onChange={(event) => setGgufPath(event.target.value)}
            />
          </label>
          <div className="settings-form__actions">
            <button
              type="button"
              onClick={importModel}
              disabled={ggufPath === ""}
              data-testid="embedded-import"
            >
              Import model
            </button>
            <button
              type="button"
              onClick={loadPath}
              disabled={ggufPath === ""}
              data-testid="embedded-load"
            >
              Load model
            </button>
            <button
              type="button"
              onClick={unloadModel}
              disabled={loadedModelId === null}
              data-testid="embedded-unload"
            >
              Unload model
            </button>
          </div>
          <p className="settings__section-desc" data-testid="embedded-status">
            {loadedModelId !== null
              ? `Loaded model: ${loadedModelId}`
              : "No embedded model is loaded."}
          </p>
          {embeddedError !== null && (
            <p className="settings__section-error" role="alert" data-testid="embedded-error">
              {embeddedError}
            </p>
          )}
          {embeddedModels.length > 0 && (
            <ul className="settings__list" data-testid="embedded-model-list">
              {embeddedModels.map((entry) => (
                <li key={entry.id}>
                  <span className="settings__list-label">
                    {entry.id} ({entry.path})
                  </span>
                  <button
                    type="button"
                    onClick={() => loadModel(entry.id)}
                    data-testid={`embedded-load-${entry.id}`}
                  >
                    Load
                  </button>
                </li>
              ))}
            </ul>
          )}
        </div>
      </div>

      <div
        className="settings__empty-state"
        role="region"
        aria-label="More local runtimes coming soon"
        data-testid="local-runtimes-empty-state"
      >
        <p>
          LM Studio, <strong>Ollama</strong>, and the built-in <strong>embedded engine</strong> are
          available as Local providers. Support for <strong>Hugging Face</strong> is coming soon.
        </p>
      </div>
    </section>
  );
}
