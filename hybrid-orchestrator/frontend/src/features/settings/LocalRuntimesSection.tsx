// Local Runtimes settings section (FEAT-003).
//
// Configures locally-hosted runtimes. Two shapes are supported:
//
//  1. OpenAI-compatible endpoints (LM Studio and a generic OpenAI-compatible
//     endpoint). The entered base URL now genuinely configures the provider: it
//     is persisted into a real ProviderConfig via the `set_local_runtime`
//     command (ipc/commands.ts), which validates the URL through the base-url
//     posture (surfacing a block as a rejection and an accepted plaintext
//     non-loopback URL as a non-blocking warning) and, when a key is supplied,
//     stores it as an opaque SecretRef. The configured runtime(s) are rehydrated
//     on mount from `list_local_runtimes` so they persist across navigation, and
//     can be edited (re-save upserts) or cleared via `clear_local_runtime`.
//
//  2. The embedded inference engine (Strategy B): an in-process llama.cpp
//     runtime that runs a local `.gguf` file with no separate install. It is a
//     first-class local provider behind the same ChatProvider contract, wired
//     here to the embedded model lifecycle commands (list/import/select/load/
//     unload/status) from ipc/commands.ts. Note: select/load record the ACTIVE
//     model selection (the id routed to); the `.gguf` is read into memory
//     lazily on the first chat, so the UI says "Active model" / "Select" rather
//     than implying the file is already resident.
//
// Ollama already ships as a Local provider (Phase 8) and the embedded engine
// ships now, so neither is "coming soon"; the informational note at the bottom
// only lists genuinely-future runtimes (Hugging Face).

import { open } from "@tauri-apps/plugin-dialog";
import { useEffect, useState } from "react";
import {
  clearLocalRuntime,
  embeddedModelStatus,
  importEmbeddedModel,
  listEmbeddedModels,
  listLocalRuntimes,
  loadEmbeddedModel,
  selectEmbeddedModel,
  setLocalRuntime,
  unloadEmbeddedModel,
} from "../../ipc/commands";
import type { EmbeddedModelStatus, EmbeddedModelView, LocalRuntimeConfig } from "../../types";
import { adviseGenericOpenAiBaseUrl } from "./baseUrlAdvisory";

type LocalKind = "lmStudio" | "genericOpenAI";

const KIND_LABELS: Record<LocalKind, string> = {
  lmStudio: "LM Studio",
  genericOpenAI: "Generic OpenAI-compatible",
};

