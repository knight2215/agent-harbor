// PerMessageOverrideControl (architecture.md Section 8.2 "Per-message override
// ownership").
//
// A one-off route override for the NEXT message only. It owns NO copy of the
// value: it reads and writes the SINGLE transient override held by the active
// conversation's routing store (state/conversations.ts). The Composer's send
// path consumes-and-clears that same value, so the override applies to exactly
// one message. Rendered inside the Composer.

import { useConversationsStore } from "../../state/conversations";
import { useProvidersStore } from "../../state/providers";
import type { ManualRoute } from "../../types";
import { ProviderModelPicker } from "./ProviderModelPicker";

export function PerMessageOverrideControl() {
  const pendingOverride = useConversationsStore((s) => s.pendingOverride);
  const setPendingOverride = useConversationsStore((s) => s.setPendingOverride);
  const models = useProvidersStore((s) => s.models);

  const choose = (route: ManualRoute) => {
    setPendingOverride(route);
  };

  const clear = () => {
    setPendingOverride(null);
  };

  return (
    <div className="per-message-override" aria-label="One-off model override">
      <div className="per-message-override__header">
        <span className="per-message-override__label">Override next message</span>
        {pendingOverride !== null && (
          <>
            <span className="per-message-override__current" data-testid="override-current">
              {pendingOverride.providerId} / {pendingOverride.model}
            </span>
            <button type="button" onClick={clear}>
              Clear
            </button>
          </>
        )}
      </div>
      <ProviderModelPicker models={models} value={pendingOverride} onChange={choose} />
    </div>
  );
}
