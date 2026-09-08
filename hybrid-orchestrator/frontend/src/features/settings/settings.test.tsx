import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, cleanup } from "@testing-library/react";

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
    // The empty-state placeholder for future runtimes is present and labeled.
    expect(
      await screen.findByRole("region", { name: "More local runtimes coming soon" }),
    ).toBeInTheDocument();
    expect(screen.getByTestId("local-runtimes-empty-state")).toBeInTheDocument();

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
