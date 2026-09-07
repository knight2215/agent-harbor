import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn().mockResolvedValue(() => undefined),
}));

import { useToolsStore } from "../../state/tools";
import { ConnectionHealth } from "./ConnectionHealth";
import { ServerForm } from "./ServerForm";
import { ServerList } from "./ServerList";
import { ToolInspector } from "./ToolInspector";
import type { McpServerConfig } from "../../types";

function stdioServer(id: string, enabled = true): McpServerConfig {
  return {
    id,
    name: "fs",
    transport: { type: "stdio", command: "mcp-fs", args: ["--root", "/tmp"], env: [] },
    permissionMode: "ask",
    enabled,
  };
}

function resetTools() {
  useToolsStore.setState({ servers: [], connectionState: {}, tools: {}, errors: {} });
}

describe("tool manager", () => {
  beforeEach(() => {
    invoke.mockReset();
    invoke.mockResolvedValue(undefined);
    resetTools();
  });

  it("ServerForm submit builds a stdio McpServerInput and calls add_mcp_server", async () => {
    const created = stdioServer("s-1");
    invoke.mockResolvedValue(created);
    render(<ServerForm />);

    fireEvent.change(screen.getByLabelText("Server name"), { target: { value: "fs" } });
    fireEvent.change(screen.getByLabelText("Command"), { target: { value: "mcp-fs" } });
    fireEvent.change(screen.getByLabelText("Arguments"), { target: { value: "--root /tmp" } });
    fireEvent.click(screen.getByRole("button", { name: "Add server" }));

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("add_mcp_server", {
        config: {
          name: "fs",
          transport: { type: "stdio", command: "mcp-fs", args: ["--root", "/tmp"], env: [] },
          permissionMode: "ask",
          enabled: true,
        },
      });
    });
  });

  it("ServerForm submit builds an httpSse McpServerInput", async () => {
    invoke.mockResolvedValue(stdioServer("s-2"));
    render(<ServerForm />);

    fireEvent.change(screen.getByLabelText("Server name"), { target: { value: "remote" } });
    fireEvent.click(screen.getByRole("radio", { name: "httpSse" }));
    fireEvent.change(screen.getByLabelText("URL"), {
      target: { value: "https://example.com/sse" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Add server" }));

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("add_mcp_server", {
        config: {
          name: "remote",
          transport: { type: "httpSse", url: "https://example.com/sse", headers: [] },
          permissionMode: "ask",
          enabled: true,
        },
      });
    });
  });

  it("ServerForm rejects a stdio transport with no command", () => {
    render(<ServerForm />);
    fireEvent.change(screen.getByLabelText("Server name"), { target: { value: "fs" } });
    fireEvent.click(screen.getByRole("button", { name: "Add server" }));
    expect(screen.getByRole("alert")).toHaveTextContent("Command is required");
    expect(invoke).not.toHaveBeenCalled();
  });

  it("ServerList enable/disable calls set_mcp_enabled and remove calls remove_mcp_server", async () => {
    useToolsStore.setState({ servers: [stdioServer("s-1", false)] });
    invoke.mockResolvedValue(stdioServer("s-1", true));
    render(<ServerList />);

    fireEvent.click(screen.getByRole("checkbox", { name: "Enable fs" }));
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("set_mcp_enabled", { id: "s-1", enabled: true });
    });

    invoke.mockResolvedValue(undefined);
    fireEvent.click(screen.getByRole("button", { name: "Remove fs" }));
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("remove_mcp_server", { id: "s-1" });
    });
  });

  it("PermissionModeControl sets the per-server mode via set_tool_permission", async () => {
    useToolsStore.setState({ servers: [stdioServer("s-1")] });
    invoke.mockResolvedValue({ ...stdioServer("s-1"), permissionMode: "allow" });
    render(<ServerList />);

    fireEvent.click(screen.getByRole("radio", { name: "Allow" }));
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("set_tool_permission", {
        serverId: "s-1",
        toolName: null,
        mode: "allow",
      });
    });
  });

  it("mcp_state_changed / mcp_error update the status badge and ConnectionHealth", () => {
    useToolsStore.setState({ servers: [stdioServer("s-1")] });
    const { rerender } = render(<ServerList />);

    useToolsStore.getState().applyCoreEvent({
      type: "mcpStateChanged",
      serverId: "s-1",
      state: "connected",
    });
    useToolsStore.getState().applyCoreEvent({
      type: "mcpError",
      serverId: "s-1",
      message: "spawn failed",
    });
    rerender(<ServerList />);

    expect(screen.getByRole("status")).toHaveTextContent("Connected");
    expect(screen.getByTestId("connection-error")).toHaveTextContent("spawn failed");
  });

  it("ConnectionHealth renders an unknown state when never reported", () => {
    render(<ConnectionHealth state={undefined} error={undefined} />);
    expect(screen.getByRole("status")).toHaveTextContent("Unknown");
  });

  it("ToolInspector renders tools from the store and refreshes on demand", async () => {
    useToolsStore.setState({
      tools: { "s-1": [{ name: "read_file", description: "reads a file", inputSchema: {} }] },
    });
    invoke.mockResolvedValue([{ name: "read_file", description: "reads a file", inputSchema: {} }]);
    render(<ToolInspector serverId="s-1" />);

    expect(screen.getByText("read_file")).toBeInTheDocument();
    expect(screen.getByText("reads a file")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("refresh_mcp_tools", { id: "s-1" });
    });
  });
});
