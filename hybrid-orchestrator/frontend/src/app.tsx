// Root application shell (architecture.md Section 8; Phase 7 UX refactor).
//
// A navigable shell with a collapsible left sidebar hosting the brand/logo and
// the primary destinations Chat / History / Settings. The main pane renders the
// active destination:
//   - Chat (default): a Welcome screen until a conversation is active, then the
//     chat surface (MessageList + Composer + PermissionPrompt). The model
//     selector's routing-mode + model control, the "Why this model?" info icon,
//     and the enumeration-error warning icon are folded INTO the Composer's
//     compact control row (Section 8.1/8.2). The Chat nav item nests the
//     Conversations sub-list (conversation list + New conversation).
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
// unmount. The SAME effect also triggers the providers store's `load()` once at
// startup (Section 8.2) so the chat model picker is populated on launch without
// the user first opening Settings; a `providersChanged` CoreEvent (emitted by
// the provider-config mutation commands) then refetches it on demand. The
// status footer surfaces the real app version, read at runtime via
// `@tauri-apps/api/app` getVersion() (which reads tauri.conf.json), so it stays
// in sync with the shipped build without any hardcoding.

import { getVersion } from "@tauri-apps/api/app";
import { useEffect, useState } from "react";
import logoUrl from "./assets/logo.svg";
import { Composer } from "./features/chat/Composer";
import { MessageList } from "./features/chat/MessageList";
import { PermissionPrompt } from "./features/chat/PermissionPrompt";
import { WelcomeScreen } from "./features/chat/WelcomeScreen";
import { ConversationList } from "./features/history/ConversationList";
import { History } from "./features/history/History";
import { NewConversationButton } from "./features/history/NewConversationButton";
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
  // The chat surface renders ONLY when a conversation is active; otherwise the
  // Chat destination shows the Welcome screen so a fresh launch never looks like
  // an already-open (but empty) conversation.
  const activeConversationId = useConversationsStore((s) => s.activeConversationId);

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

    // Populate the model-selector store once at startup so the chat picker
    // reflects configured providers/models WITHOUT the user first opening
    // Settings (bug A). This mirrors how AgentEditor/ToolManager/History load
    // their own stores on mount; here it lives in the single root effect so it
    // runs exactly once, alongside (not instead of) the subscription below. The
    // load is fire-and-forget (its own errors surface via the store's `errors`
    // and the ProviderEnumerationErrors near the picker).
    void useProvidersStore.getState().load();

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
            <div key={id} className="app__nav-group">
              <button
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
              {/*
                Conversations nested UNDER the Chat nav item: the Chat group's
                expanded body hosts the reusable conversation list + a New
                conversation action, so selecting a row resumes that session and
                creating one opens it (both flip the chat surface on). This
                replaces the old always-on <History /> block under Chat. The
                sub-list is hidden in the collapsed rail (CSS) like the labels.
              */}
              {id === "chat" && (
                <div className="app__nav-sub" data-testid="chat-conversations">
                  <span className="app__nav-sub-label">Conversations</span>
                  <NewConversationButton />
                  <ConversationList query="" />
                </div>
              )}
            </div>
          ))}
        </nav>
      </aside>

      <main className="app__main">
        {destination === "chat" &&
          (activeConversationId !== null ? (
            <section className="app__chat" aria-label="Chat">
              <MessageList />
              <Composer />
              <PermissionPrompt />
            </section>
          ) : (
            <WelcomeScreen />
          ))}
        {destination === "history" && <History />}
        {destination === "settings" && <Settings />}
      </main>

      <StatusBar />
    </div>
  );
}
