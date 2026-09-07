// PersonaList (architecture.md Section 8.4 agent editor).
//
// Lists the saved personas from the personas store; selecting one opens it in
// the editor (via the store's `select`), a New button clears the selection to
// create one, and Delete removes a persona (-> `delete_persona`). The list
// refreshes on `personasChanged` through the store.

import { usePersonasStore } from "../../state/personas";

export function PersonaList() {
  const personas = usePersonasStore((s) => s.personas);
  const selectedId = usePersonasStore((s) => s.selectedId);
  const select = usePersonasStore((s) => s.select);
  const remove = usePersonasStore((s) => s.remove);

  return (
    <div className="persona-list" aria-label="Personas">
      <button type="button" className="persona-list__new" onClick={() => select(null)}>
        New persona
      </button>
      {personas.length === 0 ? (
        <p className="persona-list__empty">No personas yet.</p>
      ) : (
        <ul>
          {personas.map((persona) => (
            <li key={persona.id} className="persona-list__item">
              <button
                type="button"
                aria-pressed={persona.id === selectedId}
                data-selected={persona.id === selectedId}
                onClick={() => select(persona.id)}
              >
                {persona.name}
              </button>
              <button
                type="button"
                aria-label={`Delete ${persona.name}`}
                onClick={() => void remove(persona.id)}
              >
                Delete
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
