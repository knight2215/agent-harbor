// Tools / MCP-manager store (architecture.md Section 8.3).
//
// Owns the configured MCP servers, their per-server connection state, and their
// discovered tool lists. The core is authoritative (Section 7.3): `load`
// fetches the servers via `list_mcp_servers`, and the `mcpStateChanged` /
// `mcpError` CoreEvents keep per-server status in sync. `refreshTools`
// reconnects a server and caches its tool descriptors.

import { create } from "zustand";
import {
  listMcpServers,
  refreshMcpTools,
  setMcpEnabled as setMcpEnabledCmd,
} from "../ipc/commands";
import type { CoreEvent, McpConnectionState, McpServerConfig, ToolDescriptorView } from "../types";

export interface ToolsState {
  /** Configured MCP servers. */
  servers: McpServerConfig[];
  /** Per-server connection state, keyed by server id. */
  connectionState: Record<string, McpConnectionState>;
  /** Discovered tools per server, keyed by server id. */
  tools: Record<string, ToolDescriptorView[]>;
  /** Last per-server error message, keyed by server id. */
  errors: Record<string, string>;

  /** Load the configured MCP servers from the core. */
  load: () => Promise<void>;
  /** Enable or disable a server (persist + connect/teardown). */
  setEnabled: (id: string, enabled: boolean) => Promise<void>;
  /** Refresh a server's tool list (reconnect + re-list). */
  refreshTools: (id: string) => Promise<void>;
  /** Apply a CoreEvent: track connection state and errors. */
  applyCoreEvent: (event: CoreEvent) => void;
}

export const useToolsStore = create<ToolsState>((set) => ({
  servers: [],
  connectionState: {},
  tools: {},
  errors: {},

  load: async () => {
    const servers = await listMcpServers();
    set({ servers });
  },

  setEnabled: async (id, enabled) => {
    const updated = await setMcpEnabledCmd(id, enabled);
    set((state) => ({
      servers: state.servers.map((s) => (s.id === updated.id ? updated : s)),
    }));
  },

  refreshTools: async (id) => {
    const tools = await refreshMcpTools(id);
    set((state) => ({ tools: { ...state.tools, [id]: tools } }));
  },

  applyCoreEvent: (event) => {
    switch (event.type) {
      case "mcpStateChanged":
        set((state) => ({
          connectionState: { ...state.connectionState, [event.serverId]: event.state },
        }));
        return;
      case "mcpError":
        set((state) => ({
          errors: { ...state.errors, [event.serverId]: event.message },
        }));
        return;
      default:
        // Some server-config changes are surfaced as conversationUpdated / other
        // events; re-fetch the server list defensively when the set may change.
        return;
    }
  },
}));
