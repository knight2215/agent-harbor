// ProviderModelPicker (architecture.md Section 8.2 model selector).
//
// A REUSABLE, presentational picker over the available provider/model options.
// The chat model selector renders it (via PerMessageOverrideControl and the
// manual-pin path), and the agent editor's DefaultRoutePicker (FEAT-003)
// imports the very same component, so it takes its data + selection through
// props and owns no store state itself.
//
// Options are grouped into Local vs Cloud. `AvailableModel` does not carry the
// provider kind/baseUrl, so locality is decided by the optional `isLocal`
// predicate; it defaults to the zero-price heuristic (local providers price at
// zero per the `TokenPrice` contract, Section 6.2). Each option shows capability
// hints (from `Capabilities`) and a rough cost signal (from `TokenPrice`).

import type { AvailableModel, ManualRoute } from "../../types";
import { NoModelsEmptyState } from "./NoModelsEmptyState";

/** True when both token rates are zero, the default "this is a local model" signal. */
function isFreeModel(model: AvailableModel): boolean {
  return model.price.inputPerMtok === 0 && model.price.outputPerMtok === 0;
}

/** Compact capability labels for an option (e.g. "tools", "vision"). */
function capabilityLabels(model: AvailableModel): string[] {
  const caps = model.capabilities;
  const labels: string[] = [];
  if (caps.streaming) labels.push("streaming");
  if (caps.tools) labels.push("tools");
  if (caps.vision) labels.push("vision");
  if (caps.jsonMode) labels.push("json");
  if (caps.maxContext !== null) labels.push(`${caps.maxContext} ctx`);
  return labels;
}

/** A human-readable rough price hint, or "free" for zero-rate (local) models. */
function priceHint(model: AvailableModel): string {
  if (isFreeModel(model)) return "free";
  const { inputPerMtok, outputPerMtok } = model.price;
  return `$${inputPerMtok}/$${outputPerMtok} per Mtok`;
}

/** True when two routes reference the same provider + model. */
function sameRoute(a: ManualRoute | null, model: AvailableModel): boolean {
  return a !== null && a.providerId === model.providerId && a.model === model.model;
}

export interface ProviderModelPickerProps {
  /** The available options to choose from (from the providers store). */
  models: AvailableModel[];
  /** The currently selected route, or null when nothing is selected. */
  value: ManualRoute | null;
  /** Called with the chosen route when the user selects an option. */
  onChange: (route: ManualRoute) => void;
  /**
   * Decide whether an option is a Local model. Defaults to the zero-price
   * heuristic; callers with richer provider metadata can override it.
   */
  isLocal?: (model: AvailableModel) => boolean;
}

interface GroupProps {
  label: string;
  models: AvailableModel[];
  value: ManualRoute | null;
  onChange: (route: ManualRoute) => void;
}

function ModelGroup({ label, models, value, onChange }: GroupProps) {
  if (models.length === 0) return null;
  return (
    <section className="model-group" aria-label={label}>
      <h4 className="model-group__label">{label}</h4>
      <ul className="model-group__list">
        {models.map((model) => {
          const selected = sameRoute(value, model);
          return (
            <li key={`${model.providerId}:${model.model}`}>
              <button
                type="button"
                className="model-option"
                aria-pressed={selected}
                data-selected={selected}
                onClick={() => onChange({ providerId: model.providerId, model: model.model })}
              >
                <span className="model-option__name">
                  {model.providerId} / {model.model}
                </span>
                <span className="model-option__caps">{capabilityLabels(model).join(" · ")}</span>
                <span className="model-option__price">{priceHint(model)}</span>
              </button>
            </li>
          );
        })}
      </ul>
    </section>
  );
}

/** A grouped Local vs Cloud provider/model picker. */
export function ProviderModelPicker({
  models,
  value,
  onChange,
  isLocal = isFreeModel,
}: ProviderModelPickerProps) {
  const local = models.filter((m) => isLocal(m));
  const cloud = models.filter((m) => !isLocal(m));

  if (models.length === 0) {
    return <NoModelsEmptyState />;
  }

  return (
    <div className="model-picker">
      <ModelGroup label="Local" models={local} value={value} onChange={onChange} />
      <ModelGroup label="Cloud" models={cloud} value={value} onChange={onChange} />
    </div>
  );
}
