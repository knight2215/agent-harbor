// ServerList (architecture.md Section 8.3 tool manager).
//
// Lists the configured MCP servers from the tools store, each with a live
// status badge (from per-server `connectionState`, kept current by
// `mcpStateChanged`), an enable/disable toggle (-> `setEnabled`), a remove
// button (-> `removeServer`), a per-server PermissionModeControl, a
// ConnectionHealth panel, and an expandable ToolInspector. An Edit button
// selects the server for the ServerForm (handled by the parent).

import { useToolsStore } from "../../state/tools";
import type { McpServerConfig } from "../../types";
import { ConnectionHealth } from "./ConnectionHealth";
import { PermissionModeControl } from "./PermissionModeControl";
import { ToolInspector } from "./ToolInspector";

export interface ServerListProps {
  /** Called when the user asks to edit a server. */
  onEdit?: (server: McpServerConfig) => void;
}

export function ServerList({ onEdit }: ServerListProps) {
  const servers = useToolsStore((s) => s.servers);
  const connectionState = useToolsStore((s) => s.connectionState);
  const errors = useToolsStore((s) => s.errors);
  const setEnabled = useToolsStore((s) => s.setEnabled);
  const removeServer = useToolsStore((s) => s.removeServer);

  if (servers.length === 0) {
    return <p className="server-list__empty">No MCP servers configured.</p>;
  }

  return (
    <ul className="server-list" aria-label="MCP servers">
      {servers.map((server) => (
        <li key={server.id} className="server-list__item" data-testid={`server-${server.id}`}>
          <div className="server-list__header">
            <span className="server-list__name">{server.name}</span>
            <span className="server-list__transport">{server.transport.type}</span>
            <ConnectionHealth state={connectionState[server.id]} error={errors[server.id]} />
          </div>

          <div className="server-list__controls">
            <label className="server-list__enabled">
              <input
                type="checkbox"
                aria-label={`Enable ${server.name}`}
                checked={server.enabled}
                onChange={(e) => void setEnabled(server.id, e.target.checked)}
              />
              Enabled
            </label>
            <PermissionModeControl serverId={server.id} mode={server.permissionMode} />
            <button type="button" onClick={() => onEdit?.(server)}>
              Edit
            </button>
            <button
              type="button"
              aria-label={`Remove ${server.name}`}
              onClick={() => void removeServer(server.id)}
            >
              Remove
            </button>
          </div>

          <ToolInspector serverId={server.id} />
        </li>
      ))}
    </ul>
  );
}
