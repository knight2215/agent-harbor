import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";

// Mock the Tauri updater flow so the section renders without a live runtime and
// the tests never hang: `check()` (plugin-updater) and `relaunch()`
// (plugin-process) are spies, and getVersion (@tauri-apps/api/app) resolves a
// deterministic version. Individual tests override `check`'s resolved value to
// drive the no-update / update-available paths.
const check = vi.fn();
vi.mock("@tauri-apps/plugin-updater", () => ({
  check: () => check(),
}));

const relaunch = vi.fn();
vi.mock("@tauri-apps/plugin-process", () => ({
  relaunch: () => relaunch(),
}));

const getVersion = vi.fn();
vi.mock("@tauri-apps/api/app", () => ({
  getVersion: () => getVersion(),
}));

import { AboutUpdatesSection } from "./AboutUpdatesSection";

describe("AboutUpdatesSection", () => {
  beforeEach(() => {
    check.mockReset();
    relaunch.mockReset();
    getVersion.mockReset();
    getVersion.mockResolvedValue("0.4.0");
    relaunch.mockResolvedValue(undefined);
  });

  it("renders the current app version read via getVersion()", async () => {
    render(<AboutUpdatesSection />);

    await waitFor(() => {
      expect(screen.getByTestId("about-app-version")).toHaveTextContent("0.4.0");
    });
    expect(getVersion).toHaveBeenCalled();
  });

  it("reports up-to-date when check() resolves null", async () => {
    check.mockResolvedValue(null);
    render(<AboutUpdatesSection />);

    fireEvent.click(screen.getByRole("button", { name: "Check for updates" }));

    await waitFor(() => {
      expect(screen.getByTestId("update-status")).toHaveTextContent(
        "You are on the latest version.",
      );
    });
    expect(check).toHaveBeenCalled();
    // No install control appears on the up-to-date path.
    expect(screen.queryByRole("button", { name: "Install and restart" })).toBeNull();
  });

  it("shows version + notes and installs then relaunches on an available update", async () => {
    const downloadAndInstall = vi.fn().mockResolvedValue(undefined);
    check.mockResolvedValue({
      version: "0.5.0",
      body: "Shiny new release notes.",
      downloadAndInstall,
    });
    render(<AboutUpdatesSection />);

    fireEvent.click(screen.getByRole("button", { name: "Check for updates" }));

    // The update-available state surfaces the version and its release notes.
    await waitFor(() => {
      expect(screen.getByTestId("update-status")).toHaveTextContent(
        "Update available: version 0.5.0.",
      );
    });
    expect(screen.getByTestId("update-notes")).toHaveTextContent("Shiny new release notes.");

    // Installing downloads/installs the signed update and then relaunches.
    fireEvent.click(screen.getByRole("button", { name: "Install and restart" }));

    await waitFor(() => {
      expect(downloadAndInstall).toHaveBeenCalled();
    });
    await waitFor(() => {
      expect(relaunch).toHaveBeenCalled();
    });
  });

  it("surfaces an error state when check() rejects", async () => {
    check.mockRejectedValue(new Error("network down"));
    render(<AboutUpdatesSection />);

    fireEvent.click(screen.getByRole("button", { name: "Check for updates" }));

    await waitFor(() => {
      expect(screen.getByTestId("update-status")).toHaveTextContent(
        "Update check failed: network down",
      );
    });
  });

  it("surfaces an error state when downloadAndInstall() rejects", async () => {
    const downloadAndInstall = vi.fn().mockRejectedValue(new Error("disk full"));
    check.mockResolvedValue({
      version: "0.5.0",
      body: "Shiny new release notes.",
      downloadAndInstall,
    });
    render(<AboutUpdatesSection />);

    fireEvent.click(screen.getByRole("button", { name: "Check for updates" }));

    // Wait for the available update before triggering the failing install.
    await waitFor(() => {
      expect(screen.getByTestId("update-status")).toHaveTextContent(
        "Update available: version 0.5.0.",
      );
    });

    fireEvent.click(screen.getByRole("button", { name: "Install and restart" }));

    // The download/install failure surfaces in the error state.
    await waitFor(() => {
      expect(screen.getByTestId("update-status")).toHaveTextContent(
        "Update check failed: disk full",
      );
    });
    expect(downloadAndInstall).toHaveBeenCalled();
    // A failed install must not relaunch the process.
    expect(relaunch).not.toHaveBeenCalled();
  });
});
