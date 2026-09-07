// PermissionModeControl (architecture.md Section 8.3 / 5.6 tool manager).
//
// Sets an MCP server's tool-invocation permission mode: Ask, Allow, or Deny.
// FEAT-001 scoped `set_tool_permission` to the per-SERVER `permission_mode`
// (the Phase 5 schema has no per-tool override column), so this control is
// per-server. Selecting a mode delegates to the tools store's `setPermission`,
// which forwards to the command and reflects the returned config.

import { useToolsStore } from "../../state/tools";
import type { PermissionMode } from "../../types";

const MODES: { value: PermissionMode; label: string }[] = [
  { value: "ask", label: "Ask" },
  { value: "allow", label: "Allow" },
  { value: "deny", label: "Deny" },
];

export interface PermissionModeControlProps {
  /** The server whose permission mode is being edited. */
  serverId: string;
  /** The server's current permission mode. */
  mode: PermissionMode;
}

export function PermissionModeControl({ serverId, mode }: PermissionModeControlProps) {
  const setPermission = useToolsStore((s) => s.setPermission);

  return (
    <div className="permission-mode" role="radiogroup" aria-label="Permission mode">
      {MODES.map((option) => (
        <button
          key={option.value}
          type="button"
          role="radio"
          aria-checked={mode === option.value}
          data-active={mode === option.value}
          onClick={() => void setPermission(serverId, option.value)}
        >
          {option.label}
        </button>
      ))}
    </div>
  );
}
