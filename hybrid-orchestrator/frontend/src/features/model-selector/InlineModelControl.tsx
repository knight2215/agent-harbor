// InlineModelControl (architecture.md Section 8.2 model selector; FEAT-002
// redesign).
//
// A COMPACT, Kiro-style inline model control that lives inside the composer,
// replacing the old wall-of-controls PerMessageOverrideControl block above the
// input. It shows the effective "providerId / model" (the shared transient
// per-message override) as a small dropdown trigger (▾); clicking it reveals the
// reusable ProviderModelPicker (grouped Local vs Cloud) in a popover so the user
// can pick a one-off route for the NEXT message.
//
// OWNERSHIP: it owns NO copy of the value. It reads and writes the SINGLE
// transient override held by the conversations store (`pendingOverride` /
// `setPendingOverride`), the very same value the Composer's send path
// consumes-and-clears, so the override applies to exactly one message (Section
// 8.2 per-message override ownership). This is the identical store path
// PerMessageOverrideControl uses; the two are alternate presentations of the
// same shared state, never duplicated ownership.
//
// EMPTY STATE: when there are zero models it shows a SINGLE compact "No models"
// affordance (one NoModelsEmptyState), preserving the single-guidance invariant
// tested in app.test.tsx.

import { useState } from "react";
import { useConversationsStore } from "../../state/conversations";
import { useProvidersStore } from "../../state/providers";
import type { ManualRoute } from "../../types";
import { NoModelsEmptyState } from "./NoModelsEmptyState";
import { ProviderModelPicker } from "./ProviderModelPicker";

export function InlineModelControl() {
  const pendingOverride = useConversationsStore((s) => s.pendingOverride);
  const setPendingOverride = useConversationsStore((s) => s.setPendingOverride);
  const models = useProvidersStore((s) => s.models);
  const [open, setOpen] = useState(false);

  const modelItems = Array.isArray(models) ? models : [];

  const choose = (route: ManualRoute) => {
    setPendingOverride(route);
    setOpen(false);
  };

  const clear = () => {
    setPendingOverride(null);
    setOpen(false);
  };

  // With zero models, present a SINGLE compact "No models" affordance (visible,
  // not hidden behind a closed dropdown) so the chat pane always carries exactly
  // one piece of guidance (the single-no-models-guidance invariant tested in
  // app.test.tsx). RoutingModeToggle suppresses its own manual-pin picker when
  // models are empty, so this is the one and only NoModelsEmptyState.
  if (modelItems.length === 0) {
    return (
      <div className="inline-model inline-model--empty" aria-label="One-off model override">
        <NoModelsEmptyState />
      </div>
    );
  }

  // The trigger label reflects the effective one-off override, or an "Auto"
  // affordance when nothing is pinned (routing then decides the model).
  const label =
    pendingOverride !== null
      ? `${pendingOverride.providerId} / ${pendingOverride.model}`
      : "Auto (routed)";

  return (
    <div className="inline-model" aria-label="One-off model override">
      <button
        type="button"
        className="inline-model__trigger"
        aria-haspopup="menu"
        aria-expanded={open}
        aria-label="Select model for the next message"
        onClick={() => setOpen((prev) => !prev)}
      >
        <span className="inline-model__current" data-testid="inline-model-current">
          {label}
        </span>
        <span className="inline-model__caret" aria-hidden="true">
          ▾
        </span>
      </button>
      {open && (
        <div className="inline-model__popover" role="menu" aria-label="Available models">
          <div className="inline-model__popover-header">
            <span className="inline-model__popover-label">Override next message</span>
            {pendingOverride !== null && (
              <button type="button" className="inline-model__clear" onClick={clear}>
                Clear
              </button>
            )}
          </div>
          <ProviderModelPicker models={modelItems} value={pendingOverride} onChange={choose} />
        </div>
      )}
    </div>
  );
}
