// PersonaEditor (architecture.md Section 8.4 agent editor).
//
// Edits a persona's name, system prompt, model parameters (via
// ModelParametersControl), default route (via the reused ProviderModelPicker in
// DefaultRoutePicker), allowed tool servers (AllowedToolsSelector), and routing
// hint (RoutingHintControl). Save builds the full `PersonaInput` and delegates
// to the personas store's `create` / `update` (which call `create_persona` /
// `update_persona`). Required fields (name, systemPrompt) are validated
// client-side to match the Rust bounds; a CommandError is surfaced inline.

import { useEffect, useState } from "react";
import { usePersonasStore } from "../../state/personas";
import type { PersonaInput } from "../../ipc/commands";
import type { AgentPersona, ManualRoute, ModelParameters, RoutingHint } from "../../types";
import { AllowedToolsSelector } from "./AllowedToolsSelector";
import { DefaultRoutePicker } from "./DefaultRoutePicker";
import { ModelParametersControl } from "./ModelParametersControl";
import { RoutingHintControl } from "./RoutingHintControl";

const EMPTY_PARAMETERS: ModelParameters = {
  temperature: null,
  maxTokens: null,
  topP: null,
  frequencyPenalty: null,
  presencePenalty: null,
  stop: null,
};

interface EditorFields {
  name: string;
  systemPrompt: string;
  parameters: ModelParameters;
  defaultRoute: ManualRoute | null;
  routingHint: RoutingHint | null;
  allowedToolServers: string[];
}

/** Seed the editor fields from an existing persona, or blank defaults. */
function fieldsFrom(persona: AgentPersona | null): EditorFields {
  if (persona === null) {
    return {
      name: "",
      systemPrompt: "",
      parameters: EMPTY_PARAMETERS,
      defaultRoute: null,
      routingHint: null,
      allowedToolServers: [],
    };
  }
  return {
    name: persona.name,
    systemPrompt: persona.systemPrompt,
    parameters: persona.parameters,
    defaultRoute: persona.defaultRoute,
    routingHint: persona.routingHint,
    allowedToolServers: persona.allowedToolServers,
  };
}

export interface PersonaEditorProps {
  /** The persona being edited, or null to create a new one. */
  persona?: AgentPersona | null;
  /** Called after a successful save. */
  onSaved?: (persona: AgentPersona) => void;
}

export function PersonaEditor({ persona = null, onSaved }: PersonaEditorProps) {
  const create = usePersonasStore((s) => s.create);
  const update = usePersonasStore((s) => s.update);

  const [fields, setFields] = useState<EditorFields>(() => fieldsFrom(persona));
  const [error, setError] = useState<string | null>(null);

  // Re-seed when the selected persona changes.
  useEffect(() => {
    setFields(fieldsFrom(persona));
    setError(null);
  }, [persona]);

  const isEdit = persona !== null;

  const validate = (): string | null => {
    if (fields.name.trim().length === 0) return "Name is required.";
    if (fields.systemPrompt.trim().length === 0) return "System prompt is required.";
    return null;
  };

  const submit = async () => {
    const validationError = validate();
    if (validationError !== null) {
      setError(validationError);
      return;
    }
    const input: PersonaInput = {
      name: fields.name.trim(),
      systemPrompt: fields.systemPrompt.trim(),
      defaultRoute: fields.defaultRoute,
      routingHint: fields.routingHint,
      allowedToolServers: fields.allowedToolServers,
      parameters: fields.parameters,
    };
    try {
      const saved = isEdit ? await update(persona.id, input) : await create(input);
      setError(null);
      onSaved?.(saved);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  return (
    <form
      className="persona-editor"
      aria-label={isEdit ? "Edit persona" : "New persona"}
      onSubmit={(e) => {
        e.preventDefault();
        void submit();
      }}
    >
      <label className="persona-editor__field">
        <span>Name</span>
        <input
          aria-label="Persona name"
          value={fields.name}
          onChange={(e) => setFields({ ...fields, name: e.target.value })}
        />
      </label>

      <label className="persona-editor__field">
        <span>System prompt</span>
        <textarea
          aria-label="System prompt"
          value={fields.systemPrompt}
          onChange={(e) => setFields({ ...fields, systemPrompt: e.target.value })}
        />
      </label>

      <ModelParametersControl
        value={fields.parameters}
        onChange={(parameters) => setFields({ ...fields, parameters })}
      />

      <DefaultRoutePicker
        value={fields.defaultRoute}
        onChange={(defaultRoute) => setFields({ ...fields, defaultRoute })}
      />

      <RoutingHintControl
        value={fields.routingHint}
        onChange={(routingHint) => setFields({ ...fields, routingHint })}
      />

      <AllowedToolsSelector
        value={fields.allowedToolServers}
        onChange={(allowedToolServers) => setFields({ ...fields, allowedToolServers })}
      />

      {error !== null && (
        <p className="persona-editor__error" role="alert">
          {error}
        </p>
      )}

      <div className="persona-editor__actions">
        <button type="submit">{isEdit ? "Save" : "Create persona"}</button>
      </div>
    </form>
  );
}
