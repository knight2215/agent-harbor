// ServerForm (architecture.md Section 8.3 tool manager).
//
// Create/edit form for an MCP server. A transport selector switches between
// `stdio` (command + args[] + env [key, value][]) and `httpSse` (url + headers
// [key, value][]); submit builds the exact `McpServerInput` shape from FEAT-001
// and delegates to the tools store's `addServer` / `updateServer` (which call
// `add_mcp_server` / `update_mcp_server`). The surface never touches transport
// internals directly. Required transport fields are validated client-side to
// match the Rust bounds; a CommandError from the backend is surfaced inline.

import { useState } from "react";
import { useToolsStore } from "../../state/tools";
import type { McpServerConfig, McpServerInput, McpTransport, PermissionMode } from "../../types";

type TransportKind = McpTransport["type"];
type Pair = [string, string];

/** Extract the initial editor fields from an existing server (for edit mode). */
function initialFields(server: McpServerConfig | null) {
  if (server === null) {
    return {
      name: "",
      kind: "stdio" as TransportKind,
      command: "",
      argsText: "",
      env: [] as Pair[],
      url: "",
      headers: [] as Pair[],
      permissionMode: "ask" as PermissionMode,
      enabled: true,
    };
  }
  const t = server.transport;
  return {
    name: server.name,
    kind: t.type,
    command: t.type === "stdio" ? t.command : "",
    argsText: t.type === "stdio" ? t.args.join(" ") : "",
    env: t.type === "stdio" ? t.env.map((p): Pair => [p[0], p[1]]) : [],
    url: t.type === "httpSse" ? t.url : "",
    headers: t.type === "httpSse" ? t.headers.map((p): Pair => [p[0], p[1]]) : [],
    permissionMode: server.permissionMode,
    enabled: server.enabled,
  };
}

export interface ServerFormProps {
  /** The server being edited, or null to create a new one. */
  server?: McpServerConfig | null;
  /** Called after a successful add/update. */
  onSaved?: (server: McpServerConfig) => void;
  /** Called when the user cancels. */
  onCancel?: () => void;
}

/** A repeated key/value editor (used for env and headers). */
function PairEditor({
  label,
  pairs,
  onChange,
}: {
  label: string;
  pairs: Pair[];
  onChange: (pairs: Pair[]) => void;
}) {
  const setPair = (index: number, next: Pair) => {
    onChange(pairs.map((p, i) => (i === index ? next : p)));
  };
  const add = () => onChange([...pairs, ["", ""]]);
  const remove = (index: number) => onChange(pairs.filter((_pair, i) => i !== index));

  return (
    <fieldset className="pair-editor" aria-label={label}>
      <legend>{label}</legend>
      {pairs.map((pair, index) => (
        <div key={index} className="pair-editor__row">
          <input
            aria-label={`${label} key ${index + 1}`}
            value={pair[0]}
            onChange={(e) => setPair(index, [e.target.value, pair[1]])}
          />
          <input
            aria-label={`${label} value ${index + 1}`}
            value={pair[1]}
            onChange={(e) => setPair(index, [pair[0], e.target.value])}
          />
          <button type="button" onClick={() => remove(index)}>
            Remove
          </button>
        </div>
      ))}
      <button type="button" onClick={add}>
        Add {label}
      </button>
    </fieldset>
  );
}

