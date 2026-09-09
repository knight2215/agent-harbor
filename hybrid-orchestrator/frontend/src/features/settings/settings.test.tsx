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
    render(<Settings />);

    const nav = screen.getByRole("navigation", { name: "Settings sections" });
    expect(nav).toBeInTheDocument();

    // Providers & Keys is the default section.
    expect(screen.getByRole("region", { name: "Providers & Keys" })).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Local Runtimes" }));
    expect(await screen.findByRole("region", { name: "Local Runtimes" })).toBeInTheDocument();
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

    fireEvent.click(screen.getByRole("button", { name: "MCP / Tools" }));
    expect(await screen.findByRole("region", { name: "Tool manager" })).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Agents" }));
    expect(await screen.findByRole("region", { name: "Agent editor" })).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Routing" }));
    expect(await screen.findByRole("region", { name: "Routing" })).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Appearance" }));
    expect(await screen.findByRole("region", { name: "Appearance" })).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "About / Updates" }));
    expect(await screen.findByRole("region", { name: "About / Updates" })).toBeInTheDocument();
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
