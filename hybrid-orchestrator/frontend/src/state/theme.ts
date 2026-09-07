// Theme store (Phase 7 UX refactor, FEAT-003 Appearance).
//
// Owns the app's light/dark theme. The user's explicit choice ("light" |
// "dark") is persisted to localStorage under `ah-theme`; when no explicit
// choice is stored ("system") the theme follows the OS via
// `prefers-color-scheme`. The RESOLVED theme is applied by setting
// `data-theme="light" | "dark"` on `<html>`, which the CSS token system keys
// off (styles/tokens.css).
//
// localStorage and matchMedia exist in jsdom, but access is guarded so the
// store never throws in constrained environments; the test-setup shim provides
// a matchMedia stub for the vitest DOM.

import { create } from "zustand";

/** The user-selectable theme preference. `system` follows the OS. */
export type ThemePreference = "light" | "dark" | "system";

/** The concrete theme actually applied to the document. */
export type ResolvedTheme = "light" | "dark";

/** localStorage key holding the persisted {@link ThemePreference}. */
export const THEME_STORAGE_KEY = "ah-theme";

/** Read the stored preference, defaulting to `system` when unset/unavailable. */
function readStoredPreference(): ThemePreference {
  try {
    const stored = window.localStorage.getItem(THEME_STORAGE_KEY);
    if (stored === "light" || stored === "dark" || stored === "system") {
      return stored;
    }
  } catch {
    // Ignore storage access errors (private mode / constrained env).
  }
  return "system";
}

/** Whether the OS currently prefers a dark color scheme (guarded for jsdom). */
function systemPrefersDark(): boolean {
  try {
    return window.matchMedia("(prefers-color-scheme: dark)").matches;
  } catch {
    return false;
  }
}

/** Resolve a preference to the concrete theme to apply. */
function resolveTheme(preference: ThemePreference): ResolvedTheme {
  if (preference === "system") {
    return systemPrefersDark() ? "dark" : "light";
  }
  return preference;
}

/** Apply the resolved theme to the document root so the CSS tokens switch. */
function applyTheme(resolved: ResolvedTheme): void {
  if (typeof document !== "undefined") {
    document.documentElement.setAttribute("data-theme", resolved);
  }
}

export interface ThemeState {
  /** The user's preference (persisted). */
  preference: ThemePreference;
  /** The concrete theme currently applied to the document. */
  resolved: ResolvedTheme;
  /** Set (and persist) the preference, applying the resolved theme. */
  setPreference: (preference: ThemePreference) => void;
}

const initialPreference = readStoredPreference();
const initialResolved = resolveTheme(initialPreference);
// Apply the initial theme eagerly so the first paint matches the preference.
applyTheme(initialResolved);

export const useThemeStore = create<ThemeState>((set) => ({
  preference: initialPreference,
  resolved: initialResolved,

  setPreference: (preference) => {
    try {
      window.localStorage.setItem(THEME_STORAGE_KEY, preference);
    } catch {
      // Ignore storage write errors (private mode / constrained env).
    }
    const resolved = resolveTheme(preference);
    applyTheme(resolved);
    set({ preference, resolved });
  },
}));
