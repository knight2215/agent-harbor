import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, cleanup, waitFor } from "@testing-library/react";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn().mockResolvedValue(() => undefined),
}));
// The About / Updates section reads getVersion() on mount and calls the updater
// plugins on interaction; mock all three so switching to that section does not
// hang the suite on a live Tauri runtime.
vi.mock("@tauri-apps/api/app", () => ({
  getVersion: vi.fn().mockResolvedValue("0.4.0"),
}));
vi.mock("@tauri-apps/plugin-updater", () => ({
  check: vi.fn().mockResolvedValue(null),
}));
vi.mock("@tauri-apps/plugin-process", () => ({
  relaunch: vi.fn().mockResolvedValue(undefined),
}));
// The embedded engine "Browse…" button opens the native OS file dialog via the
// dialog plugin; mock open() so the picker resolves synchronously in tests.
vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(),
}));

import { Settings } from "./Settings";

describe("Settings", () => {
  beforeEach(() => {
    invoke.mockReset();
    invoke.mockResolvedValue([]);
    window.localStorage.clear();
  });

  it("renders the sub-navigation and each section by accessible name", async () => {
    // The Diagnostics section calls provider_diagnostics on mount; return a
    // well-formed report so navigating to it never resolves undefined.
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "provider_diagnostics") {
        return Promise.resolve({
          configuredCount: 0,
          totalModelCount: 0,
          providerCountWithModels: 0,
          providers: [],
        });
      }
      // FEAT-004: the Web Search section reads this on mount; unconfigured.
      if (cmd === "get_web_search_config") {
        return Promise.resolve(null);
      }
      // FEAT-006: the Network Sharing section reads these on mount.
      if (cmd === "get_model_sharing") {
        return Promise.resolve({ enabled: false, port: 11435, status: "Sharing is off." });
      }
      if (cmd === "list_network_peers") {
        return Promise.resolve([]);
      }
      // FEAT-002: the Diagnostics section's "Test key storage" button invokes
      // this; return a well-formed { ok, detail } view.
      if (cmd === "test_key_storage") {
        return Promise.resolve({ ok: true, detail: "Keychain round-trip succeeded." });
      }
      return Promise.resolve([]);
    });
    render(<Settings />);

    const nav = screen.getByRole("navigation", { name: "Settings sections" });
    expect(nav).toBeInTheDocument();

    // Providers & Keys is the default section.
    expect(screen.getByRole("region", { name: "Providers & Keys" })).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Local Runtimes" }));
    expect(await screen.findByRole("region", { name: "Local Runtimes" })).toBeInTheDocument();
    // While the Local Runtimes section is the active (mounted) section, assert
    // its informational note and embedded-engine surface. Only the active
    // section is mounted (Settings renders sections conditionally), so these
    // must be checked here, before navigating away to another section.
    //
    // The informational note for genuinely-future runtimes is present and
    // labeled, and it no longer claims Ollama (shipped) is "coming soon" nor
    // that the embedded engine is unavailable.
    const note = await screen.findByRole("region", { name: "More local runtimes coming soon" });
    expect(note).toBeInTheDocument();
    expect(screen.getByTestId("local-runtimes-empty-state")).toBeInTheDocument();
    expect(note).toHaveTextContent(/Hugging Face/);
    expect(note).toHaveTextContent(/Ollama/);
    expect(note).toHaveTextContent(/embedded engine/);
    // The embedded engine now has a first-class surface (not "coming soon").
    expect(
      await screen.findByRole("region", { name: "Embedded inference engine" }),
    ).toBeInTheDocument();

    // FEAT-004: the Web Search section is registered and renders by name.
    fireEvent.click(screen.getByRole("button", { name: "Web Search" }));
    expect(await screen.findByRole("region", { name: "Web Search" })).toBeInTheDocument();

    // FEAT-006: the Network Sharing section is registered and renders by name.
    fireEvent.click(screen.getByRole("button", { name: "Network Sharing" }));
    expect(await screen.findByRole("region", { name: "Network Sharing" })).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "MCP / Tools" }));
    expect(await screen.findByRole("region", { name: "Tool manager" })).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Agents" }));
    expect(await screen.findByRole("region", { name: "Agent editor" })).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Routing" }));
    expect(await screen.findByRole("region", { name: "Routing" })).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Appearance" }));
    expect(await screen.findByRole("region", { name: "Appearance" })).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Diagnostics" }));
    expect(await screen.findByRole("region", { name: "Diagnostics" })).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "About / Updates" }));
    expect(await screen.findByRole("region", { name: "About / Updates" })).toBeInTheDocument();
  });

  it("renders the Diagnostics section with a summary from provider_diagnostics", async () => {
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "provider_diagnostics") {
        return Promise.resolve({
          configuredCount: 2,
          totalModelCount: 3,
          providerCountWithModels: 1,
          providers: [
            {
              id: "ollama-local",
              kind: "ollama",
              baseUrl: null,
              instanceBuilt: true,
              modelCount: 0,
              error: "transport error: connection refused",
            },
            {
              id: "openai-cloud",
              kind: "openAI",
              baseUrl: "https://api.openai.com/v1",
              instanceBuilt: true,
              modelCount: 3,
              error: null,
            },
          ],
        });
      }
      if (cmd === "test_key_storage") {
        return Promise.resolve({ ok: true, detail: "Keychain round-trip succeeded." });
      }
      return Promise.resolve([]);
    });
    render(<Settings />);

    fireEvent.click(screen.getByRole("button", { name: "Diagnostics" }));
    const region = await screen.findByRole("region", { name: "Diagnostics" });
    expect(region).toBeInTheDocument();

    // The summary line reflects the well-formed report.
    expect(await screen.findByTestId("diagnostics-summary")).toHaveTextContent(
      "loaded: 3 models from 1 of 2 configured providers",
    );
    // The per-provider list shows display-safe fields (baseUrl only, "default"
    // when null, instance-built yes/no, model counts, and any error).
    const list = await screen.findByTestId("diagnostics-providers");
    expect(list).toHaveTextContent("ollama-local");
    expect(list).toHaveTextContent("default");
    expect(list).toHaveTextContent("transport error: connection refused");
    expect(list).toHaveTextContent("https://api.openai.com/v1");
    // A Run diagnostics / Refresh button re-invokes the command.
    invoke.mockClear();
    fireEvent.click(screen.getByTestId("diagnostics-refresh"));
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("provider_diagnostics");
    });
  });

  it("shows 'failed: <error>' rather than a blank Diagnostics panel when the IPC throws", async () => {
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "provider_diagnostics") {
        return Promise.reject(new Error("provider registry build failed"));
      }
      return Promise.resolve([]);
    });
    render(<Settings />);

    fireEvent.click(screen.getByRole("button", { name: "Diagnostics" }));
    expect(await screen.findByRole("region", { name: "Diagnostics" })).toBeInTheDocument();
    expect(await screen.findByTestId("diagnostics-summary")).toHaveTextContent(
      "failed: provider registry build failed",
    );
  });

  it("runs the keychain self-test and renders the ok/detail result", async () => {
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "provider_diagnostics") {
        return Promise.resolve({
          configuredCount: 0,
          totalModelCount: 0,
          providerCountWithModels: 0,
          providers: [],
        });
      }
      if (cmd === "test_key_storage") {
        return Promise.resolve({
          ok: true,
          detail: "Keychain round-trip succeeded: secrets persist on this system.",
        });
      }
      return Promise.resolve([]);
    });
    render(<Settings />);

    fireEvent.click(screen.getByRole("button", { name: "Diagnostics" }));
    expect(await screen.findByRole("region", { name: "Diagnostics" })).toBeInTheDocument();

    // Before clicking, the self-test has not run.
    expect(screen.getByTestId("key-storage-summary")).toHaveTextContent("not tested yet");

    // Clicking "Test key storage" invokes the command and renders the result.
    fireEvent.click(screen.getByRole("button", { name: "Test key storage" }));
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("test_key_storage");
    });
    expect(await screen.findByTestId("key-storage-summary")).toHaveTextContent(
      "ok: Keychain round-trip succeeded: secrets persist on this system.",
    );
  });

  it("shows 'failed: <error>' rather than a blank panel when test_key_storage throws", async () => {
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "provider_diagnostics") {
        return Promise.resolve({
          configuredCount: 0,
          totalModelCount: 0,
          providerCountWithModels: 0,
          providers: [],
        });
      }
      if (cmd === "test_key_storage") {
        return Promise.reject(new Error("keychain unavailable"));
      }
      return Promise.resolve([]);
    });
    render(<Settings />);

    fireEvent.click(screen.getByRole("button", { name: "Diagnostics" }));
    expect(await screen.findByRole("region", { name: "Diagnostics" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Test key storage" }));
    expect(await screen.findByTestId("key-storage-summary")).toHaveTextContent(
      "failed: keychain unavailable",
    );
  });

  it("registers a cloud provider via set_cloud_provider without retaining plaintext", async () => {
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "list_cloud_providers") return Promise.resolve([]);
      if (cmd === "set_cloud_provider") {
        return Promise.resolve({
          id: "openai-cloud",
          kind: "openAI",
          baseUrl: null,
          hasApiKey: true,
          warning: null,
        });
      }
      return Promise.resolve([]);
    });
    render(<Settings />);

    // The default selected provider is a cloud KIND (dropdown), not free text.
    fireEvent.change(screen.getByLabelText("API key"), { target: { value: "sk-test" } });
    fireEvent.click(screen.getByRole("button", { name: "Save key" }));

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("set_cloud_provider", {
        kind: "openAI",
        apiKey: "sk-test",
        baseUrl: null,
      });
    });
    // The "configured" indication shows the provider by kind (display-safe).
    expect(await screen.findByTestId("provider-key-configured-openAI")).toBeInTheDocument();
    // The plaintext key is cleared from the field after saving.
    expect((screen.getByLabelText("API key") as HTMLInputElement).value).toBe("");
  });

  it("requires a base URL when the Kiro (genericOpenAI) provider is selected", async () => {
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "list_cloud_providers") return Promise.resolve([]);
      if (cmd === "set_cloud_provider") {
        return Promise.resolve({
          id: "generic-openai-cloud",
          kind: "genericOpenAI",
          baseUrl: "https://kiro.example.com/v1",
          hasApiKey: true,
          warning: null,
        });
      }
      return Promise.resolve([]);
    });
    render(<Settings />);

    // Select Kiro; the Base URL field appears and is required.
    fireEvent.change(screen.getByLabelText("Provider"), { target: { value: "genericOpenAI" } });
    fireEvent.change(screen.getByLabelText("API key"), { target: { value: "sk-kiro" } });
    // Saving without a base URL surfaces a validation error and does not call
    // the backend.
    fireEvent.click(screen.getByRole("button", { name: "Save key" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(/base URL is required/i);
    expect(invoke).not.toHaveBeenCalledWith("set_cloud_provider", expect.anything());

    // Providing the base URL then saves via set_cloud_provider with it.
    fireEvent.change(screen.getByLabelText("Base URL"), {
      target: { value: "https://kiro.example.com/v1" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save key" }));

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("set_cloud_provider", {
        kind: "genericOpenAI",
        apiKey: "sk-kiro",
        baseUrl: "https://kiro.example.com/v1",
      });
    });
    expect(await screen.findByTestId("provider-key-configured-genericOpenAI")).toBeInTheDocument();
  });

  it("displays the backend warning for an accepted plaintext non-loopback Kiro URL", async () => {
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "list_cloud_providers") return Promise.resolve([]);
      if (cmd === "set_cloud_provider") {
        return Promise.resolve({
          id: "generic-openai-cloud",
          kind: "genericOpenAI",
          baseUrl: "http://example.com/v1",
          hasApiKey: true,
          warning: "This endpoint uses plaintext http over a non-loopback address.",
        });
      }
      return Promise.resolve([]);
    });
    render(<Settings />);

    // Select Kiro and save with a plaintext, non-loopback base URL.
    fireEvent.change(screen.getByLabelText("Provider"), { target: { value: "genericOpenAI" } });
    fireEvent.change(screen.getByLabelText("API key"), { target: { value: "sk-kiro" } });
    fireEvent.change(screen.getByLabelText("Base URL"), {
      target: { value: "http://example.com/v1" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save key" }));

    // The advisory is shown (role="status", distinct from the error region)
    // without blocking the save (the provider is still configured).
    expect(await screen.findByTestId("cloud-provider-warning")).toHaveTextContent(
      "This endpoint uses plaintext http over a non-loopback address.",
    );
    expect(await screen.findByTestId("provider-key-configured-genericOpenAI")).toBeInTheDocument();
  });

  it("shows a non-blocking advisory for a session-style Kiro base URL and still saves", async () => {
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "list_cloud_providers") return Promise.resolve([]);
      if (cmd === "list_available_models") return Promise.resolve({ models: [], errors: [] });
      if (cmd === "set_cloud_provider") {
        return Promise.resolve({
          id: "generic-openai-cloud",
          kind: "genericOpenAI",
          baseUrl: "https://app.kiro.dev/session/abc",
          hasApiKey: true,
          warning:
            'base_url "https://app.kiro.dev/session/abc" looks like a web/session or model endpoint URL, not an OpenAI-compatible API base URL; enter the API root (e.g. https://host/v1) so models can be enumerated',
        });
      }
      return Promise.resolve([]);
    });
    render(<Settings />);

    fireEvent.change(screen.getByLabelText("Provider"), { target: { value: "genericOpenAI" } });
    fireEvent.change(screen.getByLabelText("API key"), { target: { value: "sk-kiro" } });
    fireEvent.change(screen.getByLabelText("Base URL"), {
      target: { value: "https://app.kiro.dev/session/abc" },
    });

    // The live client advisory (role="status", distinct from the blocking error
    // region) appears as soon as the session-style URL is entered, before save.
    const advisory = await screen.findByTestId("cloud-provider-base-url-advisory");
    expect(advisory).toHaveTextContent(/web\/session/i);
    expect(advisory).toHaveAttribute("role", "status");
    // The inline help steers toward an API base URL.
    expect(screen.getByTestId("cloud-base-url-help")).toHaveTextContent(
      /OpenAI-compatible API base URL/i,
    );

    // Saving is NOT blocked by the advisory: the backend is still invoked and
    // its own advisory surfaces in the warning region.
    fireEvent.click(screen.getByRole("button", { name: "Save key" }));
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("set_cloud_provider", {
        kind: "genericOpenAI",
        apiKey: "sk-kiro",
        baseUrl: "https://app.kiro.dev/session/abc",
      });
    });
    expect(await screen.findByTestId("provider-key-configured-genericOpenAI")).toBeInTheDocument();
    expect(await screen.findByTestId("cloud-provider-warning")).toHaveTextContent(/web\/session/i);
  });

  it("rehydrates configured cloud providers on mount and persists across unmount/remount", async () => {
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "list_cloud_providers") {
        return Promise.resolve([
          { id: "gemini-cloud", kind: "gemini", baseUrl: null, hasApiKey: true, warning: null },
        ]);
      }
      return Promise.resolve([]);
    });
    render(<Settings />);

    // The configured provider is shown from the mount-time rehydration.
    expect(await screen.findByTestId("provider-key-configured-gemini")).toBeInTheDocument();
    expect(invoke).toHaveBeenCalledWith("list_cloud_providers");

    // Simulate navigating away and back: unmount, then mount a fresh tree. It
    // rehydrates from the backend source of truth (not a localStorage marker).
    cleanup();
    render(<Settings />);
    expect(await screen.findByTestId("provider-key-configured-gemini")).toBeInTheDocument();
  });
});

