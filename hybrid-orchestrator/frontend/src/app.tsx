import { useEffect, useState } from "react";
import { appVersion } from "./ipc/commands";

/**
 * Root application component.
 *
 * Phase 0 proves the end-to-end IPC wiring by calling the `app_version`
 * command exposed by the Tauri shell and rendering the returned version
 * string. Later phases replace this with the real UI surfaces (chat,
 * model-selector, tool-manager, agent-editor, history).
 */
export function App() {
  const [version, setVersion] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    appVersion()
      .then((v) => {
        if (active) {
          setVersion(v);
        }
      })
      .catch(() => {
        if (active) {
          setVersion(null);
        }
      });
    return () => {
      active = false;
    };
  }, []);

  return (
    <main>
      <h1>Agent Harbor</h1>
      <p>
        App version: <span data-testid="app-version">{version ?? "loading…"}</span>
      </p>
    </main>
  );
}
