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
import { NoModelsEmptyState } from "./NoModelsEmptyState";
import { ProviderEnumerationErrors } from "./ProviderEnumerationErrors";
import { ProviderModelPicker } from "./ProviderModelPicker";

export function PerMessageOverrideControl() {
  const pendingOverride = useConversationsStore((s) => s.pendingOverride);
  const setPendingOverride = useConversationsStore((s) => s.setPendingOverride);
  const models = useProvidersStore((s) => s.models);
  const errors = useProvidersStore((s) => s.errors);
  const loadState = useProvidersStore((s) => s.loadState);
  const lastError = useProvidersStore((s) => s.lastError);
  const load = useProvidersStore((s) => s.load);

  const choose = (route: ManualRoute) => {
    setPendingOverride(route);
  };

  const clear = () => {
    setPendingOverride(null);
  };

  // Always-available manual re-enumeration: re-run `list_available_models` so a
  // user who just started a local runtime or fixed a key can pull models in
  // without restarting or reopening Settings. Fire-and-forget; failures surface
  // via the store's `errors` (rendered by ProviderEnumerationErrors below).
  const refresh = () => {
    void load();
  };

  // A prominent, copyable status line reporting the outcome of the last model
  // load, so a user who sees no models can read WHY (loading / how many loaded /
  // the load failure message) instead of an unexplained empty picker. Defensive
  // against undefined/empty arrays (mirrors the Array.isArray guard in
  // ProviderEnumerationErrors).
  const modelItems = Array.isArray(models) ? models : [];
  const providerCount = new Set(modelItems.map((m) => m.providerId)).size;
  const loadStatusText =
    loadState === "loading"
      ? "loading…"
      : loadState === "failed"
        ? `failed: ${lastError ?? "unknown error"}`
        : `loaded ${modelItems.length} models from ${providerCount} providers`;

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
        <button
          type="button"
          className="per-message-override__refresh"
          aria-label="Refresh models"
          data-testid="refresh-models"
          onClick={refresh}
        >
          ↻ Refresh models
        </button>
      </div>
      {/* Prominent, copyable status line: reports the outcome of the last model
          load (loading / how many models loaded / the failure message) so an
          empty picker is always explained. Placed above the per-provider errors. */}
      <p className="per-message-override__load-status" data-testid="model-load-status">
        {loadStatusText}
      </p>
      {/* Surface why any provider failed to enumerate, near the picker, so a
          misconfigured/unreachable provider is diagnosable. Rendered alongside
          the picker so healthy providers' models are never blanked by an error. */}
      <ProviderEnumerationErrors errors={errors} />
      {models.length === 0 ? (
        <NoModelsEmptyState />
      ) : (
        <ProviderModelPicker models={models} value={pendingOverride} onChange={choose} />
      )}
    </div>
  );
}
