import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";

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
    // The plaintext key is cleared from the field after saving.
    expect(await screen.findByTestId("provider-key-saved")).toBeInTheDocument();
    expect((screen.getByLabelText("API key") as HTMLInputElement).value).toBe("");
  });
});
