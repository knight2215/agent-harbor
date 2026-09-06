import { describe, it, expect, vi, beforeEach } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
}));

import { useToolsStore } from "./tools";
import type { McpServerConfig } from "../types";

function server(id: string, enabled = false): McpServerConfig {
  return {
    id,
    name: "fs",
    transport: { type: "stdio", command: "mcp-fs", args: [], env: [] },
    permissionMode: "ask",
    enabled,
  };
}

describe("tools store", () => {
  beforeEach(() => {
    invoke.mockReset();
    invoke.mockResolvedValue([]);
    useToolsStore.setState({ servers: [], connectionState: {}, tools: {}, errors: {} });
  });

  it("load fetches the configured MCP servers", async () => {
    invoke.mockResolvedValue([server("s-1")]);
    await useToolsStore.getState().load();
    expect(invoke).toHaveBeenCalledWith("list_mcp_servers");
    expect(useToolsStore.getState().servers).toHaveLength(1);
  });

  it("setEnabled persists and reflects the updated server", async () => {
    useToolsStore.setState({ servers: [server("s-1", false)] });
    invoke.mockResolvedValue(server("s-1", true));
    await useToolsStore.getState().setEnabled("s-1", true);
    expect(invoke).toHaveBeenCalledWith("set_mcp_enabled", { id: "s-1", enabled: true });
    expect(useToolsStore.getState().servers[0].enabled).toBe(true);
  });

  it("refreshTools caches the discovered tools per server", async () => {
    invoke.mockResolvedValue([{ name: "read_file", description: "", inputSchema: {} }]);
    await useToolsStore.getState().refreshTools("s-1");
    expect(invoke).toHaveBeenCalledWith("refresh_mcp_tools", { id: "s-1" });
    expect(useToolsStore.getState().tools["s-1"]).toHaveLength(1);
  });

  it("mcpStateChanged tracks per-server connection state", () => {
    useToolsStore.getState().applyCoreEvent({
      type: "mcpStateChanged",
      serverId: "s-1",
      state: "connected",
    });
    expect(useToolsStore.getState().connectionState["s-1"]).toBe("connected");
  });

  it("mcpError records the last per-server error", () => {
    useToolsStore.getState().applyCoreEvent({
      type: "mcpError",
      serverId: "s-1",
      message: "spawn failed",
    });
    expect(useToolsStore.getState().errors["s-1"]).toBe("spawn failed");
  });
});
