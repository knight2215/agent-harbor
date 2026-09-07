// Appearance settings section (FEAT-003).
//
// An accessible theme toggle (Light / Dark / System) wired to the theme store
// (state/theme.ts), which applies data-theme on <html> and persists the choice
// to localStorage under `ah-theme`. System follows the OS preference.

import { useThemeStore } from "../../state/theme";
import type { ThemePreference } from "../../state/theme";

const OPTIONS: ReadonlyArray<{ value: ThemePreference; label: string }> = [
  { value: "light", label: "Light" },
  { value: "dark", label: "Dark" },
  { value: "system", label: "System" },
];

export function AppearanceSection() {
  const preference = useThemeStore((s) => s.preference);
  const setPreference = useThemeStore((s) => s.setPreference);

  return (
    <section className="settings__panel" role="region" aria-label="Appearance">
      <h3 className="settings__section-title">Appearance</h3>
      <p className="settings__section-desc">
        Choose a color theme. &ldquo;System&rdquo; follows your operating system preference.
      </p>
      <div className="theme-toggle" role="radiogroup" aria-label="Theme">
        {OPTIONS.map(({ value, label }) => (
          <button
            key={value}
            type="button"
            role="radio"
            aria-checked={preference === value}
            data-active={preference === value}
            onClick={() => setPreference(value)}
          >
            {label}
          </button>
        ))}
      </div>
    </section>
  );
}