import { open } from "@tauri-apps/plugin-dialog";
import { LocalRuntimesSection } from "./LocalRuntimesSection";
import type { EmbeddedModelStatus, EmbeddedModelView } from "../../types";

const openMock = vi.mocked(open);

function status(loadedModelId: string | null, registeredCount: number): EmbeddedModelStatus {
  return { loadedModelId, registeredCount };
}

function view(id: string, path: string, loaded: boolean): EmbeddedModelView {
  return { id, path, loaded };
}

describe("LocalRuntimesSection embedded engine", () => {
  beforeEach(() => {
    invoke.mockReset();
    openMock.mockReset();
    window.localStorage.clear();
  });

  it("renders the embedded engine as an available Local runtime (not coming soon)", async () => {
    // The mount-time probes resolve to an empty engine (no imports, nothing
    // selected).
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "list_local_runtimes") return Promise.resolve([]);
      if (cmd === "list_embedded_models") return Promise.resolve([]);
      if (cmd === "embedded_model_status") return Promise.resolve(status(null, 0));
      return Promise.resolve(undefined);
    });
    render(<LocalRuntimesSection />);

    expect(screen.getByRole("region", { name: "Embedded inference engine" })).toBeInTheDocument();
    // The status line reflects nothing selected initially.
    expect(await screen.findByTestId("embedded-status")).toHaveTextContent(
      "No embedded model is selected.",
    );
    // The reconciled note lists Ollama + embedded as available and only keeps
    // genuinely-future runtimes as "coming soon".
    const note = screen.getByTestId("local-runtimes-empty-state");
    expect(note).toHaveTextContent(/Ollama/);
    expect(note).toHaveTextContent(/embedded engine/);
    expect(note).toHaveTextContent(/Hugging Face/);
  });

  it("hydrates previously-imported models on mount", async () => {
    // A prior-session import is returned by list_embedded_models; the status
    // probe reports it as the active model.
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "list_local_runtimes") return Promise.resolve([]);
      if (cmd === "list_embedded_models") {
        return Promise.resolve([view("phi-3-mini", "/models/phi-3-mini.gguf", true)]);
      }
      if (cmd === "embedded_model_status") return Promise.resolve(status("phi-3-mini", 1));
      return Promise.resolve(undefined);
    });
    render(<LocalRuntimesSection />);

    // The list is populated from the mount probe (no import action this
    // session), and the status line reflects the active selection.
    expect(await screen.findByTestId("embedded-model-list")).toHaveTextContent(
      "/models/phi-3-mini.gguf",
    );
    expect(await screen.findByTestId("embedded-status")).toHaveTextContent(
      "Active model: phi-3-mini",
    );
    expect(invoke).toHaveBeenCalledWith("list_embedded_models");
  });

  it("imports a local .gguf and lists it", async () => {
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "list_local_runtimes") return Promise.resolve([]);
      if (cmd === "list_embedded_models") return Promise.resolve([]);
      if (cmd === "embedded_model_status") return Promise.resolve(status(null, 0));
      if (cmd === "import_embedded_model") {
        return Promise.resolve([view("model.gguf", "/models/model.gguf", false)]);
      }
      return Promise.resolve(undefined);
    });
    render(<LocalRuntimesSection />);

    fireEvent.change(screen.getByTestId("embedded-model-path"), {
      target: { value: "/models/model.gguf" },
    });
    fireEvent.click(screen.getByTestId("embedded-import"));

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("import_embedded_model", {
        path: "/models/model.gguf",
      });
    });
    // The imported model appears in the list.
    expect(await screen.findByTestId("embedded-model-list")).toHaveTextContent(
      "/models/model.gguf",
    );
  });

  it("loads a selected .gguf and reflects the loaded status, then unloads", async () => {
    let loaded = false;
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "list_local_runtimes") return Promise.resolve([]);
      if (cmd === "list_embedded_models") return Promise.resolve([]);
      if (cmd === "embedded_model_status") {
        return Promise.resolve(loaded ? status("model.gguf", 1) : status(null, loaded ? 1 : 0));
      }
      if (cmd === "select_embedded_model") {
        loaded = true;
        return Promise.resolve([view("model.gguf", "/models/model.gguf", true)]);
      }
      if (cmd === "unload_embedded_model") {
        loaded = false;
        return Promise.resolve(status(null, 1));
      }
      return Promise.resolve(undefined);
    });
    render(<LocalRuntimesSection />);

    fireEvent.change(screen.getByTestId("embedded-model-path"), {
      target: { value: "/models/model.gguf" },
    });
    fireEvent.click(screen.getByTestId("embedded-load"));

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("select_embedded_model", {
        path: "/models/model.gguf",
      });
    });
    // The status line reflects the now-active model.
    expect(await screen.findByTestId("embedded-status")).toHaveTextContent(
      "Active model: model.gguf",
    );

    // Clearing the selection resets the active-model state.
    fireEvent.click(screen.getByTestId("embedded-unload"));
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("unload_embedded_model");
    });
    expect(await screen.findByTestId("embedded-status")).toHaveTextContent(
      "No embedded model is selected.",
    );
  });

  it("loads an already-imported model by id from the list", async () => {
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "list_local_runtimes") return Promise.resolve([]);
      if (cmd === "list_embedded_models") return Promise.resolve([]);
      if (cmd === "embedded_model_status") return Promise.resolve(status(null, 1));
      if (cmd === "import_embedded_model") {
        return Promise.resolve([view("model.gguf", "/models/model.gguf", false)]);
      }
      if (cmd === "load_embedded_model") return Promise.resolve(status("model.gguf", 1));
      return Promise.resolve(undefined);
    });
    render(<LocalRuntimesSection />);

    // Import first so the list has an entry with a per-row Load button.
    fireEvent.change(screen.getByTestId("embedded-model-path"), {
      target: { value: "/models/model.gguf" },
    });
    fireEvent.click(screen.getByTestId("embedded-import"));

    const rowLoad = await screen.findByTestId("embedded-load-model.gguf");
    fireEvent.click(rowLoad);

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("load_embedded_model", { modelId: "model.gguf" });
    });
    expect(await screen.findByTestId("embedded-status")).toHaveTextContent(
      "Active model: model.gguf",
    );
  });

  it("browses for a .gguf via the native dialog and fills the model-path input", async () => {
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "list_local_runtimes") return Promise.resolve([]);
      if (cmd === "list_embedded_models") return Promise.resolve([]);
      if (cmd === "embedded_model_status") return Promise.resolve(status(null, 0));
      return Promise.resolve(undefined);
    });
    // The native file picker resolves to the chosen absolute path.
    openMock.mockResolvedValue("/models/picked.gguf");
    render(<LocalRuntimesSection />);

    fireEvent.click(screen.getByTestId("embedded-model-browse"));

    // The dialog is opened as a single-file picker filtered to `.gguf`.
    await waitFor(() => {
      expect(openMock).toHaveBeenCalledWith({
        multiple: false,
        directory: false,
        filters: [{ name: "GGUF model", extensions: ["gguf"] }],
      });
    });
    // The chosen path is dropped into the existing model-path input so the
    // Import/Select buttons work unchanged.
    await waitFor(() => {
      expect((screen.getByTestId("embedded-model-path") as HTMLInputElement).value).toBe(
        "/models/picked.gguf",
      );
    });
  });

  it("leaves the model-path input unchanged when the file dialog is cancelled", async () => {
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "list_local_runtimes") return Promise.resolve([]);
      if (cmd === "list_embedded_models") return Promise.resolve([]);
      if (cmd === "embedded_model_status") return Promise.resolve(status(null, 0));
      return Promise.resolve(undefined);
    });
    // Cancelling the picker resolves to null; the input stays empty (no-op).
    openMock.mockResolvedValue(null);
    render(<LocalRuntimesSection />);

    fireEvent.click(screen.getByTestId("embedded-model-browse"));

    await waitFor(() => {
      expect(openMock).toHaveBeenCalled();
    });
    expect((screen.getByTestId("embedded-model-path") as HTMLInputElement).value).toBe("");
  });
});

