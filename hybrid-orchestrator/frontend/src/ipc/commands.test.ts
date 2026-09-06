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
} from "./commands";

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
});
