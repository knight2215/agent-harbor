// UI surface 3: Tool / MCP server manager (architecture.md Section 8.3).
//
// Lists configured MCP servers with live status, adds/edits them via a
// transport-aware ServerForm (stdio or httpSse), inspects discovered tools,
// sets per-server permission modes, and shows connection health. All mutations
// go through the FEAT-001 MCP commands via the tools store.

export { ConnectionHealth } from "./ConnectionHealth";
export type { ConnectionHealthProps } from "./ConnectionHealth";
export { PermissionModeControl } from "./PermissionModeControl";
export type { PermissionModeControlProps } from "./PermissionModeControl";
export { ServerForm } from "./ServerForm";
export type { ServerFormProps } from "./ServerForm";
export { ServerList } from "./ServerList";
export type { ServerListProps } from "./ServerList";
export { ToolInspector } from "./ToolInspector";
export type { ToolInspectorProps } from "./ToolInspector";
export { ToolManager } from "./ToolManager";
