// RoutingHintControl (architecture.md Section 8.4 agent editor).
//
// Picks a persona's coarse `RoutingHint` (preferLocal / preferQuality /
// preferCheap / preferSpeed), or none. Presentational: takes the current value
// and an onChange through props.

import type { RoutingHint } from "../../types";

const HINTS: { value: RoutingHint; label: string }[] = [
  { value: "preferLocal", label: "Prefer local" },
  { value: "preferQuality", label: "Prefer quality" },
  { value: "preferCheap", label: "Prefer cheap" },
  { value: "preferSpeed", label: "Prefer speed" },
];

export interface RoutingHintControlProps {
  /** The current routing hint, or null for none. */
  value: RoutingHint | null;
  /** Called with the chosen hint, or null when cleared. */
  onChange: (hint: RoutingHint | null) => void;
}

export function RoutingHintControl({ value, onChange }: RoutingHintControlProps) {
  return (
    <div className="routing-hint" role="radiogroup" aria-label="Routing hint">
      <button
        type="button"
        role="radio"
        aria-checked={value === null}
        data-active={value === null}
        onClick={() => onChange(null)}
      >
        None
      </button>
      {HINTS.map((hint) => (
        <button
          key={hint.value}
          type="button"
          role="radio"
          aria-checked={value === hint.value}
          data-active={value === hint.value}
          onClick={() => onChange(hint.value)}
        >
          {hint.label}
        </button>
      ))}
    </div>
  );
}
