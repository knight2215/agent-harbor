// RoutingModeToggle (architecture.md Section 8.2 model selector).
//
// Switches the active conversation between Automatic routing and a Manual pin.
// Auto CLEARS the pin via `setConversationRoute(id, null)`; Manual reveals the
// reusable ProviderModelPicker, and choosing a model pins it via
// `setConversationRoute(id, route)`. The conversation's persisted pin
// (`conversationPref`) is the source of truth for the current mode, so the
// toggle reflects it directly; `conversationUpdated` events flow through the
// conversations store and re-render this component.

import { useState } from "react";
import { useConversationsStore } from "../../state/conversations";
import { useProvidersStore } from "../../state/providers";
import type { ManualRoute } from "../../types";
import { AutoRationaleTooltip } from "./AutoRationaleTooltip";
import { ProviderModelPicker } from "./ProviderModelPicker";

export function RoutingModeToggle() {
  const activeConversationId = useConversationsStore((s) => s.activeConversationId);
  const conversations = useConversationsStore((s) => s.conversations);
  const setConversationRoute = useConversationsStore((s) => s.setConversationRoute);
  const models = useProvidersStore((s) => s.models);

  const active = conversations.find((c) => c.id === activeConversationId) ?? null;
  const pin = active?.conversationPref ?? null;
  const isManual = pin !== null;

  // Whether the Manual picker is expanded. Manual implies expanded; the user
  // can also expand it while still Automatic to choose a first pin.
  const [showPicker, setShowPicker] = useState(false);

  if (activeConversationId === null) {
    return <div className="routing-mode" aria-label="Routing mode" />;
  }

  const selectAuto = () => {
    setShowPicker(false);
    void setConversationRoute(activeConversationId, null);
  };

  const selectManual = () => {
    setShowPicker(true);
  };

  const pinRoute = (route: ManualRoute) => {
    void setConversationRoute(activeConversationId, route);
  };

  return (
    <div className="routing-mode" aria-label="Routing mode">
      <div className="routing-mode__toggle" role="radiogroup" aria-label="Routing mode">
        <button
          type="button"
          role="radio"
          aria-checked={!isManual}
          data-active={!isManual}
          onClick={selectAuto}
        >
          Auto
        </button>
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

      {(isManual || showPicker) && (
        <ProviderModelPicker models={models} value={pin} onChange={pinRoute} />
      )}
    </div>
  );
}