import type { LocalRuntimeConfig, ProviderKind } from "../../types";

function runtime(
  kind: ProviderKind,
  baseUrl: string,
  warning: string | null = null,
  hasApiKey = false,
): LocalRuntimeConfig {
  const id = kind === "lmStudio" ? "lmstudio-local" : "generic-openai-local";
  return { id, kind, baseUrl, hasApiKey, warning };
}

describe("LocalRuntimesSection OpenAI-compatible runtimes", () => {
  beforeEach(() => {
    invoke.mockReset();
    window.localStorage.clear();
  });

  it("saves the LM Studio form via set_local_runtime with a null key and reflects it", async () => {
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "list_local_runtimes") return Promise.resolve([]);
      if (cmd === "list_embedded_models") return Promise.resolve([]);
      if (cmd === "embedded_model_status") return Promise.resolve(status(null, 0));
      if (cmd === "set_local_runtime") {
        return Promise.resolve(runtime("lmStudio", "http://localhost:1234/v1"));
      }
      return Promise.resolve(undefined);
    });
    render(<LocalRuntimesSection />);

    fireEvent.change(screen.getByLabelText("Base URL"), {
      target: { value: "http://localhost:1234/v1" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save runtime" }));

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("set_local_runtime", {
        kind: "lmStudio",
        baseUrl: "http://localhost:1234/v1",
        apiKey: null,
      });
    });
    // The configured runtime is reflected as a persistent line.
    expect(await screen.findByTestId("local-runtime-configured-lmStudio")).toHaveTextContent(
      "LM Studio configured at http://localhost:1234/v1",
    );
    // The (optional) key field is cleared after saving.
    expect((screen.getByLabelText("API key (optional)") as HTMLInputElement).value).toBe("");
  });

  it("displays the backend warning for an accepted plaintext non-loopback URL", async () => {
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "list_local_runtimes") return Promise.resolve([]);
      if (cmd === "list_embedded_models") return Promise.resolve([]);
      if (cmd === "embedded_model_status") return Promise.resolve(status(null, 0));
      if (cmd === "set_local_runtime") {
        return Promise.resolve(
          runtime(
            "genericOpenAI",
            "http://192.168.1.10:1234/v1",
            "This endpoint uses plaintext http over a non-loopback address.",
          ),
        );
      }
      return Promise.resolve(undefined);
    });
    render(<LocalRuntimesSection />);

    fireEvent.change(screen.getByLabelText("Runtime"), { target: { value: "genericOpenAI" } });
    fireEvent.change(screen.getByLabelText("Base URL"), {
      target: { value: "http://192.168.1.10:1234/v1" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save runtime" }));

    // The warning is shown without blocking the save (the runtime is still
    // configured).
    expect(await screen.findByTestId("local-runtime-warning")).toHaveTextContent(
      "This endpoint uses plaintext http over a non-loopback address.",
    );
    expect(await screen.findByTestId("local-runtime-configured-genericOpenAI")).toBeInTheDocument();
  });

  it("shows a non-blocking advisory for a :generateContent local base URL and still saves", async () => {
    const url =
      "https://generativelanguage.googleapis.com/v1beta/models/gemini-flash-latest:generateContent";
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "list_local_runtimes") return Promise.resolve([]);
      if (cmd === "list_embedded_models") return Promise.resolve([]);
      if (cmd === "embedded_model_status") return Promise.resolve(status(null, 0));
      if (cmd === "set_local_runtime") {
        return Promise.resolve(
          runtime(
            "genericOpenAI",
            url,
            "base_url looks like a web/session or model endpoint URL, not an OpenAI-compatible API base URL; enter the API root (e.g. https://host/v1) so models can be enumerated",
          ),
        );
      }
      return Promise.resolve(undefined);
    });
    render(<LocalRuntimesSection />);

    fireEvent.change(screen.getByLabelText("Runtime"), { target: { value: "genericOpenAI" } });
    fireEvent.change(screen.getByLabelText("Base URL"), { target: { value: url } });

    // The live client advisory (role="status") appears for the Gemini
    // generateContent URL, with a gentle hint to use the Gemini kind, before save.
    const advisory = await screen.findByTestId("local-runtime-base-url-advisory");
    expect(advisory).toHaveTextContent(/API base URL/i);
    expect(advisory).toHaveTextContent(/Gemini kind/i);
    expect(advisory).toHaveAttribute("role", "status");
    // The inline help is present for the generic runtime field.
    expect(screen.getByTestId("local-base-url-help")).toHaveTextContent(
      /OpenAI-compatible API base URL/i,
    );

    // Saving is NOT blocked: the backend is invoked and its advisory surfaces.
    fireEvent.click(screen.getByRole("button", { name: "Save runtime" }));
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("set_local_runtime", {
        kind: "genericOpenAI",
        baseUrl: url,
        apiKey: null,
      });
    });
    expect(await screen.findByTestId("local-runtime-configured-genericOpenAI")).toBeInTheDocument();
    expect(await screen.findByTestId("local-runtime-warning")).toHaveTextContent(/API base URL/i);
  });

  it("shows an error and persists nothing when the base URL is blocked", async () => {
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "list_local_runtimes") return Promise.resolve([]);
      if (cmd === "list_embedded_models") return Promise.resolve([]);
      if (cmd === "embedded_model_status") return Promise.resolve(status(null, 0));
      if (cmd === "set_local_runtime") {
        return Promise.reject("base_url resolves to a blocked internal host");
      }
      return Promise.resolve(undefined);
    });
    render(<LocalRuntimesSection />);

    fireEvent.change(screen.getByLabelText("Base URL"), {
      target: { value: "http://169.254.169.254/v1" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save runtime" }));

    // The rejection surfaces as a visible validation error.
    expect(await screen.findByTestId("local-runtime-error")).toHaveTextContent(
      "base_url resolves to a blocked internal host",
    );
    // Nothing is configured (no runtime line rendered).
    expect(screen.queryByTestId("local-runtime-configured-lmStudio")).not.toBeInTheDocument();
  });

  it("rehydrates configured runtimes on mount and persists across unmount/remount", async () => {
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "list_local_runtimes") {
        return Promise.resolve([runtime("lmStudio", "http://localhost:1234/v1")]);
      }
      if (cmd === "list_embedded_models") return Promise.resolve([]);
      if (cmd === "embedded_model_status") return Promise.resolve(status(null, 0));
      return Promise.resolve(undefined);
    });
    render(<LocalRuntimesSection />);

    // The configured runtime is shown from the mount-time rehydration.
    expect(await screen.findByTestId("local-runtime-configured-lmStudio")).toHaveTextContent(
      "LM Studio configured at http://localhost:1234/v1",
    );
    expect(invoke).toHaveBeenCalledWith("list_local_runtimes");

    // Simulate navigating away and back: unmount, then mount a fresh tree
    // (which resets in-component state to defaults). It rehydrates from the
    // backend source of truth.
    cleanup();
    render(<LocalRuntimesSection />);
    expect(await screen.findByTestId("local-runtime-configured-lmStudio")).toHaveTextContent(
      "LM Studio configured at http://localhost:1234/v1",
    );
  });

  it("clears a configured runtime via clear_local_runtime and removes the line", async () => {
    let configured = true;
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "list_local_runtimes") {
        return Promise.resolve(configured ? [runtime("lmStudio", "http://localhost:1234/v1")] : []);
      }
      if (cmd === "list_embedded_models") return Promise.resolve([]);
      if (cmd === "embedded_model_status") return Promise.resolve(status(null, 0));
      if (cmd === "clear_local_runtime") {
        configured = false;
        return Promise.resolve(undefined);
      }
      return Promise.resolve(undefined);
    });
    render(<LocalRuntimesSection />);

    const clear = await screen.findByTestId("local-runtime-clear-lmStudio");
    fireEvent.click(clear);

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("clear_local_runtime", { kind: "lmStudio" });
    });
    // The configured line is removed after the refreshed list comes back empty.
    await waitFor(() => {
      expect(screen.queryByTestId("local-runtime-configured-lmStudio")).not.toBeInTheDocument();
    });
  });
});

