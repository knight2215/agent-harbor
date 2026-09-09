// ProviderEnumerationErrors (architecture.md Section 8.2 model selector
// diagnostics).
//
// A small, token-styled presentational list of per-provider enumeration
// failures surfaced by `list_available_models`. It is rendered NEAR the picker
// (not instead of it) so a user whose provider is misconfigured or unreachable
// sees WHY it contributed no models, while any healthy providers still populate
// the picker. It renders nothing when there are no errors.
//
// DISPLAY-SAFE: the messages come from `ProviderError`'s Display and never carry
// secret material (Section 9.1 / 9.2).

import type { ProviderEnumerationError } from "../../types";

export interface ProviderEnumerationErrorsProps {
  /** The per-provider enumeration failures (from the providers store). */
  errors: ProviderEnumerationError[];
}

/** Render the per-provider enumeration errors, or nothing when the list is empty. */
export function ProviderEnumerationErrors({ errors }: ProviderEnumerationErrorsProps) {
  if (errors.length === 0) return null;
  return (
    <ul
      className="provider-enumeration-errors"
      data-testid="provider-enumeration-errors"
      aria-label="Provider errors"
    >
      {errors.map((error) => (
        <li key={error.providerId} className="provider-enumeration-errors__item">
          Couldn&apos;t load models from <strong>{error.providerId}</strong>: {error.message}
        </li>
      ))}
    </ul>
  );
}
