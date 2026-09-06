// UI surface 4: Agent config editor (architecture.md Section 8.4).
//
// Lists/creates/edits/deletes personas with a system prompt, model parameters,
// a default route (reusing the FEAT-002 ProviderModelPicker via
// DefaultRoutePicker), allowed tool servers, and a routing hint. Saves via
// create_persona / update_persona and refreshes on personasChanged.

export { AgentEditor } from "./AgentEditor";
export { AllowedToolsSelector } from "./AllowedToolsSelector";
export type { AllowedToolsSelectorProps } from "./AllowedToolsSelector";
export { DefaultRoutePicker } from "./DefaultRoutePicker";
export type { DefaultRoutePickerProps } from "./DefaultRoutePicker";
export { ModelParametersControl } from "./ModelParametersControl";
export type { ModelParametersControlProps } from "./ModelParametersControl";
export { PersonaEditor } from "./PersonaEditor";
export type { PersonaEditorProps } from "./PersonaEditor";
export { PersonaList } from "./PersonaList";
export { RoutingHintControl } from "./RoutingHintControl";
export type { RoutingHintControlProps } from "./RoutingHintControl";
