// RoutingModeToggle (architecture.md Section 8.2 model selector; FEAT-002/003).
//
// A 4-position segmented control that switches the active conversation between
// the backend routing modes: Auto, Prefer Local, Prefer Quality, and Manual.
//
//   - Auto / Prefer Local / Prefer Quality call `setConversationRoutingMode`
//     with the matching mode. When LEAVING Manual they also clear any manual
//     pin via `setConversationRoute(id, null)`, since a pin implies Manual.
//   - Manual calls `setConversationRoutingMode(id, "manual")` and reveals the
//     reusable ProviderModelPicker; choosing a model pins it via
//     `setConversationRoute(id, route)` (the existing behaviour).
//
// The active position is derived from the persisted conversation: Manual when a
// pin (`conversationPref`) is set OR `routingMode === "manual"`; otherwise the
// stored `routingMode`, defaulting a missing/null value to Auto (serde omits
// the field when None). `conversationUpdated` events flow through the
// conversations store and re-render this component.
//
// NOTE: this ships as a discrete SEGMENTED toggle (not a continuous slider). A
// future continuous slider (e.g. cost<->quality) could drive the same
// `RoutingMode` values through the same `setConversationRoutingMode` action
// without changing the store or IPC layer.

import { useState } from "react";
import { useConversationsStore } from "../../state/conversations";
import { useProvidersStore } from "../../state/providers";
import type { ManualRoute, RoutingMode } from "../../types";
import { AutoRationaleTooltip } from "./AutoRationaleTooltip";
import { ProviderModelPicker } from "./ProviderModelPicker";

/** The non-Manual segments and their labels, in display order. */
const AUTO_MODES: ReadonlyArray<{ mode: Exclude<RoutingMode, "manual">; label: string }> = [
  { mode: "auto", label: "Auto" },
  { mode: "preferLocal", label: "Prefer Local" },
  { mode: "preferQuality", label: "Prefer Quality" },
];

export function RoutingModeToggle() {
  const activeConversationId = useConversationsStore((s) => s.activeConversationId);
  const conversations = useConversationsStore((s) => s.conversations);
  const setConversationRoute = useConversationsStore((s) => s.setConversationRoute);
  const setConversationRoutingMode = useConversationsStore((s) => s.setConversationRoutingMode);
  const models = useProvidersStore((s) => s.models);

  const active = conversations.find((c) => c.id === activeConversationId) ?? null;
  const pin = active?.conversationPref ?? null;
  // A pin implies Manual regardless of the stored mode; otherwise fall back to
  // the persisted mode, treating a missing/null value as Auto.
  const isManual = pin !== null || active?.routingMode === "manual";
  const activeMode: RoutingMode = isManual ? "manual" : (active?.routingMode ?? "auto");

  // Whether the Manual picker is expanded. Manual implies expanded; the user
  // can also expand it while still automatic to choose a first pin.
  const [showPicker, setShowPicker] = useState(false);

  if (activeConversationId === null) {
    return (
      <div className="routing-mode routing-mode--empty" aria-label="Routing mode">
        <p className="settings__empty-state">
          No conversation selected. Start a <strong>New conversation</strong> or pick one from
          History to choose how it routes.
        </p>
      </div>
    );
  }

  const selectMode = (mode: Exclude<RoutingMode, "manual">) => {
    setShowPicker(false);
    void setConversationRoutingMode(activeConversationId, mode);
    // Leaving Manual clears any pin, since a pin forces the Manual position.
    if (pin !== null) {
      void setConversationRoute(activeConversationId, null);
    }
  };

  const selectManual = () => {
    setShowPicker(true);
    void setConversationRoutingMode(activeConversationId, "manual");
  };

  const pinRoute = (route: ManualRoute) => {
    void setConversationRoute(activeConversationId, route);
  };

  return (
    <div className="routing-mode" aria-label="Routing mode">
      <div className="routing-mode__toggle" role="radiogroup" aria-label="Routing mode">
        {AUTO_MODES.map(({ mode, label }) => (
          <button
            key={mode}
            type="button"
            role="radio"
            aria-checked={activeMode === mode}
            data-active={activeMode === mode}
            onClick={() => selectMode(mode)}
          >
            {label}
          </button>
        ))}
        <button
          type="button"
          role="radio"
          aria-checked={isManual}
          data-active={isManual}
          onClick={selectManual}
        >
          Manual
        </button>
      </div>

      {!isManual && <AutoRationaleTooltip conversationId={activeConversationId} />}

      {/*
        Reveal the manual-pin picker only when Manual/showPicker is on AND there
        are models to choose from. With zero models the picker would render a
        NoModelsEmptyState that stacks with the one PerMessageOverrideControl
        already shows in the chat pane; suppressing it here keeps a single piece
        of guidance visible (the override control owns it) without changing the
        routing-mode or pin semantics.
      */}
      {(isManual || showPicker) && models.length > 0 && (
        <ProviderModelPicker models={models} value={pin} onChange={pinRoute} />
      )}
    </div>
  );
}
