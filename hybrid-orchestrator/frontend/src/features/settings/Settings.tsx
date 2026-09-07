// Settings area (FEAT-003).
//
// The heavy-configuration home, reachable from the shell sidebar. A left
// sub-navigation switches between sections, each of which REUSES an existing
// surface/control where one exists (MCP / Tools -> ToolManager, Agents ->
// AgentEditor) or a thin wrapper over an existing command (Providers & Keys,
// Local Runtimes). The per-conversation routing-mode control deliberately stays
// near Chat; the Routing section here only documents the modes and hosts future
// global defaults.

import { useState } from "react";
import { AgentEditor } from "../agent-editor/AgentEditor";
import { ToolManager } from "../tool-manager/ToolManager";
import { AboutUpdatesSection } from "./AboutUpdatesSection";
import { AppearanceSection } from "./AppearanceSection";
import { LocalRuntimesSection } from "./LocalRuntimesSection";
import { ProviderKeysSection } from "./ProviderKeysSection";
import { RoutingSection } from "./RoutingSection";

/** The Settings sub-sections, in display order. */
type SettingsSection =
  "providers" | "localRuntimes" | "tools" | "agents" | "routing" | "appearance" | "about";

const SECTIONS: ReadonlyArray<{ id: SettingsSection; label: string }> = [
  { id: "providers", label: "Providers & Keys" },
  { id: "localRuntimes", label: "Local Runtimes" },
  { id: "tools", label: "MCP / Tools" },
  { id: "agents", label: "Agents" },
  { id: "routing", label: "Routing" },
  { id: "appearance", label: "Appearance" },
  { id: "about", label: "About / Updates" },
];

export function Settings() {
  const [section, setSection] = useState<SettingsSection>("providers");

  return (
    <div className="settings" aria-label="Settings">
      <nav className="settings__nav" role="navigation" aria-label="Settings sections">
        {SECTIONS.map(({ id, label }) => (
          <button
            key={id}
            type="button"
            data-active={section === id}
            aria-current={section === id ? "page" : undefined}
            onClick={() => setSection(id)}
          >
            {label}
          </button>
        ))}
      </nav>

      {section === "providers" && <ProviderKeysSection />}
      {section === "localRuntimes" && <LocalRuntimesSection />}
      {section === "tools" && <ToolManager />}
      {section === "agents" && <AgentEditor />}
      {section === "routing" && <RoutingSection />}
      {section === "appearance" && <AppearanceSection />}
      {section === "about" && <AboutUpdatesSection />}
    </div>
  );
}
