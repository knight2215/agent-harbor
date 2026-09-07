// ToolManager (architecture.md Section 8.3).
//
// The tool/MCP server manager surface: loads the configured servers on mount,
// lists them (ServerList) with live status, and hosts a ServerForm for
// creating a new server or editing an existing one. All mutations go through
// the FEAT-001 MCP commands via the tools store; `mcpStateChanged` / `mcpError`
// events flow into the store from the single top-level `onCoreEvent` fan-out
// wired by the app shell (FEAT-004).

import { useEffect, useState } from "react";
import { useToolsStore } from "../../state/tools";
import type { McpServerConfig } from "../../types";
import { ServerForm } from "./ServerForm";
import { ServerList } from "./ServerList";

export function ToolManager() {
  const load = useToolsStore((s) => s.load);
  const [editing, setEditing] = useState<McpServerConfig | null>(null);
  const [showForm, setShowForm] = useState(false);

  useEffect(() => {
    void load();
  }, [load]);

  const startCreate = () => {
    setEditing(null);
    setShowForm(true);
  };

  const startEdit = (server: McpServerConfig) => {
    setEditing(server);
    setShowForm(true);
  };

  const closeForm = () => {
    setEditing(null);
    setShowForm(false);
  };

  return (
    <section className="tool-manager" aria-label="Tool manager">
      <header className="tool-manager__header">
        <h2>MCP servers</h2>
        <button type="button" onClick={startCreate}>
          Add server
        </button>
      </header>

      <ServerList onEdit={startEdit} />

      {showForm && <ServerForm server={editing} onSaved={closeForm} onCancel={closeForm} />}
    </section>
  );
}
