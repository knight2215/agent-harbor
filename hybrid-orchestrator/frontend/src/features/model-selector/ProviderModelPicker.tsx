// ProviderModelPicker (architecture.md Section 8.2 model selector).
//
// A REUSABLE, presentational picker over the available provider/model options.
// The chat model selector renders it (via PerMessageOverrideControl and the
// manual-pin path), and the agent editor's DefaultRoutePicker (FEAT-003)
// imports the very same component, so it takes its data + selection through
// props and owns no store state itself.
//
// Options are grouped into Network (LAN peers, FEAT-006), Local (on-host), and
// Cloud. `AvailableModel` does not carry the provider kind/baseUrl, so a LAN
// peer is recognized by the optional `isNetwork` predicate (default: the peer's
// provider id prefix, which the backend `add_network_peer` command stamps) and
// on-host locality by the optional `isLocal` predicate (default: the zero-price
// heuristic, since local providers price at zero per the `TokenPrice` contract,
// Section 6.2). Network peers are shown SEPARATELY from on-host Local so the user
// can tell an off-host peer apart. Each option shows capability hints (from
// `Capabilities`) and a rough cost signal (from `TokenPrice`).

import type { AvailableModel, ManualRoute } from "../../types";
import { NoModelsEmptyState } from "./NoModelsEmptyState";

/** True when both token rates are zero, the default "this is a local model" signal. */
function isFreeModel(model: AvailableModel): boolean {
  return model.price.inputPerMtok === 0 && model.price.outputPerMtok === 0;
}

/**
 * The stable provider-id prefix the backend `add_network_peer` command stamps on
 * a LAN peer's provider row (FEAT-006). Mirrors Rust `NETWORK_PEER_ID_PREFIX`.
 * A peer's models therefore carry a `providerId` starting with this, which is
 * how the default `isNetwork` predicate tells a LAN peer apart from an on-host
 * Local or a Cloud provider without extra data.
 */
const NETWORK_PEER_ID_PREFIX = "network-peer-";

/** True when a model belongs to a LAN peer (FEAT-006), by its provider-id prefix. */
function isNetworkPeerModel(model: AvailableModel): boolean {
  return model.providerId.startsWith(NETWORK_PEER_ID_PREFIX);
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
   * Decide whether an option is a Local (on-host) model. Defaults to the
   * zero-price heuristic; callers with richer provider metadata can override it.
   */
  isLocal?: (model: AvailableModel) => boolean;
  /**
   * Decide whether an option is a LAN peer (FEAT-006), shown in a distinct
   * Network group. Defaults to the peer provider-id prefix the backend stamps.
   */
  isNetwork?: (model: AvailableModel) => boolean;
}

interface GroupProps {
  label: string;
  models: AvailableModel[];
  value: ManualRoute | null;
  onChange: (route: ManualRoute) => void;
}

function ModelGroup({ label, models, value, onChange }: GroupProps) {
  // Normalize defensively so an undefined/non-array `models` cannot crash the
  // `.length`/`.map` reads below.
  const list = Array.isArray(models) ? models : [];
  if (list.length === 0) return null;
  return (
    <section className="model-group" aria-label={label}>
      <h4 className="model-group__label">{label}</h4>
      <ul className="model-group__list">
        {list.map((model) => {
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

/** A grouped Network (LAN peers) / Local (on-host) / Cloud provider/model picker. */
export function ProviderModelPicker({
  models,
  value,
  onChange,
  isLocal = isFreeModel,
  isNetwork = isNetworkPeerModel,
}: ProviderModelPickerProps) {
  // Normalize defensively so an undefined/non-array `models` prop cannot crash
  // the `.filter`/`.length` reads below.
  const list = Array.isArray(models) ? models : [];
  // A LAN peer is classified first so it is shown in its own Network group and
  // never double-counts under Local/Cloud (a peer's zero-priced models would
  // otherwise fall into Local).
  const network = list.filter((m) => isNetwork(m));
  const rest = list.filter((m) => !isNetwork(m));
  const local = rest.filter((m) => isLocal(m));
  const cloud = rest.filter((m) => !isLocal(m));

  if (list.length === 0) {
    return <NoModelsEmptyState />;
  }

  return (
    <div className="model-picker">
      <ModelGroup label="Network" models={network} value={value} onChange={onChange} />
      <ModelGroup label="Local" models={local} value={value} onChange={onChange} />
      <ModelGroup label="Cloud" models={cloud} value={value} onChange={onChange} />
    </div>
  );
}
