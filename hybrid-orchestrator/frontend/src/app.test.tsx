import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import { App } from "./app";

// Mock the Tauri IPC bridge so the test runs without a live backend.
const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
}));

describe("<App />", () => {
  beforeEach(() => {
    invoke.mockReset();
  });

  it("renders the app version returned by the app_version command", async () => {
    invoke.mockResolvedValue("9.9.9-test");

    render(<App />);

    await waitFor(() => {
      expect(screen.getByTestId("app-version")).toHaveTextContent("9.9.9-test");
    });

    expect(invoke).toHaveBeenCalledWith("app_version");
  });
});
