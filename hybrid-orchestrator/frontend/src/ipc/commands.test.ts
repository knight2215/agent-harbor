import { describe, it, expect, vi, beforeEach } from "vitest";

// Mock the Tauri IPC bridge so the wrappers can be exercised without a live
// backend, matching the Phase 0 vitest pattern in `src/app.test.tsx`.
const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
}));

import {
  appVersion,
  createConversation,
  deleteConversation,
  listConversations,
  renameConversation,
  setConversationTags,
  listPersonas,
  createPersona,
  updatePersona,
  deletePersona,
  setProviderSecret,
  sendMessage,
  listAvailableModels,
  getMessages,
  setConversationRoute,
  assignPersona,
  getRouteExplanation,
  listMcpServers,
  addMcpServer,
  updateMcpServer,
  removeMcpServer,
  setMcpEnabled,
  refreshMcpTools,
  setToolPermission,
  exportConversation,
  openConversation,
  stopGeneration,
} from "./commands";
import type { McpServerInput } from "../types";

describe("ipc/commands wrappers", () => {
  beforeEach(() => {
    invoke.mockReset();
    invoke.mockResolvedValue(undefined);
  });

  it("appVersion invokes app_version", async () => {
    invoke.mockResolvedValue("1.2.3");
    await expect(appVersion()).resolves.toBe("1.2.3");
    expect(invoke).toHaveBeenCalledWith("app_version");
  });

  it("listConversations invokes list_conversations", async () => {
    invoke.mockResolvedValue([]);
    await listConversations();
    expect(invoke).toHaveBeenCalledWith("list_conversations");
  });

  it("createConversation forwards args (defaulting to an empty object)", async () => {
    await createConversation();
    expect(invoke).toHaveBeenCalledWith("create_conversation", { args: {} });

    await createConversation({ title: "Hi", privacyTags: ["localOnly"] });
    expect(invoke).toHaveBeenLastCalledWith("create_conversation", {
      args: { title: "Hi", privacyTags: ["localOnly"] },
    });
  });

  it("renameConversation forwards conversationId + title", async () => {
    await renameConversation("id-1", "New title");
    expect(invoke).toHaveBeenCalledWith("rename_conversation", {
      conversationId: "id-1",
      title: "New title",
    });
  });

  it("setConversationTags forwards conversationId + tags", async () => {
    await setConversationTags("id-1", ["confidential", { custom: "pii" }]);
    expect(invoke).toHaveBeenCalledWith("set_conversation_tags", {
      conversationId: "id-1",
      tags: ["confidential", { custom: "pii" }],
    });
  });

  it("deleteConversation forwards conversationId", async () => {
    await deleteConversation("id-1");
    expect(invoke).toHaveBeenCalledWith("delete_conversation", { conversationId: "id-1" });
  });

  it("listPersonas invokes list_personas", async () => {
    invoke.mockResolvedValue([]);
    await listPersonas();
    expect(invoke).toHaveBeenCalledWith("list_personas");
  });

  it("createPersona forwards the persona input", async () => {
    await createPersona({ name: "Coder", systemPrompt: "write code" });
    expect(invoke).toHaveBeenCalledWith("create_persona", {
      persona: { name: "Coder", systemPrompt: "write code" },
    });
  });

  it("updatePersona forwards personaId + persona input", async () => {
    await updatePersona("p-1", { name: "Coder v2", systemPrompt: "write more code" });
    expect(invoke).toHaveBeenCalledWith("update_persona", {
      personaId: "p-1",
      persona: { name: "Coder v2", systemPrompt: "write more code" },
    });
  });

  it("deletePersona forwards personaId", async () => {
    await deletePersona("p-1");
    expect(invoke).toHaveBeenCalledWith("delete_persona", { personaId: "p-1" });
  });

  it("setProviderSecret sends the secret IN and returns only the ref handle", async () => {
    invoke.mockResolvedValue("openai");
    const ref = await setProviderSecret("openai", "sk-do-not-leak");
    // The wrapper returns exactly what the command returns: an opaque handle.
    expect(ref).toBe("openai");
    expect(invoke).toHaveBeenCalledWith("set_provider_secret", {
      providerId: "openai",
      secret: "sk-do-not-leak",
    });
  });

  it("sendMessage forwards conversationId + content + overrideRoute", async () => {
    // Automatic routing: overrideRoute is undefined.
    await sendMessage("c-1", "hello");
    expect(invoke).toHaveBeenCalledWith("send_message", {
      conversationId: "c-1",
      content: "hello",
      overrideRoute: undefined,
    });

    // Manual override forwarded verbatim.
    await sendMessage("c-1", "hello", { providerId: "openai", model: "gpt-4o" });
    expect(invoke).toHaveBeenLastCalledWith("send_message", {
      conversationId: "c-1",
      content: "hello",
      overrideRoute: { providerId: "openai", model: "gpt-4o" },
    });
  });

  it("listAvailableModels invokes list_available_models", async () => {
    invoke.mockResolvedValue([]);
    await listAvailableModels();
    expect(invoke).toHaveBeenCalledWith("list_available_models");
  });

  it("getMessages forwards conversationId", async () => {
    invoke.mockResolvedValue([]);
    await getMessages("c-1");
    expect(invoke).toHaveBeenCalledWith("get_messages", { conversationId: "c-1" });
  });

  it("setConversationRoute forwards conversationId + route (and null clears)", async () => {
    await setConversationRoute("c-1", { providerId: "openai", model: "gpt-4o" });
    expect(invoke).toHaveBeenCalledWith("set_conversation_route", {
      conversationId: "c-1",
      route: { providerId: "openai", model: "gpt-4o" },
    });

    await setConversationRoute("c-1", null);
    expect(invoke).toHaveBeenLastCalledWith("set_conversation_route", {
      conversationId: "c-1",
      route: null,
    });
  });

  it("assignPersona forwards conversationId + personaId (and null clears)", async () => {
    await assignPersona("c-1", "p-1");
    expect(invoke).toHaveBeenCalledWith("assign_persona", {
      conversationId: "c-1",
      personaId: "p-1",
    });

    await assignPersona("c-1", null);
    expect(invoke).toHaveBeenLastCalledWith("assign_persona", {
      conversationId: "c-1",
      personaId: null,
    });
  });

  it("getRouteExplanation forwards conversationId", async () => {
    invoke.mockResolvedValue({ rationale: "auto", source: "automatic" });
    await getRouteExplanation("c-1");
    expect(invoke).toHaveBeenCalledWith("get_route_explanation", { conversationId: "c-1" });
  });

  it("listMcpServers invokes list_mcp_servers", async () => {
    invoke.mockResolvedValue([]);
    await listMcpServers();
    expect(invoke).toHaveBeenCalledWith("list_mcp_servers");
  });

  it("addMcpServer forwards the config", async () => {
    const config: McpServerInput = {
      name: "fs",
      transport: { type: "stdio", command: "mcp-fs", args: [], env: [] },
      permissionMode: "ask",
      enabled: true,
    };
    await addMcpServer(config);
    expect(invoke).toHaveBeenCalledWith("add_mcp_server", { config });
  });

  it("updateMcpServer forwards id + config", async () => {
    const config: McpServerInput = {
      name: "remote",
      transport: { type: "httpSse", url: "https://x/sse", headers: [["A", "B"]] },
      permissionMode: "allow",
      enabled: false,
    };
    await updateMcpServer("s-1", config);
    expect(invoke).toHaveBeenCalledWith("update_mcp_server", { id: "s-1", config });
  });

  it("removeMcpServer forwards id", async () => {
    await removeMcpServer("s-1");
    expect(invoke).toHaveBeenCalledWith("remove_mcp_server", { id: "s-1" });
  });

  it("setMcpEnabled forwards id + enabled", async () => {
    await setMcpEnabled("s-1", true);
    expect(invoke).toHaveBeenCalledWith("set_mcp_enabled", { id: "s-1", enabled: true });
  });

  it("refreshMcpTools forwards id", async () => {
    invoke.mockResolvedValue([]);
    await refreshMcpTools("s-1");
    expect(invoke).toHaveBeenCalledWith("refresh_mcp_tools", { id: "s-1" });
  });

  it("setToolPermission forwards serverId + toolName + mode", async () => {
    await setToolPermission("s-1", "read_file", "deny");
    expect(invoke).toHaveBeenCalledWith("set_tool_permission", {
      serverId: "s-1",
      toolName: "read_file",
      mode: "deny",
    });

    await setToolPermission("s-1", null, "allow");
    expect(invoke).toHaveBeenLastCalledWith("set_tool_permission", {
      serverId: "s-1",
      toolName: null,
      mode: "allow",
    });
  });

  it("exportConversation forwards conversationId + format", async () => {
    invoke.mockResolvedValue("# transcript");
    await exportConversation("c-1", "markdown");
    expect(invoke).toHaveBeenCalledWith("export_conversation", {
      conversationId: "c-1",
      format: "markdown",
    });
  });

  it("openConversation forwards conversationId", async () => {
    invoke.mockResolvedValue({ conversation: {}, messages: [] });
    await openConversation("c-1");
    expect(invoke).toHaveBeenCalledWith("open_conversation", { conversationId: "c-1" });
  });

  it("stopGeneration forwards conversationId", async () => {
    await stopGeneration("c-1");
    expect(invoke).toHaveBeenCalledWith("stop_generation", { conversationId: "c-1" });
  });
});
