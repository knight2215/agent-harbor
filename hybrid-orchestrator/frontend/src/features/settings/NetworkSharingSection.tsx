// Network Sharing settings section (FEAT-006).
//
// Incorporates LOCAL NETWORK (LAN) model sharing into the build, per explicit
// user request. It has three parts, each backed by a command in
// ipc/commands.ts:
//
//   1. CONSUME peers (the solid, testable core): list configured LAN peers,
//      Add one by base URL (+ optional label + optional key), and Remove one. A
//      peer is persisted as a generic OpenAI-compatible provider row, so its
//      models enumerate + route like any provider (they appear under a distinct
//      "Network" group in the model picker). The base-url posture rejects
//      link-local/metadata targets and warns on non-loopback plaintext.
//   2. DISCOVER peers (scaffold; live use user-only): a "Discover peers" button
//      lists found peers with a one-click Add, and shows a non-fatal "no peers
//      found / discovery unavailable" notice when empty.
//   3. SHARE my local models (scaffold; live use user-only): a toggle +
//      optional port that runs a small OpenAI-compatible read surface exposing
//      THIS machine's local models to the LAN. It shows the current status and a
//      clear SECURITY note. OFF by default.
//
// SECRET HYGIENE (Section 9.1): a peer's optional key flows IN and only an
// opaque SecretRef is stored server-side; no command returns key material, and
// this form never displays a key. Every input has an explicit aria-label
// (help-text-wrapping bug guard).

import { useEffect, useState } from "react";
import {
  addNetworkPeer,
  discoverNetworkPeers,
  getModelSharing,
  listNetworkPeers,
  removeNetworkPeer,
  setModelSharing,
} from "../../ipc/commands";
import type { DiscoveredPeerView, ModelSharingView, NetworkPeerView } from "../../types";

