// Root application shell (architecture.md Section 8).
//
// Assembles all five UI surfaces into a navigable layout and establishes the
// SINGLE top-level core-event subscription. A left sidebar hosts the History
// surface (Section 8.5); the main pane hosts the Chat surface (Section 8.1)
// with the Model selector's routing controls next to the Composer (Section 8.2)
// and switchable panels for the Tool manager (Section 8.3) and Agent editor
// (Section 8.4). The Phase 0 `app_version` display is folded into a small
// status/about footer.
//
// EVENT FAN-OUT (Section 7.3): rather than each surface subscribing to core
// events independently, the shell opens ONE `onCoreEvent` subscription in a
// `useEffect` and dispatches every CoreEvent to each store's `applyCoreEvent`.
// The effect returns a stable cleanup that awaits and calls the `UnlistenFn`, so
// the subscription is torn down exactly once on unmount.

import { useEffect, useState } from "react";
import { Composer } from "./features/chat/Composer";
import { MessageList } from "./features/chat/MessageList";
import { PermissionPrompt } from "./features/chat/PermissionPrompt";
import { RoutingModeToggle } from "./features/model-selector/RoutingModeToggle";
import { AgentEditor } from "./features/agent-editor/AgentEditor";
import { History } from "./features/history/History";
import { ToolManager } from "./features/tool-manager/ToolManager";
import { appVersion } from "./ipc/commands";
import { onCoreEvent } from "./ipc/events";
import { useConversationsStore } from "./state/conversations";
import { usePersonasStore } from "./state/personas";
import { useProvidersStore } from "./state/providers";
import { useToolsStore } from "./state/tools";

/** The switchable panels in the main pane's secondary area. */
type Panel = "chat" | "tools" | "agents";

/** Fold the app version into a small status/about footer (Phase 0 wiring). */
function StatusBar() {
  const [version, setVersion] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    appVersion()
      .then((v) => {
        if (active) setVersion(v);
      })
      .catch(() => {
        if (active) setVersion(null);
      });
    return () => {
      active = false;
    };
  }, []);

  return (
    <footer className="app__status">
      App version: <span data-testid="app-version">{version ?? "loading…"}</span>
    </footer>
  );
}

export function App() {
  const [panel, setPanel] = useState<Panel>("chat");

  // Single top-level core-event subscription: fan every CoreEvent out to each
  // store's reducer so the surfaces stay in sync without subscribing
  // independently (architecture.md Section 7.3).
  useEffect(() => {
    const applyConversations = useConversationsStore.getState().applyCoreEvent;
    const applyProviders = useProvidersStore.getState().applyCoreEvent;
    const applyTools = useToolsStore.getState().applyCoreEvent;
    const applyPersonas = usePersonasStore.getState().applyCoreEvent;

    const unlistenPromise = onCoreEvent((event) => {
      applyConversations(event.payload);
      applyProviders(event.payload);
      applyTools(event.payload);
      applyPersonas(event.payload);
    });

    return () => {
      void unlistenPromise.then((unlisten) => unlisten());
    };
  }, []);

  return (
    <div className="app">
      <aside className="app__sidebar">
        <h1 className="app__brand">Agent Harbor</h1>
        <History />
      </aside>

      <main className="app__main">
        <nav className="app__tabs" role="tablist" aria-label="Surfaces">
          <button
            type="button"
            role="tab"
            aria-selected={panel === "chat"}
            data-active={panel === "chat"}
            onClick={() => setPanel("chat")}
          >
            Chat
          </button>
          <button
            type="button"
            role="tab"
            aria-selected={panel === "tools"}
            data-active={panel === "tools"}
            onClick={() => setPanel("tools")}
          >
            Tools
          </button>
          <button
            type="button"
            role="tab"
            aria-selected={panel === "agents"}
            data-active={panel === "agents"}
            onClick={() => setPanel("agents")}
          >
            Agents
          </button>
        </nav>

        {panel === "chat" && (
          <section className="app__chat" aria-label="Chat">
            <MessageList />
            <RoutingModeToggle />
            <Composer />
            <PermissionPrompt />
          </section>
        )}
        {panel === "tools" && <ToolManager />}
        {panel === "agents" && <AgentEditor />}
      </main>

      <StatusBar />
    </div>
  );
}
