// Root application shell (architecture.md Section 8; Phase 7 UX refactor).
//
// A navigable shell with a collapsible left sidebar hosting the brand/logo and
// the primary destinations Chat / History / Settings. The main pane renders the
// active destination:
//   - Chat (default): the chat surface (MessageList + Composer + PermissionPrompt)
//     with the model selector's per-conversation routing-mode + model control
//     next to the Composer (RouteBadge surfaces the chosen provider/model +
//     rationale, Section 8.2).
//   - History: the conversation history / session-management surface (Section 8.5).
//   - Settings: the heavy-configuration area (providers & keys, local runtimes,
//     MCP / tools, agents, routing, appearance) with its own sub-navigation.
//
// EVENT FAN-OUT (Section 7.3): the shell opens ONE `onCoreEvent` subscription in
// a `useEffect` at the shell ROOT and dispatches every CoreEvent to each store's
// `applyCoreEvent`. This effect has EMPTY deps and reads each store's reducer
// via `getState()`, so switching destinations NEVER tears it down or opens a
// second subscription. The effect returns a stable cleanup that awaits and
// calls the `UnlistenFn`, so the subscription is torn down exactly once on
// unmount. The status footer surfaces the real app version, read at runtime via
// `@tauri-apps/api/app` getVersion() (which reads tauri.conf.json), so it stays
// in sync with the shipped build without any hardcoding.

import { getVersion } from "@tauri-apps/api/app";
import { useEffect, useState } from "react";
import logoUrl from "./assets/logo.svg";
import { Composer } from "./features/chat/Composer";
import { MessageList } from "./features/chat/MessageList";
import { PermissionPrompt } from "./features/chat/PermissionPrompt";
import { RoutingModeToggle } from "./features/model-selector/RoutingModeToggle";
import { History } from "./features/history/History";
import { Settings } from "./features/settings/Settings";
import { onCoreEvent } from "./ipc/events";
import { useConversationsStore } from "./state/conversations";
import { usePersonasStore } from "./state/personas";
import { useProvidersStore } from "./state/providers";
import { useToolsStore } from "./state/tools";

/** The primary sidebar destinations in the shell. */
type Destination = "chat" | "history" | "settings";

/** localStorage key persisting the sidebar collapse state. */
const SIDEBAR_COLLAPSED_KEY = "ah-sidebar-collapsed";

/** Read the persisted sidebar collapse state (guarded for constrained envs). */
function readCollapsed(): boolean {
  try {
    return window.localStorage.getItem(SIDEBAR_COLLAPSED_KEY) === "true";
  } catch {
    return false;
  }
}

const NAV_ITEMS: ReadonlyArray<{ id: Destination; label: string; icon: string }> = [
  { id: "chat", label: "Chat", icon: "💬" },
  { id: "history", label: "History", icon: "🕘" },
  { id: "settings", label: "Settings", icon: "⚙" },
];

/**
 * Fold the real app version into a small status/about footer. The version is
 * read at runtime from `@tauri-apps/api/app` getVersion() (backed by
 * tauri.conf.json), so it tracks the shipped build automatically on every
 * version bump. Renders "loading…" until the async read resolves, and a
 * distinct "unknown" fallback if the read rejects (so a failed read is not
 * indistinguishable from a still-pending one).
 */
function StatusBar() {
  const [version, setVersion] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    getVersion()
      .then((v) => {
        if (active) setVersion(v);
      })
      .catch(() => {
        // A failed read should be distinguishable from a pending one: render a
        // concrete fallback rather than leaving the "loading…" placeholder up.
        if (active) setVersion("unknown");
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
  const [destination, setDestination] = useState<Destination>("chat");
  const [collapsed, setCollapsed] = useState<boolean>(readCollapsed);

  // Single top-level core-event subscription: fan every CoreEvent out to each
  // store's reducer so the surfaces stay in sync without subscribing
  // independently (architecture.md Section 7.3). EMPTY deps + getState() keep
  // this stable across destination switches: it is opened once and torn down
  // once (on unmount), never re-run when the active view changes.
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

  const toggleCollapsed = () => {
    setCollapsed((prev) => {
      const next = !prev;
      try {
        window.localStorage.setItem(SIDEBAR_COLLAPSED_KEY, String(next));
      } catch {
        // Ignore storage write errors (private mode / constrained env).
      }
      return next;
    });
  };

  return (
    <div className="app" data-collapsed={collapsed}>
      <aside className="app__sidebar">
        <h1 className="app__brand">
          <img className="app__brand-logo" src={logoUrl} alt="" width={28} height={28} />
          <span className="app__brand-text">Agent Harbor</span>
        </h1>

        <button
          type="button"
          className="app__collapse"
          aria-label={collapsed ? "Expand sidebar" : "Collapse sidebar"}
          aria-pressed={collapsed}
          onClick={toggleCollapsed}
        >
          {collapsed ? "»" : "«"}
        </button>

        <nav className="app__nav" role="navigation" aria-label="Primary">
          {NAV_ITEMS.map(({ id, label, icon }) => (
            <button
              key={id}
              type="button"
              className="app__nav-item"
              data-active={destination === id}
              aria-current={destination === id ? "page" : undefined}
              title={label}
              onClick={() => setDestination(id)}
            >
              <span className="app__nav-icon" aria-hidden="true">
                {icon}
              </span>
              <span className="app__nav-label">{label}</span>
            </button>
          ))}
        </nav>

        {destination === "chat" && (
          <div className="app__sidebar-history">
            <History />
          </div>
        )}
      </aside>

      <main className="app__main">
        {destination === "chat" && (
          <section className="app__chat" aria-label="Chat">
            <MessageList />
            <RoutingModeToggle />
            <Composer />
            <PermissionPrompt />
          </section>
        )}
        {destination === "history" && <History />}
        {destination === "settings" && <Settings />}
      </main>

      <StatusBar />
    </div>
  );
}
