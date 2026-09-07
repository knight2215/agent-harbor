// Typed updater bridge (Phase 8b signed self-update).
//
// A thin wrapper over the Tauri v2 updater flow: `@tauri-apps/plugin-updater`
// `check()` queries the GitHub Releases `latest.json` endpoint (configured in
// crates/tauri-app/tauri.conf.json under plugins.updater, with the embedded
// minisign public key), and `@tauri-apps/plugin-process` `relaunch()` restarts
// the app once a signed update has been downloaded and installed.
//
// This module is the ONLY place the frontend touches those plugins, mirroring
// the typed-IPC discipline in ipc/commands.ts. `checkForUpdate()` collapses the
// v2 `Update | null` result into a small, test-friendly shape: `null` when the
// running build is already current, otherwise the available `version`, its
// release `notes`, and a `downloadAndInstall` action that downloads + installs
// the signed update and then relaunches into it. Every plugin promise is
// awaited (no floating promises).

import { relaunch } from "@tauri-apps/plugin-process";
import { check } from "@tauri-apps/plugin-updater";

/**
 * An available update, in the minimal shape the About / Updates UI needs.
 *
 * `downloadAndInstall` runs the signed download+install and then relaunches the
 * app into the new version. It resolves after the relaunch request is issued
 * (in practice the process is replaced, so callers should treat the pending
 * promise as "restarting" rather than expecting further UI updates).
 */
export interface AvailableUpdate {
  /** The version string of the available update (e.g. "0.4.0"). */
  version: string;
  /** Release notes for the update, or `null` when none were published. */
  notes: string | null;
  /** Download + install the signed update, then relaunch into it. */
  downloadAndInstall: () => Promise<void>;
}

/**
 * Check the configured endpoint for a newer signed release.
 *
 * Returns `null` when the running build is already up to date, or an
 * {@link AvailableUpdate} describing the newer release otherwise. Rejections
 * from the underlying plugin (network/signature failures) propagate to the
 * caller, which surfaces them as the UI's error state.
 */
export async function checkForUpdate(): Promise<AvailableUpdate | null> {
  const update = await check();
  if (update === null) {
    return null;
  }

  return {
    version: update.version,
    notes: update.body ?? null,
    downloadAndInstall: async () => {
      await update.downloadAndInstall();
      await relaunch();
    },
  };
}
