// About / Updates settings section (Phase 8b auto-updater).
//
// Shows the running app version (read at runtime via `@tauri-apps/api/app`
// getVersion(), mirroring the shell StatusBar in app.tsx) and hosts the in-app
// "Check for updates" control. The control drives a small state machine over
// the updater bridge (ipc/updater.ts):
//
//   idle -> checking -> up-to-date            (no newer signed release)
//                    -> update-available      (newer release; show notes +
//                                              "Install and restart")
//                    -> error                 (check failed)
//   update-available -> downloading -> error  (download/install failed)
//                                   -> (relaunch replaces the process)
//
// All plugin/getVersion promises are awaited inside async handlers that catch
// into the error state, so nothing floats. Status text and buttons carry stable
// accessible names / data-testids for the vitest coverage.

import { getVersion } from "@tauri-apps/api/app";
import { useEffect, useState } from "react";
import type { AvailableUpdate } from "../../ipc/updater";
import { checkForUpdate } from "../../ipc/updater";

/** The update-check state machine. */
type UpdateState =
  | { status: "idle" }
  | { status: "checking" }
  | { status: "up-to-date" }
  | { status: "update-available"; update: AvailableUpdate }
  | { status: "downloading"; update: AvailableUpdate }
  | { status: "error"; message: string };

/** Human-readable message for a caught error value. */
function errorMessage(error: unknown): string {
  if (error instanceof Error) {
    return error.message;
  }
  return typeof error === "string" ? error : "Unknown error";
}

export function AboutUpdatesSection() {
  const [version, setVersion] = useState<string | null>(null);
  const [state, setState] = useState<UpdateState>({ status: "idle" });

  // Read the running app version once, mirroring the StatusBar pattern:
  // "loading…" until it resolves, a distinct "unknown" fallback on reject.
  useEffect(() => {
    let active = true;
    getVersion()
      .then((v) => {
        if (active) setVersion(v);
      })
      .catch(() => {
        if (active) setVersion("unknown");
      });
    return () => {
      active = false;
    };
  }, []);

  const onCheck = () => {
    setState({ status: "checking" });
    void (async () => {
      try {
        const update = await checkForUpdate();
        setState(
          update === null ? { status: "up-to-date" } : { status: "update-available", update },
        );
      } catch (error) {
        setState({ status: "error", message: errorMessage(error) });
      }
    })();
  };

  const onInstall = (update: AvailableUpdate) => {
    setState({ status: "downloading", update });
    void (async () => {
      try {
        await update.downloadAndInstall();
        // On success the process is relaunched into the new version, so there
        // is no further state transition to render here.
      } catch (error) {
        setState({ status: "error", message: errorMessage(error) });
      }
    })();
  };

  const busy = state.status === "checking" || state.status === "downloading";

  return (
    <section className="settings__panel" role="region" aria-label="About / Updates">
      <h3 className="settings__section-title">About / Updates</h3>
      <p className="settings__section-desc">
        Agent Harbor version <span data-testid="about-app-version">{version ?? "loading…"}</span>.
      </p>

      <div className="settings-form">
        <button type="button" onClick={onCheck} disabled={busy}>
          Check for updates
        </button>

        <p className="settings__section-desc" data-testid="update-status">
          {state.status === "idle" && "Check for available updates."}
          {state.status === "checking" && "Checking for updates…"}
          {state.status === "up-to-date" && "You are on the latest version."}
          {state.status === "update-available" &&
            `Update available: version ${state.update.version}.`}
          {state.status === "downloading" &&
            `Downloading and installing version ${state.update.version}…`}
          {state.status === "error" && `Update check failed: ${state.message}`}
        </p>

        {(state.status === "update-available" || state.status === "downloading") &&
          state.update.notes !== null && (
            <p className="settings__section-desc" data-testid="update-notes">
              {state.update.notes}
            </p>
          )}

        {state.status === "update-available" && (
          <button type="button" onClick={() => onInstall(state.update)}>
            Install and restart
          </button>
        )}
      </div>
    </section>
  );
}
