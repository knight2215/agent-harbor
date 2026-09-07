// DefaultRoutePicker (architecture.md Section 8.4 agent editor).
//
// Picks a persona's default route. It REUSES the FEAT-002 ProviderModelPicker
// (the very same reusable component the chat model selector renders) over the
// available models from the providers store, and adds a "clear" affordance so a
// persona can have no default route (falling back to automatic routing). Built
// once, imported here.

import { useProvidersStore } from "../../state/providers";
import { ProviderModelPicker } from "../model-selector";
import type { ManualRoute } from "../../types";

export interface DefaultRoutePickerProps {
  /** The persona's current default route, or null for automatic. */
  value: ManualRoute | null;
  /** Called with the chosen route, or null when cleared. */
  onChange: (route: ManualRoute | null) => void;
}

export function DefaultRoutePicker({ value, onChange }: DefaultRoutePickerProps) {
  const models = useProvidersStore((s) => s.models);

  return (
    <div className="default-route-picker" aria-label="Default route">
      <div className="default-route-picker__header">
        <span className="default-route-picker__label">Default route</span>
        {value !== null && (
          <>
            <span className="default-route-picker__current" data-testid="default-route-current">
              {value.providerId} / {value.model}
            </span>
            <button type="button" onClick={() => onChange(null)}>
              Clear
            </button>
          </>
        )}
      </div>
      <ProviderModelPicker models={models} value={value} onChange={onChange} />
    </div>
  );
}
