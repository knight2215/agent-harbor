// EnumerationErrorModal (architecture.md Section 8.2 model selector diagnostics;
// FEAT-002 redesign).
//
// Replaces the always-on red ProviderEnumerationErrors text near the picker with
// a compact, unobtrusive WARNING ICON that appears in the composer control row
// ONLY when the providers store `errors` array is non-empty. Clicking the icon
// opens a small role="dialog" modal that lists each per-provider enumeration
// failure ({providerId, message}) and, when present, the store `lastError` (the
// display-safe message of a failed load()). The modal is dismissible.
//
// It reads the providers store directly (errors + lastError) so the composer
// only has to render <EnumerationErrorModal /> without threading the data.
// DISPLAY-SAFE: the messages come from ProviderError's Display and never carry
// secret material (Section 9.1 / 9.2).

import { useState } from "react";
import { useProvidersStore } from "../../state/providers";

export function EnumerationErrorModal() {
  const errors = useProvidersStore((s) => s.errors);
  const lastError = useProvidersStore((s) => s.lastError);
  const [open, setOpen] = useState(false);

  // Normalize defensively: an undefined/non-array `errors` (a store or IPC
  // shape resolved without the field) must not crash the `.length`/`.map` reads.
  const items = Array.isArray(errors) ? errors : [];

  // No per-provider errors AND no load failure => nothing to warn about, so no
  // icon renders at all (the zero-errors-no-icon invariant).
  if (items.length === 0 && (lastError === null || lastError === undefined)) {
    return null;
  }

  return (
    <>
      <button
        type="button"
        className="composer__icon composer__warning"
        aria-label="Show model load warnings"
        title="Model load warnings"
        onClick={() => setOpen(true)}
      >
        <span aria-hidden="true">⚠️</span>
      </button>
      {open && (
        <div className="ah-modal__backdrop" role="presentation" onClick={() => setOpen(false)}>
          <div
            className="ah-modal"
            role="dialog"
            aria-modal="true"
            aria-label="Model load warnings"
            onClick={(event) => event.stopPropagation()}
          >
            <header className="ah-modal__header">
              <h2 className="ah-modal__title">Model load warnings</h2>
              <button
                type="button"
                className="ah-modal__close"
                aria-label="Close"
                onClick={() => setOpen(false)}
              >
                <span aria-hidden="true">×</span>
              </button>
            </header>
            {lastError !== null && lastError !== undefined && (
              <p className="ah-modal__last-error" data-testid="modal-last-error">
                Last load failed: {lastError}
              </p>
            )}
            {items.length > 0 && (
              <ul className="ah-modal__list" aria-label="Provider errors">
                {items.map((error) => (
                  <li key={error.providerId} className="ah-modal__list-item">
                    Couldn&apos;t load models from <strong>{error.providerId}</strong>:{" "}
                    {error.message}
                  </li>
                ))}
              </ul>
            )}
          </div>
        </div>
      )}
    </>
  );
}
