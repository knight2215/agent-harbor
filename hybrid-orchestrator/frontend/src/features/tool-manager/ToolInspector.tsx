// ToolInspector (architecture.md Section 8.3 tool manager).
//
// Shows the tools a server has discovered, with each tool's name, description,
// and JSON input schema. Tools come from the tools store's per-server `tools`
// cache (populated by `refreshTools` -> `refresh_mcp_tools`); a Refresh button
// re-lists them. The surface never touches transport internals directly.

import { useToolsStore } from "../../state/tools";

export interface ToolInspectorProps {
  /** The server whose discovered tools are inspected. */
  serverId: string;
}

export function ToolInspector({ serverId }: ToolInspectorProps) {
  const tools = useToolsStore((s) => s.tools[serverId]) ?? [];
  const refreshTools = useToolsStore((s) => s.refreshTools);

  return (
    <div className="tool-inspector" aria-label="Discovered tools">
      <div className="tool-inspector__header">
        <span className="tool-inspector__title">Tools</span>
        <button type="button" onClick={() => void refreshTools(serverId)}>
          Refresh
        </button>
      </div>
      {tools.length === 0 ? (
        <p className="tool-inspector__empty">No tools discovered.</p>
      ) : (
        <ul className="tool-inspector__list">
          {tools.map((tool) => (
            <li key={tool.name} className="tool-inspector__item">
              <span className="tool-inspector__name">{tool.name}</span>
              {tool.description.length > 0 && (
                <span className="tool-inspector__description">{tool.description}</span>
              )}
              <pre className="tool-inspector__schema">
                {JSON.stringify(tool.inputSchema, null, 2)}
              </pre>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
