// Diagnostics settings section (model-picker diagnostics self-report).
//
// An always-reachable, plain-text readout of what actually happened during
// model enumeration, so a user who sees "No models available yet" can copy a
// precise per-provider diagnosis instead of reporting "nothing". On mount (and
// on demand via Run diagnostics / Refresh) it calls the `provider_diagnostics`
// command and renders, per configured provider row, its id, kind, display-safe
// base URL, whether an instance was built, its model count, and any enumeration
// error, plus a one-line summary.
//
// DISPLAY-SAFE: the report carries only ids/kinds/base URLs/counts and provider
// error messages, never secret material (Section 9.1 / 9.2). It also catches an
// IPC throw from `provider_diagnostics` itself and shows `failed: <error>`
// rather than leaving a blank panel.
//
// It also hosts the keychain self-test (FEAT-002): a "Test key storage" button
// round-trips a sentinel through the OS keychain via `test_key_storage` and
// renders the display-safe `{ ok, detail }` result. This is the affordance that
// lets a real build (notably Windows, which the sandbox cannot exercise) confirm
// the v0.8.1 keychain-persistence bug is fixed. It likewise catches an IPC throw
// so the panel never blanks, and never surfaces secret material.

import { useEffect, useState } from "react";
import { providerDiagnostics, testKeyStorage } from "../../ipc/commands";
import type { KeyStorageTestResult, ProviderDiagnosticsReport } from "../../types";

export function DiagnosticsSection() {
  // `report` is the last successful readout; `loading` guards the summary line
  // while an invocation is in flight; `error` captures an IPC throw so the panel
  // never renders blank.
  const [report, setReport] = useState<ProviderDiagnosticsReport | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  // Keychain self-test state. `keyStorage` is the last `{ ok, detail }` result;
  // `keyStorageLoading` guards the button while the round-trip is in flight;
  // `keyStorageError` captures an IPC throw so the panel never renders blank.
  const [keyStorage, setKeyStorage] = useState<KeyStorageTestResult | null>(null);
  const [keyStorageLoading, setKeyStorageLoading] = useState(false);
  const [keyStorageError, setKeyStorageError] = useState<string | null>(null);

  const run = () => {
    setLoading(true);
    setError(null);
    providerDiagnostics()
      .then((next) => {
        setReport(next);
        setLoading(false);
      })
      .catch((err: unknown) => {
        setError(err instanceof Error ? err.message : String(err));
        setLoading(false);
      });
  };

  // Round-trip a sentinel through the OS keychain so a real build (notably
  // Windows) can confirm secrets actually persist. Catches an IPC throw so the
  // panel never blanks.
  const runKeyStorageTest = () => {
    setKeyStorageLoading(true);
    setKeyStorageError(null);
    testKeyStorage()
      .then((next) => {
        setKeyStorage(next);
        setKeyStorageLoading(false);
      })
      .catch((err: unknown) => {
        setKeyStorageError(err instanceof Error ? err.message : String(err));
        setKeyStorageLoading(false);
      });
  };

  // The display-safe key-storage status line, guarding every read of the
  // possibly-null result behind the loading/error/loaded state.
  const keyStorageSummary = keyStorageLoading
    ? "testing…"
    : keyStorageError !== null
      ? `failed: ${keyStorageError}`
      : keyStorage !== null
        ? `${keyStorage.ok ? "ok" : "failed"}: ${keyStorage.detail}`
        : "not tested yet";

  // Run once on mount so the section is populated as soon as it is opened.
  useEffect(() => {
    run();
  }, []);

  // Build the summary line, guarding every read of the possibly-null `report`
  // behind the loading/error/loaded state so a strict-null read can never occur.
  const summary = loading
    ? "loading…"
    : error !== null
      ? `failed: ${error}`
      : report !== null
        ? `loaded: ${report.totalModelCount} models from ${report.providerCountWithModels} of ${report.configuredCount} configured providers`
        : "no diagnostics available";

  const providers = report !== null && Array.isArray(report.providers) ? report.providers : [];

  return (
    <section className="settings__panel" role="region" aria-label="Diagnostics">
      <h3 className="settings__section-title">Diagnostics</h3>
      <p className="settings__section-desc">
        A per-provider readout of what happened during model enumeration. Copy this when reporting
        an empty model picker so the cause is precise. It never shows keys or secrets.
      </p>
      <p className="settings__section-desc" data-testid="diagnostics-summary">
        {summary}
      </p>
      <button
        type="button"
        onClick={run}
        aria-label="Run diagnostics"
        data-testid="diagnostics-refresh"
      >
        Run diagnostics
      </button>
      <div className="settings-form">
        <p className="settings__section-desc">
          Confirm this system&apos;s OS keychain (macOS Keychain, Windows Credential Manager, Linux
          Secret Service) actually persists secrets by round-tripping a sentinel value. Use this if
          a saved provider key later reports &quot;no secret found&quot;. It never shows keys or
          secrets.
        </p>
        <p className="settings__section-desc" data-testid="key-storage-summary">
          {keyStorageSummary}
        </p>
        <button
          type="button"
          onClick={runKeyStorageTest}
          disabled={keyStorageLoading}
          aria-label="Test key storage"
          data-testid="key-storage-test"
        >
          Test key storage
        </button>
      </div>
      {providers.length > 0 && (
        <ul
          className="settings__list"
          data-testid="diagnostics-providers"
          aria-label="Per-provider diagnostics"
        >
          {providers.map((provider) => (
            <li key={provider.id} data-testid={`diagnostics-provider-${provider.id}`}>
              <span className="settings__list-label">
                {provider.id} ({provider.kind}) at {provider.baseUrl ?? "default"}, instance built:{" "}
                {provider.instanceBuilt ? "yes" : "no"}, models: {provider.modelCount}
                {provider.error !== null ? `, error: ${provider.error}` : ""}
              </span>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
