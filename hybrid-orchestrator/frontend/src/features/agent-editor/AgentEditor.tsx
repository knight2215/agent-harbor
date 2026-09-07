// AgentEditor (architecture.md Section 8.4).
//
// The agent config editor surface: a PersonaList on the side and a PersonaEditor
// for the selected persona (or a blank one when nothing is selected). Loads the
// personas and the MCP servers (for AllowedToolsSelector) on mount. The list
// refreshes on `personasChanged` through the personas store; MCP state flows
// through the tools store from the app shell's single `onCoreEvent` fan-out.

import { useEffect } from "react";
import { usePersonasStore } from "../../state/personas";
import { useToolsStore } from "../../state/tools";
import { PersonaEditor } from "./PersonaEditor";
import { PersonaList } from "./PersonaList";

export function AgentEditor() {
  const loadPersonas = usePersonasStore((s) => s.load);
  const loadServers = useToolsStore((s) => s.load);
  const personas = usePersonasStore((s) => s.personas);
  const selectedId = usePersonasStore((s) => s.selectedId);
  const select = usePersonasStore((s) => s.select);

  useEffect(() => {
    void loadPersonas();
    void loadServers();
  }, [loadPersonas, loadServers]);

  const selected = personas.find((p) => p.id === selectedId) ?? null;

  return (
    <section className="agent-editor" aria-label="Agent editor">
      <PersonaList />
      <PersonaEditor
        key={selectedId ?? "new"}
        persona={selected}
        onSaved={(persona) => select(persona.id)}
      />
    </section>
  );
}