export function NetworkSharingSection() {
  // Consume peers.
  const [peers, setPeers] = useState<NetworkPeerView[]>([]);
  const [baseUrl, setBaseUrl] = useState("");
  const [label, setLabel] = useState("");
  const [apiKey, setApiKey] = useState("");
  const [warning, setWarning] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  // Discovery.
  const [discovered, setDiscovered] = useState<DiscoveredPeerView[] | null>(null);
  const [discovering, setDiscovering] = useState(false);

  // Sharing.
  const [sharing, setSharing] = useState<ModelSharingView | null>(null);
  const [port, setPort] = useState<string>("");

  // On mount, hydrate configured peers + the sharing status from the backend.
  useEffect(() => {
    listNetworkPeers()
      .then(setPeers)
      .catch(() => setPeers([]));
    getModelSharing()
      .then((next) => {
        setSharing(next);
        setPort(String(next.port));
      })
      .catch(() => setSharing(null));
  }, []);

  const refreshPeers = () =>
    listNetworkPeers()
      .then(setPeers)
      .catch(() => setPeers([]));

  const add = (url: string, peerLabel: string | null, peerKey: string | null) => {
    setError(null);
    setWarning(null);
    if (url.trim() === "") {
      setError("A base URL is required (e.g. http://192.168.1.50:11435/v1).");
      return;
    }
    addNetworkPeer(url.trim(), peerLabel, peerKey)
      .then((next) => {
        setWarning(next.warning);
        setBaseUrl("");
        setLabel("");
        setApiKey("");
        return refreshPeers();
      })
      .catch((err: unknown) => setError(String(err)));
  };

  const remove = (id: string) => {
    setError(null);
    removeNetworkPeer(id)
      .then(() => refreshPeers())
      .catch((err: unknown) => setError(String(err)));
  };

  const discover = () => {
    setError(null);
    setDiscovering(true);
    discoverNetworkPeers()
      .then((found) => setDiscovered(found))
      .catch(() => setDiscovered([]))
      .finally(() => setDiscovering(false));
  };

  const toggleSharing = (enabled: boolean) => {
    setError(null);
    const parsed = Number.parseInt(port, 10);
    const portArg = Number.isNaN(parsed) ? null : parsed;
    setModelSharing(enabled, portArg)
      .then((next) => {
        setSharing(next);
        setPort(String(next.port));
      })
      .catch((err: unknown) => setError(String(err)));
  };

  return (
    <section className="settings__panel" role="region" aria-label="Network Sharing">
      <h3 className="settings__section-title">Network Sharing</h3>
      <p className="settings__section-desc">
        Use models running on other machines on your local network, and optionally share your own
        local models with them. Peers you add appear under a <strong>Network</strong> group in the
        model picker and route like any other provider. A peer is off your machine, so a
        local-only / confidential conversation never routes to it.
      </p>

      {/* --- Consume peers --- */}
      <div className="settings-form">
        <h4 className="settings__subsection-title">Network peers</h4>
        <label>
          Peer base URL
          <input
            type="text"
            aria-label="Peer base URL"
            value={baseUrl}
            placeholder="http://192.168.1.50:11435/v1"
            onChange={(event) => setBaseUrl(event.target.value)}
          />
        </label>
        <label>
          Label (optional)
          <input
            type="text"
            aria-label="Peer label"
            value={label}
            placeholder="Studio box"
            onChange={(event) => setLabel(event.target.value)}
          />
        </label>
        <label>
          API key (optional)
          <input
            type="password"
            aria-label="Peer API key"
            value={apiKey}
            placeholder="Only if the peer requires one"
            onChange={(event) => setApiKey(event.target.value)}
          />
        </label>
        <button
          type="button"
          onClick={() => add(baseUrl, label === "" ? null : label, apiKey === "" ? null : apiKey)}
        >
          Add peer
        </button>

        {warning !== null && (
          <p className="settings__section-desc" role="status" data-testid="network-peer-warning">
            {warning}
          </p>
        )}

        {peers.length > 0 ? (
          <ul
            className="settings__list"
            data-testid="network-peer-list"
            aria-label="Configured network peers"
          >
            {peers.map((peer) => (
              <li key={peer.id} data-testid={`network-peer-${peer.id}`}>
                <span className="settings__list-label">
                  <strong>{peer.label}</strong> — {peer.baseUrl}
                  {peer.hasApiKey ? " (key set)" : ""}
                </span>
                <button
                  type="button"
                  aria-label={`Remove peer ${peer.label}`}
                  onClick={() => remove(peer.id)}
                >
                  Remove
                </button>
              </li>
            ))}
          </ul>
        ) : (
          <p className="settings__section-desc" data-testid="network-peer-empty">
            No network peers configured yet.
          </p>
        )}
      </div>

      {/* --- Discovery --- */}
      <div className="settings-form">
        <h4 className="settings__subsection-title">Discover peers</h4>
        <button
          type="button"
          aria-label="Discover peers on the local network"
          onClick={discover}
          disabled={discovering}
        >
          {discovering ? "Discovering…" : "Discover peers"}
        </button>
        {discovered !== null &&
          (discovered.length > 0 ? (
            <ul
              className="settings__list"
              data-testid="discovered-peer-list"
              aria-label="Discovered network peers"
            >
              {discovered.map((peer) => (
                <li key={peer.baseUrl} data-testid={`discovered-peer-${peer.baseUrl}`}>
                  <span className="settings__list-label">
                    <strong>{peer.label}</strong> — {peer.baseUrl}
                  </span>
                  <button
                    type="button"
                    aria-label={`Add discovered peer ${peer.label}`}
                    onClick={() => add(peer.baseUrl, peer.label, null)}
                  >
                    Add
                  </button>
                </li>
              ))}
            </ul>
          ) : (
            <p className="settings__section-desc" role="status" data-testid="discovered-peer-empty">
              No peers found. Discovery may be unavailable on this network; you can still add a peer
              by its base URL above.
            </p>
          ))}
      </div>

      {/* --- Share my local models --- */}
      <div className="settings-form">
        <h4 className="settings__subsection-title">Share my local models</h4>
        <p className="settings__section-desc">
          When enabled, this instance exposes your locally-hosted models (Ollama / LM Studio /
          embedded) to other machines on your local network over an OpenAI-compatible endpoint. This
          is OFF by default; only enable it on a network you trust.
        </p>
        <label>
          Share port
          <input
            type="number"
            aria-label="Model sharing port"
            min={1}
            max={65535}
            value={port}
            onChange={(event) => setPort(event.target.value)}
          />
        </label>
        <label className="settings__toggle-label">
          <input
            type="checkbox"
            aria-label="Share my local models on the network"
            checked={sharing?.enabled ?? false}
            onChange={(event) => toggleSharing(event.target.checked)}
          />
          Share my local models on the network
        </label>
        {sharing !== null && (
          <p className="settings__section-desc" role="status" data-testid="model-sharing-status">
            {sharing.status}
          </p>
        )}
      </div>

      {error !== null && (
        <p className="settings__section-desc" role="alert">
          {error}
        </p>
      )}
    </section>
  );
}