export function ServerForm({ server = null, onSaved, onCancel }: ServerFormProps) {
  const addServer = useToolsStore((s) => s.addServer);
  const updateServer = useToolsStore((s) => s.updateServer);

  const [fields, setFields] = useState(() => initialFields(server));
  const [error, setError] = useState<string | null>(null);

  const isEdit = server !== null;

  const buildTransport = (): McpTransport => {
    if (fields.kind === "stdio") {
      const args = fields.argsText.trim().length === 0 ? [] : fields.argsText.trim().split(/\s+/);
      return { type: "stdio", command: fields.command.trim(), args, env: fields.env };
    }
    return { type: "httpSse", url: fields.url.trim(), headers: fields.headers };
  };

  const validate = (): string | null => {
    if (fields.name.trim().length === 0) return "Name is required.";
    if (fields.kind === "stdio" && fields.command.trim().length === 0) {
      return "Command is required for a stdio transport.";
    }
    if (fields.kind === "httpSse" && fields.url.trim().length === 0) {
      return "URL is required for an httpSse transport.";
    }
    return null;
  };

  const submit = async () => {
    const validationError = validate();
    if (validationError !== null) {
      setError(validationError);
      return;
    }
    const input: McpServerInput = {
      name: fields.name.trim(),
      transport: buildTransport(),
      permissionMode: fields.permissionMode,
      enabled: fields.enabled,
    };
    try {
      const saved = isEdit ? await updateServer(server.id, input) : await addServer(input);
      setError(null);
      onSaved?.(saved);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  return (
    <form
      className="server-form"
      aria-label={isEdit ? "Edit MCP server" : "Add MCP server"}
      onSubmit={(e) => {
        e.preventDefault();
        void submit();
      }}
    >
      <label className="server-form__field">
        <span>Name</span>
        <input
          aria-label="Server name"
          value={fields.name}
          onChange={(e) => setFields({ ...fields, name: e.target.value })}
        />
      </label>

      <div className="server-form__transport" role="radiogroup" aria-label="Transport">
        <button
          type="button"
          role="radio"
          aria-checked={fields.kind === "stdio"}
          data-active={fields.kind === "stdio"}
          onClick={() => setFields({ ...fields, kind: "stdio" })}
        >
          stdio
        </button>
        <button
          type="button"
          role="radio"
          aria-checked={fields.kind === "httpSse"}
          data-active={fields.kind === "httpSse"}
          onClick={() => setFields({ ...fields, kind: "httpSse" })}
        >
          httpSse
        </button>
      </div>

      {fields.kind === "stdio" ? (
        <>
          <label className="server-form__field">
            <span>Command</span>
            <input
              aria-label="Command"
              value={fields.command}
              onChange={(e) => setFields({ ...fields, command: e.target.value })}
            />
          </label>
          <label className="server-form__field">
            <span>Arguments</span>
            <input
              aria-label="Arguments"
              value={fields.argsText}
              onChange={(e) => setFields({ ...fields, argsText: e.target.value })}
            />
          </label>
          <PairEditor
            label="Environment"
            pairs={fields.env}
            onChange={(env) => setFields({ ...fields, env })}
          />
        </>
      ) : (
        <>
          <label className="server-form__field">
            <span>URL</span>
            <input
              aria-label="URL"
              value={fields.url}
              onChange={(e) => setFields({ ...fields, url: e.target.value })}
            />
          </label>
          <PairEditor
            label="Headers"
            pairs={fields.headers}
            onChange={(headers) => setFields({ ...fields, headers })}
          />
        </>
      )}

      <label className="server-form__field">
        <span>Permission mode</span>
        <select
          aria-label="Permission mode"
          value={fields.permissionMode}
          onChange={(e) =>
            setFields({ ...fields, permissionMode: e.target.value as PermissionMode })
          }
        >
          <option value="ask">Ask</option>
          <option value="allow">Allow</option>
          <option value="deny">Deny</option>
        </select>
      </label>

      <label className="server-form__enabled">
        <input
          type="checkbox"
          checked={fields.enabled}
          onChange={(e) => setFields({ ...fields, enabled: e.target.checked })}
        />
        Enabled
      </label>

      {error !== null && (
        <p className="server-form__error" role="alert">
          {error}
        </p>
      )}

      <div className="server-form__actions">
        {onCancel !== undefined && (
          <button type="button" onClick={onCancel}>
            Cancel
          </button>
        )}
        <button type="submit">{isEdit ? "Save" : "Add server"}</button>
      </div>
    </form>
  );
}
