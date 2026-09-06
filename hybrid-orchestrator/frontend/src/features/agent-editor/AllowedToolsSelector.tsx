// AllowedToolsSelector (architecture.md Section 8.4 agent editor).
//
// Chooses which configured MCP servers a persona may use. The options come from
// the tools store (the same servers the tool manager owns); the value is a list
// of server UUIDs (`persona.allowedToolServers`), which the Rust side validates
// as UUIDs. Presentational: takes the selected ids + onChange through props.

import { useToolsStore } from "../../state/tools";

export interface AllowedToolsSelectorProps {
  /** The currently selected server ids. */
  value: string[];
  /** Called with the next selected server ids when a server is toggled. */
  onChange: (serverIds: string[]) => void;
}

export function AllowedToolsSelector({ value, onChange }: AllowedToolsSelectorProps) {
  const servers = useToolsStore((s) => s.servers);

  const toggle = (id: string, checked: boolean) => {
    if (checked) {
      if (!value.includes(id)) onChange([...value, id]);
    } else {
      onChange(value.filter((v) => v !== id));
    }
  };

  if (servers.length === 0) {
    return <p className="allowed-tools__empty">No MCP servers configured.</p>;
  }

  return (
    <fieldset className="allowed-tools" aria-label="Allowed tool servers">
      <legend>Allowed tool servers</legend>
      {servers.map((server) => (
        <label key={server.id} className="allowed-tools__item">
          <input
            type="checkbox"
            aria-label={server.name}
            checked={value.includes(server.id)}
            onChange={(e) => toggle(server.id, e.target.checked)}
          />
          {server.name}
        </label>
      ))}
    </fieldset>
  );
}
