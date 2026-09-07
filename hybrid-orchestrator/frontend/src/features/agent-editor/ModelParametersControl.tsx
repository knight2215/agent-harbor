// ModelParametersControl (architecture.md Section 8.4 agent editor).
//
// Edits a persona's `ModelParameters`: temperature, maxTokens, topP,
// frequencyPenalty, presencePenalty, and stop sequences. Every numeric field is
// optional (null clears it); `stop` is edited as a newline-separated list.
// Presentational: takes the current parameters + onChange through props.

import type { ModelParameters } from "../../types";

export interface ModelParametersControlProps {
  /** The current model parameters. */
  value: ModelParameters;
  /** Called with the next parameters when a field changes. */
  onChange: (parameters: ModelParameters) => void;
}

/** Parse a numeric input value into a number, or null when empty/invalid. */
function parseNumber(raw: string): number | null {
  if (raw.trim().length === 0) return null;
  const parsed = Number(raw);
  return Number.isNaN(parsed) ? null : parsed;
}

/** The single-value numeric fields of ModelParameters. */
type NumericField = "temperature" | "maxTokens" | "topP" | "frequencyPenalty" | "presencePenalty";

const NUMERIC_FIELDS: { key: NumericField; label: string; step: string }[] = [
  { key: "temperature", label: "Temperature", step: "0.1" },
  { key: "maxTokens", label: "Max tokens", step: "1" },
  { key: "topP", label: "Top P", step: "0.1" },
  { key: "frequencyPenalty", label: "Frequency penalty", step: "0.1" },
  { key: "presencePenalty", label: "Presence penalty", step: "0.1" },
];

export function ModelParametersControl({ value, onChange }: ModelParametersControlProps) {
  const setNumeric = (key: NumericField, raw: string) => {
    onChange({ ...value, [key]: parseNumber(raw) });
  };

  const setStop = (raw: string) => {
    const stop = raw
      .split("\n")
      .map((line) => line.trim())
      .filter((line) => line.length > 0);
    onChange({ ...value, stop: stop.length === 0 ? null : stop });
  };

  return (
    <fieldset className="model-parameters" aria-label="Model parameters">
      <legend>Model parameters</legend>
      {NUMERIC_FIELDS.map((field) => (
        <label key={field.key} className="model-parameters__field">
          <span>{field.label}</span>
          <input
            type="number"
            step={field.step}
            aria-label={field.label}
            value={value[field.key] ?? ""}
            onChange={(e) => setNumeric(field.key, e.target.value)}
          />
        </label>
      ))}
      <label className="model-parameters__field">
        <span>Stop sequences</span>
        <textarea
          aria-label="Stop sequences"
          value={(value.stop ?? []).join("\n")}
          onChange={(e) => setStop(e.target.value)}
        />
      </label>
    </fieldset>
  );
}