import { WebSearchSection } from "./WebSearchSection";
import type { WebSearchConfigView } from "../../types";

// FEAT-004: the Web Search settings section saves / clears / rehydrates the
// configured provider through the web-search commands. Every mounting test
// mocks `get_web_search_config` (read on mount) with a well-formed shape; the
// key is write-only (never displayed) and every input has an explicit
// aria-label (help-text-wrapping bug guard).
describe("WebSearchSection", () => {
  beforeEach(() => {
    invoke.mockReset();
    invoke.mockResolvedValue(null);
    window.localStorage.clear();
  });

  const view = (
    kind: WebSearchConfigView["kind"],
    maxResults: number,
    baseUrl: string | null = null,
  ): WebSearchConfigView => ({
    kind,
    hasApiKey: true,
    maxResults,
    baseUrl,
  });

  it("rehydrates the configured provider from get_web_search_config on mount", async () => {
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "get_web_search_config") return Promise.resolve(view("tavily", 7));
      return Promise.resolve(null);
    });
    render(<WebSearchSection />);

    // The configured line rehydrates with the provider label + result count.
    expect(await screen.findByTestId("web-search-configured-tavily")).toBeInTheDocument();
    // Every input is reachable by its explicit accessible name.
    expect(screen.getByLabelText("Web search provider")).toBeInTheDocument();
    expect(screen.getByLabelText("Web search API key")).toBeInTheDocument();
    expect(screen.getByLabelText("Web search max results")).toHaveValue(7);
  });

  it("saves the provider + key and never displays the key", async () => {
    invoke.mockImplementation((cmd: string, args?: Record<string, unknown>) => {
      if (cmd === "get_web_search_config") return Promise.resolve(null);
      if (cmd === "set_web_search_provider") {
        return Promise.resolve(view((args?.kind as WebSearchConfigView["kind"]) ?? "tavily", 5));
      }
      return Promise.resolve(null);
    });
    render(<WebSearchSection />);

    // Enter a key and save.
    const keyInput = screen.getByLabelText("Web search API key") as HTMLInputElement;
    fireEvent.change(keyInput, { target: { value: "tvly-secret-key" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("set_web_search_provider", {
        kind: "tavily",
        apiKey: "tvly-secret-key",
        maxResults: 5,
        baseUrl: null,
      });
    });
    // The configured line appears and the key input is cleared (write-only).
    expect(await screen.findByTestId("web-search-configured-tavily")).toBeInTheDocument();
    expect((screen.getByLabelText("Web search API key") as HTMLInputElement).value).toBe("");
    // The key is a password input and is never rendered as visible text.
    expect(keyInput.type).toBe("password");
    expect(screen.queryByText("tvly-secret-key")).not.toBeInTheDocument();
  });

  it("clears the configured provider", async () => {
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "get_web_search_config") return Promise.resolve(view("tavily", 5));
      if (cmd === "clear_web_search_provider") return Promise.resolve(undefined);
      return Promise.resolve(null);
    });
    render(<WebSearchSection />);

    const clear = await screen.findByTestId("web-search-clear");
    fireEvent.click(clear);

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("clear_web_search_provider");
    });
    await waitFor(() => {
      expect(screen.queryByTestId("web-search-configured-tavily")).not.toBeInTheDocument();
    });
  });

  it("reveals the endpoint URL field only when the Custom provider is selected", async () => {
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "get_web_search_config") return Promise.resolve(null);
      return Promise.resolve(null);
    });
    render(<WebSearchSection />);

    // Wait for the mount read to settle, then confirm the endpoint field is
    // hidden for the default (Tavily) provider.
    await screen.findByLabelText("Web search provider");
    expect(screen.queryByLabelText("Web search endpoint URL")).not.toBeInTheDocument();

    // Selecting Custom reveals the endpoint URL input (stable aria-label).
    fireEvent.change(screen.getByLabelText("Web search provider"), {
      target: { value: "custom" },
    });
    expect(screen.getByLabelText("Web search endpoint URL")).toBeInTheDocument();
  });

  it("saves a custom provider with its endpoint URL as baseUrl", async () => {
    invoke.mockImplementation((cmd: string, args?: Record<string, unknown>) => {
      if (cmd === "get_web_search_config") return Promise.resolve(null);
      if (cmd === "set_web_search_provider") {
        return Promise.resolve(
          view(
            (args?.kind as WebSearchConfigView["kind"]) ?? "custom",
            5,
            (args?.baseUrl as string | null) ?? null,
          ),
        );
      }
      return Promise.resolve(null);
    });
    render(<WebSearchSection />);

    // Select Custom, enter the endpoint + key, and save.
    fireEvent.change(await screen.findByLabelText("Web search provider"), {
      target: { value: "custom" },
    });
    fireEvent.change(screen.getByLabelText("Web search endpoint URL"), {
      target: { value: "https://search.example.com" },
    });
    fireEvent.change(screen.getByLabelText("Web search API key"), {
      target: { value: "custom-secret-key" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("set_web_search_provider", {
        kind: "custom",
        apiKey: "custom-secret-key",
        maxResults: 5,
        baseUrl: "https://search.example.com",
      });
    });
    // The configured line shows the custom endpoint, and the key never renders.
    expect(await screen.findByTestId("web-search-configured-custom")).toBeInTheDocument();
    expect(screen.getByTestId("web-search-configured-endpoint")).toHaveTextContent(
      "https://search.example.com",
    );
    expect(screen.queryByText("custom-secret-key")).not.toBeInTheDocument();
  });

  it("blocks saving a custom provider with an empty endpoint URL", async () => {
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "get_web_search_config") return Promise.resolve(null);
      return Promise.resolve(null);
    });
    render(<WebSearchSection />);

    fireEvent.change(await screen.findByLabelText("Web search provider"), {
      target: { value: "custom" },
    });
    // Provide a key but leave the endpoint empty.
    fireEvent.change(screen.getByLabelText("Web search API key"), {
      target: { value: "custom-secret-key" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    // A client-side validation error is shown and no save is attempted.
    expect(await screen.findByRole("alert")).toHaveTextContent("A search endpoint URL is required");
    expect(invoke).not.toHaveBeenCalledWith("set_web_search_provider", expect.anything());
  });

  it("rehydrates a configured custom endpoint on mount", async () => {
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "get_web_search_config") {
        return Promise.resolve(view("custom", 3, "https://my-endpoint.example"));
      }
      return Promise.resolve(null);
    });
    render(<WebSearchSection />);

    // The configured line + prefilled endpoint field both show the saved URL.
    expect(await screen.findByTestId("web-search-configured-custom")).toBeInTheDocument();
    expect(screen.getByTestId("web-search-configured-endpoint")).toHaveTextContent(
      "https://my-endpoint.example",
    );
    expect(screen.getByLabelText("Web search endpoint URL")).toHaveValue(
      "https://my-endpoint.example",
    );
  });
});

