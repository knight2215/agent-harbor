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

  it("stores a provider key via set_provider_secret without retaining plaintext", async () => {
    invoke.mockResolvedValue("secret-ref://openai");
    render(<Settings />);

    fireEvent.change(screen.getByLabelText("Provider id"), { target: { value: "openai" } });
    fireEvent.change(screen.getByLabelText("API key"), { target: { value: "sk-test" } });
    fireEvent.click(screen.getByRole("button", { name: "Save key" }));

    expect(invoke).toHaveBeenCalledWith("set_provider_secret", {
      providerId: "openai",
      secret: "sk-test",
    });
    // The "configured" indication shows the provider and its opaque ref.
    expect(await screen.findByTestId("provider-key-configured-openai")).toBeInTheDocument();
    // The plaintext key is cleared from the field after saving.
    expect((screen.getByLabelText("API key") as HTMLInputElement).value).toBe("");
  });

  it("keeps the configured-key indication after navigating away and back", async () => {
    invoke.mockResolvedValue("secret-ref://openai");
    render(<Settings />);

    fireEvent.change(screen.getByLabelText("Provider id"), { target: { value: "openai" } });
    fireEvent.change(screen.getByLabelText("API key"), { target: { value: "sk-test" } });
    fireEvent.click(screen.getByRole("button", { name: "Save key" }));

    expect(await screen.findByTestId("provider-key-configured-openai")).toBeInTheDocument();

    // Simulate navigating to another destination and back: unmount, then mount
    // a fresh Settings tree (which resets in-component state to defaults).
    cleanup();
    render(<Settings />);

    // The indication is rehydrated from the persisted non-secret marker.
    expect(await screen.findByTestId("provider-key-configured-openai")).toBeInTheDocument();
    // The plaintext key is never persisted; only the opaque ref is shown.
    const marker = window.localStorage.getItem("ah-configured-providers");
    expect(marker).not.toBeNull();
    expect(marker).not.toContain("sk-test");
  });
});

import { LocalRuntimesSection } from "./LocalRuntimesSection";
import type { EmbeddedModelStatus, EmbeddedModelView } from "../../types";

function status(loadedModelId: string | null, registeredCount: number): EmbeddedModelStatus {
  return { loadedModelId, registeredCount };
}

function view(id: string, path: string, loaded: boolean): EmbeddedModelView {
  return { id, path, loaded };
}

describe("LocalRuntimesSection embedded engine", () => {
  beforeEach(() => {
    invoke.mockReset();
    window.localStorage.clear();
  });

  it("renders the embedded engine as an available Local runtime (not coming soon)", async () => {
    // The mount-time probes resolve to an empty engine (no imports, nothing
    // selected).
    invoke.mockImplementation((cmd: string) => {
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
});