export function LocalRuntimesSection() {
  const [kind, setKind] = useState<LocalKind>("lmStudio");
  const [baseUrl, setBaseUrl] = useState("http://localhost:1234/v1");
  const [secret, setSecret] = useState("");
  // The configured runtimes are the backend source of truth (rehydrated on
  // mount from `list_local_runtimes`), so they survive unmount/remount instead
  // of a transient "saved" flag. `warning` holds the last non-blocking base-url
  // advisory; `error` holds a rejection (e.g. a blocked URL).
  const [runtimes, setRuntimes] = useState<LocalRuntimeConfig[]>([]);
  const [warning, setWarning] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  // Embedded engine state. `models` is the imported `.gguf` list; `status`
  // reflects which one (if any) is currently loaded; `error` surfaces the last
  // failed lifecycle call.
  const [ggufPath, setGgufPath] = useState("");
  const [embeddedModels, setEmbeddedModels] = useState<EmbeddedModelView[]>([]);
  const [status, setStatus] = useState<EmbeddedModelStatus | null>(null);
  const [embeddedError, setEmbeddedError] = useState<string | null>(null);

  // On mount, hydrate the configured local runtimes from the backend (so a
  // runtime saved in a previous session / before navigating away is shown), and
  // hydrate BOTH the imported-models list and the active-selection status so
  // embedded models imported in a previous session are listed and the status
  // line reflects the current active selection (not just imports made this
  // session).
  useEffect(() => {
    listLocalRuntimes()
      .then((next) => setRuntimes(Array.isArray(next) ? next : []))
      .catch(() => setRuntimes([]));
    listEmbeddedModels()
      .then((next) => setEmbeddedModels(Array.isArray(next) ? next : []))
      .catch(() => setEmbeddedModels([]));
    embeddedModelStatus()
      .then((next) => setStatus(next))
      .catch(() => setStatus(null));
  }, []);

  // Merge a freshly-configured runtime into the list, replacing any existing
  // row for the same kind (the backend upserts by a stable per-kind id).
  const upsertRuntime = (config: LocalRuntimeConfig) => {
    setRuntimes((prev) => [...prev.filter((entry) => entry.kind !== config.kind), config]);
  };

  const save = () => {
    setWarning(null);
    setError(null);
    // The entered base URL now genuinely configures the provider: a blocked URL
    // rejects (shown as an error) and an accepted plaintext non-loopback URL
    // resolves with a non-blocking warning. Keyless local endpoints send a null
    // api key; a non-empty key is stored as an opaque SecretRef.
    setLocalRuntime(kind, baseUrl, secret === "" ? null : secret)
      .then((config) => {
        upsertRuntime(config);
        setSecret("");
        if (config.warning !== null) setWarning(config.warning);
      })
      .catch((err: unknown) => setError(String(err)));
  };

  // Prefill the form from a configured runtime so re-saving edits the same row.
  const editRuntime = (config: LocalRuntimeConfig) => {
    setKind(config.kind as LocalKind);
    setBaseUrl(config.baseUrl);
    setSecret("");
    setWarning(null);
    setError(null);
  };

  // Remove a configured runtime, then refresh the list from the backend.
  const clearRuntime = (target: LocalKind) => {
    setWarning(null);
    setError(null);
    clearLocalRuntime(target)
      .then(() => listLocalRuntimes())
      .then((next) => setRuntimes(Array.isArray(next) ? next : []))
      .catch((err: unknown) => setError(String(err)));
  };

  // Open the native OS file-open dialog (Explorer/Finder/GTK/Dolphin) filtered
  // to `.gguf`, and drop the chosen absolute path into the existing `ggufPath`
  // state so the Import/Select buttons work unchanged. `open()` resolves to the
  // selected path string (single-file mode), or null when the user cancels
  // (a no-op). `multiple` is false so the string[] branch of the return type
  // never occurs, but we narrow defensively to keep TypeScript strict-clean.
  const browseForModel = async () => {
    setEmbeddedError(null);
    try {
      const selected = await open({
        multiple: false,
        directory: false,
        filters: [{ name: "GGUF model", extensions: ["gguf"] }],
      });
      if (typeof selected === "string") {
        setGgufPath(selected);
      }
    } catch (err: unknown) {
      setEmbeddedError(String(err));
    }
  };

  // Import (register) the entered `.gguf` path with the embedded engine and
  // refresh the imported-models list.
  const importModel = () => {
    setEmbeddedError(null);
    importEmbeddedModel(ggufPath)
      .then((next) => {
        setEmbeddedModels(Array.isArray(next) ? next : []);
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
        setEmbeddedModels(Array.isArray(next) ? next : []);
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

  // A live, NON-BLOCKING advisory for the entered base URL, shown only for the
  // generic OpenAI-compatible runtime (LM Studio's default is a plain API root).
  // Guidance only; it never blocks the save (the backend attaches its own
  // advisory and remains the enforcement point).
  const baseUrlAdvisory =
    kind === "genericOpenAI"
      ? adviseGenericOpenAiBaseUrl(
          baseUrl,
          "Enter the API root (e.g. http://localhost:1234/v1). For Gemini, use the Gemini kind under Providers & Keys instead of the generic OpenAI-compatible runtime.",
        )
      : null;

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
            aria-label="Base URL"
            onChange={(event) => setBaseUrl(event.target.value)}
          />
          <span className="settings__field-help" data-testid="local-base-url-help">
            OpenAI-compatible API base URL, e.g. http://localhost:1234/v1 - not a full
            model/generateContent URL. For Gemini, use the Gemini kind under Providers &amp; Keys.
          </span>
        </label>
        {baseUrlAdvisory !== null && (
          <p
            className="settings__section-desc"
            role="status"
            data-testid="local-runtime-base-url-advisory"
          >
            {baseUrlAdvisory}
          </p>
        )}
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
        {warning !== null && (
          <p className="settings__section-desc" role="status" data-testid="local-runtime-warning">
            {warning}
          </p>
        )}
        {error !== null && (
          <p className="settings__section-error" role="alert" data-testid="local-runtime-error">
            {error}
          </p>
        )}
        {runtimes.length > 0 && (
          <ul
            className="settings__list"
            data-testid="local-runtime-saved"
            aria-label="Configured local runtimes"
          >
            {runtimes.map((entry) => (
              <li key={entry.id} data-testid={`local-runtime-configured-${entry.kind}`}>
                <span className="settings__list-label">
                  {KIND_LABELS[entry.kind as LocalKind]} configured at {entry.baseUrl}
                </span>
                <button
                  type="button"
                  onClick={() => editRuntime(entry)}
                  data-testid={`local-runtime-edit-${entry.kind}`}
                >
                  Edit
                </button>
                <button
                  type="button"
                  onClick={() => clearRuntime(entry.kind as LocalKind)}
                  data-testid={`local-runtime-clear-${entry.kind}`}
                >
                  Clear
                </button>
              </li>
            ))}
          </ul>
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
          separate install is required; the model is treated as a Local provider. Selecting a model
          marks it active for routing; the file is read into memory on the first chat, so a missing
          or unreadable file surfaces then.
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
              onClick={() => void browseForModel()}
              aria-label="Browse for embedded model file"
              data-testid="embedded-model-browse"
            >
              Browse…
            </button>
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
              Select model
            </button>
            <button
              type="button"
              onClick={unloadModel}
              disabled={loadedModelId === null}
              data-testid="embedded-unload"
            >
              Clear selection
            </button>
          </div>
          <p className="settings__section-desc" data-testid="embedded-status">
            {loadedModelId !== null
              ? `Active model: ${loadedModelId}`
              : "No embedded model is selected."}
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
                    Select
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