import { NetworkSharingSection } from "./NetworkSharingSection";
import type { ModelSharingView, NetworkPeerView } from "../../types";

// FEAT-006: the Network Sharing section manages LAN peers (consume), discovery,
// and the share toggle. Every mounting test mocks `list_network_peers` +
// `get_model_sharing` (read on mount) with well-formed shapes. Every input has
// an explicit aria-label (help-text-wrapping bug guard).
describe("NetworkSharingSection", () => {
  beforeEach(() => {
    invoke.mockReset();
    window.localStorage.clear();
  });

  const sharingOff: ModelSharingView = {
    enabled: false,
    port: 11435,
    status: "Sharing is off. Your local models are not exposed to the network.",
  };

  const peer = (
    id: string,
    label: string,
    baseUrl: string,
    hasApiKey = false,
  ): NetworkPeerView => ({
    id,
    label,
    baseUrl,
    hasApiKey,
    warning: null,
  });

  it("rehydrates configured peers and the sharing status on mount", async () => {
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "list_network_peers") {
        return Promise.resolve([
          peer("network-peer-abc", "Studio box", "http://192.168.1.50:11435/v1"),
        ]);
      }
      if (cmd === "get_model_sharing") return Promise.resolve(sharingOff);
      return Promise.resolve([]);
    });
    render(<NetworkSharingSection />);

    expect(await screen.findByTestId("network-peer-network-peer-abc")).toHaveTextContent(
      "http://192.168.1.50:11435/v1",
    );
    expect(await screen.findByTestId("model-sharing-status")).toHaveTextContent(/off/i);
    expect(invoke).toHaveBeenCalledWith("list_network_peers");
    expect(invoke).toHaveBeenCalledWith("get_model_sharing");
  });

  it("adds a peer via add_network_peer and shows the base-url warning, then lists it", async () => {
    let listed: NetworkPeerView[] = [];
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "list_network_peers") return Promise.resolve(listed);
      if (cmd === "get_model_sharing") return Promise.resolve(sharingOff);
      if (cmd === "add_network_peer") {
        const added = peer("network-peer-xyz", "LAN box", "http://192.168.1.9:11435/v1");
        listed = [{ ...added }];
        return Promise.resolve({
          ...added,
          warning:
            "base_url is a non-loopback endpoint served over plaintext HTTP; prefer https://",
        });
      }
      return Promise.resolve([]);
    });
    render(<NetworkSharingSection />);

    // Start from the empty notice.
    expect(await screen.findByTestId("network-peer-empty")).toBeInTheDocument();

    fireEvent.change(screen.getByLabelText("Peer base URL"), {
      target: { value: "http://192.168.1.9:11435/v1" },
    });
    fireEvent.change(screen.getByLabelText("Peer label"), { target: { value: "LAN box" } });
    fireEvent.click(screen.getByRole("button", { name: "Add peer" }));

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("add_network_peer", {
        baseUrl: "http://192.168.1.9:11435/v1",
        label: "LAN box",
        apiKey: null,
      });
    });
    // The non-loopback plaintext advisory is surfaced (role=status, non-fatal).
    expect(await screen.findByTestId("network-peer-warning")).toHaveTextContent(/plaintext/i);
    // The peer now appears in the list (refreshed via list_network_peers).
    expect(await screen.findByTestId("network-peer-network-peer-xyz")).toHaveTextContent("LAN box");
  });

  it("removes a peer via remove_network_peer", async () => {
    let listed: NetworkPeerView[] = [
      peer("network-peer-abc", "Studio box", "http://10.0.0.5:11435/v1"),
    ];
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "list_network_peers") return Promise.resolve(listed);
      if (cmd === "get_model_sharing") return Promise.resolve(sharingOff);
      if (cmd === "remove_network_peer") {
        listed = [];
        return Promise.resolve(undefined);
      }
      return Promise.resolve([]);
    });
    render(<NetworkSharingSection />);

    const removeBtn = await screen.findByRole("button", { name: "Remove peer Studio box" });
    fireEvent.click(removeBtn);

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("remove_network_peer", { id: "network-peer-abc" });
    });
    expect(await screen.findByTestId("network-peer-empty")).toBeInTheDocument();
  });

  it("discovers peers and shows them with a one-click Add", async () => {
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "list_network_peers") return Promise.resolve([]);
      if (cmd === "get_model_sharing") return Promise.resolve(sharingOff);
      if (cmd === "discover_network_peers") {
        return Promise.resolve([{ label: "Found box", baseUrl: "http://192.168.1.20:11435/v1" }]);
      }
      if (cmd === "add_network_peer") {
        return Promise.resolve(
          peer("network-peer-found", "Found box", "http://192.168.1.20:11435/v1"),
        );
      }
      return Promise.resolve([]);
    });
    render(<NetworkSharingSection />);

    fireEvent.click(screen.getByRole("button", { name: "Discover peers on the local network" }));

    const found = await screen.findByTestId("discovered-peer-http://192.168.1.20:11435/v1");
    expect(found).toHaveTextContent("Found box");

    fireEvent.click(screen.getByRole("button", { name: "Add discovered peer Found box" }));
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("add_network_peer", {
        baseUrl: "http://192.168.1.20:11435/v1",
        label: "Found box",
        apiKey: null,
      });
    });
  });

  it("shows a non-fatal 'no peers found' notice when discovery returns empty", async () => {
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "list_network_peers") return Promise.resolve([]);
      if (cmd === "get_model_sharing") return Promise.resolve(sharingOff);
      if (cmd === "discover_network_peers") return Promise.resolve([]);
      return Promise.resolve([]);
    });
    render(<NetworkSharingSection />);

    fireEvent.click(screen.getByRole("button", { name: "Discover peers on the local network" }));
    expect(await screen.findByTestId("discovered-peer-empty")).toHaveTextContent(/no peers found/i);
  });

  it("toggles sharing on via set_model_sharing and reflects the status", async () => {
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "list_network_peers") return Promise.resolve([]);
      if (cmd === "get_model_sharing") return Promise.resolve(sharingOff);
      if (cmd === "set_model_sharing") {
        return Promise.resolve({
          enabled: true,
          port: 11435,
          status: "Sharing your local models on the network (port 11435).",
        });
      }
      return Promise.resolve([]);
    });
    render(<NetworkSharingSection />);

    // Off on mount.
    expect(await screen.findByTestId("model-sharing-status")).toHaveTextContent(/off/i);

    fireEvent.click(screen.getByLabelText("Share my local models on the network"));
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("set_model_sharing", { enabled: true, port: 11435 });
    });
    expect(await screen.findByTestId("model-sharing-status")).toHaveTextContent(
      /sharing your local models/i,
    );
  });
});
