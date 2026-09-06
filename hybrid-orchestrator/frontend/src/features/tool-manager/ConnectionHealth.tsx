// ConnectionHealth (architecture.md Section 8.3 tool manager).
//
// Renders a single MCP server's live connection state (from the tools store's
// per-server `connectionState`, kept current by `mcpStateChanged` events) and
// its last error message (from `mcpError`). Presentational: it reads the store
// values through props supplied by the parent so it can be reused per row.

import type { McpConnectionState } from "../../types";

const STATE_LABEL: Record<McpConnectionState, string> = {
  connecting: "Connecting",
  connected: "Connected",
  disconnected: "Disconnected",
};

export interface ConnectionHealthProps {
  /** The server's current connection state, or undefined when never reported. */
  state: McpConnectionState | undefined;
  /** The server's last error message, if any. */
  error: string | undefined;
}

export function ConnectionHealth({ state, error }: ConnectionHealthProps) {
  const label = state === undefined ? "Unknown" : STATE_LABEL[state];
  return (
    <div className="connection-health" aria-label="Connection health">
      <span className="connection-health__state" data-state={state ?? "unknown"} role="status">
        {label}
      </span>
      {error !== undefined && (
        <span className="connection-health__error" data-testid="connection-error">
          {error}
        </span>
      )}
    </div>
  );
}
