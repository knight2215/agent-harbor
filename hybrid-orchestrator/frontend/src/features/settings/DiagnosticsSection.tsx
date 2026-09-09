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

import { useEffect, useState } from "react";
import { providerDiagnostics } from "../../ipc/commands";
import type { ProviderDiagnosticsReport } from "../../types";

export function DiagnosticsSection() {
  // `report` is the last successful readout; `loading` guards the summary line
  // while an invocation is in flight; `error` captures an IPC throw so the panel
  // never renders blank.
  const [report, setReport] = useState<ProviderDiagnosticsReport | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

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
