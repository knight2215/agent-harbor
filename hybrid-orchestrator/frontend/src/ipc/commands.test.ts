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
  providerDiagnostics,
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
  resolvePermission,
  readTextFile,
  readFileBase64,
  listRepoFiles,
  setWebSearchProvider,
  getWebSearchConfig,
  clearWebSearchProvider,
  runWebSearch,
  addNetworkPeer,
  listNetworkPeers,
  removeNetworkPeer,
  setModelSharing,
  getModelSharing,
  discoverNetworkPeers,
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

  it("listAvailableModels invokes list_available_models and returns models + errors", async () => {
    invoke.mockResolvedValue({
      models: [],
      errors: [{ providerId: "ollama-local", message: "transport error" }],
    });
    const result = await listAvailableModels();
    expect(invoke).toHaveBeenCalledWith("list_available_models");
    expect(result.models).toEqual([]);
    expect(result.errors).toHaveLength(1);
    expect(result.errors[0].providerId).toBe("ollama-local");
  });

  it("providerDiagnostics invokes provider_diagnostics and returns the report shape", async () => {
    invoke.mockResolvedValue({
      configuredCount: 1,
      totalModelCount: 0,
      providerCountWithModels: 0,
      providers: [
        {
          id: "ollama-local",
          kind: "ollama",
          baseUrl: null,
          instanceBuilt: true,
          modelCount: 0,
          error: "transport error: connection refused",
        },
      ],
    });
    const report = await providerDiagnostics();
    expect(invoke).toHaveBeenCalledWith("provider_diagnostics");
    expect(report.configuredCount).toBe(1);
    expect(report.providers).toHaveLength(1);
    expect(report.providers[0].id).toBe("ollama-local");
    expect(report.providers[0].instanceBuilt).toBe(true);
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

  it("resolvePermission forwards requestId + decision and returns the awaiting flag", async () => {
    invoke.mockResolvedValue(true);
    await expect(resolvePermission("r-1", { allow: true, remember: false })).resolves.toBe(true);
    expect(invoke).toHaveBeenCalledWith("resolve_permission", {
      requestId: "r-1",
      decision: { allow: true, remember: false },
    });
  });

  it("readTextFile forwards path and returns the file content view (FEAT-003)", async () => {
    invoke.mockResolvedValue({ path: "/tmp/a.txt", name: "a.txt", byteLen: 5, text: "hello" });
    await expect(readTextFile("/tmp/a.txt")).resolves.toEqual({
      path: "/tmp/a.txt",
      name: "a.txt",
      byteLen: 5,
      text: "hello",
    });
    expect(invoke).toHaveBeenCalledWith("read_text_file", { path: "/tmp/a.txt" });
  });

  it("readFileBase64 forwards path and returns the binary view (FEAT-003)", async () => {
    invoke.mockResolvedValue({
      path: "/tmp/a.png",
      name: "a.png",
      mimeType: "image/png",
      base64: "Zm9v",
      byteLen: 3,
    });
    await expect(readFileBase64("/tmp/a.png")).resolves.toMatchObject({ mimeType: "image/png" });
    expect(invoke).toHaveBeenCalledWith("read_file_base64", { path: "/tmp/a.png" });
  });

  it("listRepoFiles forwards dir and returns the listing (FEAT-003)", async () => {
    invoke.mockResolvedValue({
      dir: "/tmp/repo",
      files: [{ relPath: "a.txt", byteLen: 5 }],
      truncated: false,
    });
    await expect(listRepoFiles("/tmp/repo")).resolves.toMatchObject({ truncated: false });
    expect(invoke).toHaveBeenCalledWith("list_repo_files", { dir: "/tmp/repo" });
  });

  // --- Web search (FEAT-004) ---------------------------------------------

  it("setWebSearchProvider forwards kind + apiKey + maxResults + baseUrl and returns the view", async () => {
    invoke.mockResolvedValue({ kind: "tavily", hasApiKey: true, maxResults: 5, baseUrl: null });
    await expect(setWebSearchProvider("tavily", "tvly-key", 5)).resolves.toMatchObject({
      kind: "tavily",
      hasApiKey: true,
    });
    expect(invoke).toHaveBeenCalledWith("set_web_search_provider", {
      kind: "tavily",
      apiKey: "tvly-key",
      maxResults: 5,
      baseUrl: undefined,
    });
  });

  it("setWebSearchProvider forwards a custom endpoint as baseUrl", async () => {
    invoke.mockResolvedValue({
      kind: "custom",
      hasApiKey: true,
      maxResults: 5,
      baseUrl: "https://search.example.com",
    });
    await expect(
      setWebSearchProvider("custom", "custom-key", 5, "https://search.example.com"),
    ).resolves.toMatchObject({ kind: "custom", baseUrl: "https://search.example.com" });
    expect(invoke).toHaveBeenCalledWith("set_web_search_provider", {
      kind: "custom",
      apiKey: "custom-key",
      maxResults: 5,
      baseUrl: "https://search.example.com",
    });
  });

  it("getWebSearchConfig invokes get_web_search_config", async () => {
    invoke.mockResolvedValue(null);
    await expect(getWebSearchConfig()).resolves.toBeNull();
    expect(invoke).toHaveBeenCalledWith("get_web_search_config");
  });

  it("clearWebSearchProvider invokes clear_web_search_provider", async () => {
    await clearWebSearchProvider();
    expect(invoke).toHaveBeenCalledWith("clear_web_search_provider");
  });

  it("runWebSearch forwards the query and returns display-safe results", async () => {
    invoke.mockResolvedValue([{ title: "T", url: "https://ex.com", snippet: "s" }]);
    await expect(runWebSearch("rust async")).resolves.toMatchObject([{ title: "T" }]);
    expect(invoke).toHaveBeenCalledWith("run_web_search", { query: "rust async" });
  });

  // --- Local network (LAN) model sharing (FEAT-006) ------------------------

  it("addNetworkPeer forwards baseUrl/label/apiKey", async () => {
    invoke.mockResolvedValue({
      id: "network-peer-1",
      label: "box",
      baseUrl: "http://192.168.1.5:11435/v1",
      hasApiKey: false,
      warning: null,
    });
    await addNetworkPeer("http://192.168.1.5:11435/v1", "box", null);
    expect(invoke).toHaveBeenCalledWith("add_network_peer", {
      baseUrl: "http://192.168.1.5:11435/v1",
      label: "box",
      apiKey: null,
    });
  });

  it("listNetworkPeers invokes list_network_peers", async () => {
    invoke.mockResolvedValue([]);
    await listNetworkPeers();
    expect(invoke).toHaveBeenCalledWith("list_network_peers");
  });

  it("removeNetworkPeer forwards the id", async () => {
    await removeNetworkPeer("network-peer-1");
    expect(invoke).toHaveBeenCalledWith("remove_network_peer", { id: "network-peer-1" });
  });

  it("setModelSharing forwards enabled + port", async () => {
    invoke.mockResolvedValue({ enabled: true, port: 11435, status: "Sharing on port 11435." });
    await setModelSharing(true, 11435);
    expect(invoke).toHaveBeenCalledWith("set_model_sharing", { enabled: true, port: 11435 });
  });

  it("getModelSharing invokes get_model_sharing", async () => {
    invoke.mockResolvedValue({ enabled: false, port: 11435, status: "Sharing is off." });
    await getModelSharing();
    expect(invoke).toHaveBeenCalledWith("get_model_sharing");
  });

  it("discoverNetworkPeers invokes discover_network_peers", async () => {
    invoke.mockResolvedValue([]);
    await discoverNetworkPeers();
    expect(invoke).toHaveBeenCalledWith("discover_network_peers");
  });
});
