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

  it("addServer appends the created server", async () => {
    const created = server("s-2", true);
    invoke.mockResolvedValue(created);
    const input = {
      name: "fs",
      transport: created.transport,
      permissionMode: "ask" as const,
      enabled: true,
    };
    await useToolsStore.getState().addServer(input);
    expect(invoke).toHaveBeenCalledWith("add_mcp_server", { config: input });
    expect(useToolsStore.getState().servers).toHaveLength(1);
    expect(useToolsStore.getState().servers[0].id).toBe("s-2");
  });

  it("updateServer replaces the matching server", async () => {
    useToolsStore.setState({ servers: [server("s-1", false)] });
    const updated = { ...server("s-1", true), name: "renamed" };
    invoke.mockResolvedValue(updated);
    const input = {
      name: "renamed",
      transport: updated.transport,
      permissionMode: "ask" as const,
      enabled: true,
    };
    await useToolsStore.getState().updateServer("s-1", input);
    expect(invoke).toHaveBeenCalledWith("update_mcp_server", { id: "s-1", config: input });
    expect(useToolsStore.getState().servers[0].name).toBe("renamed");
  });

  it("removeServer deletes the server and its per-server caches", async () => {
    useToolsStore.setState({
      servers: [server("s-1", true)],
      connectionState: { "s-1": "connected" },
      tools: { "s-1": [] },
      errors: { "s-1": "boom" },
    });
    invoke.mockResolvedValue(undefined);
    await useToolsStore.getState().removeServer("s-1");
    expect(invoke).toHaveBeenCalledWith("remove_mcp_server", { id: "s-1" });
    expect(useToolsStore.getState().servers).toHaveLength(0);
    expect(useToolsStore.getState().connectionState["s-1"]).toBeUndefined();
    expect(useToolsStore.getState().errors["s-1"]).toBeUndefined();
  });

  it("setPermission forwards the per-server mode and reflects the result", async () => {
    useToolsStore.setState({ servers: [server("s-1", true)] });
    invoke.mockResolvedValue({ ...server("s-1", true), permissionMode: "allow" });
    await useToolsStore.getState().setPermission("s-1", "allow");
    expect(invoke).toHaveBeenCalledWith("set_tool_permission", {
      serverId: "s-1",
      toolName: null,
      mode: "allow",
    });
    expect(useToolsStore.getState().servers[0].permissionMode).toBe("allow");
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
