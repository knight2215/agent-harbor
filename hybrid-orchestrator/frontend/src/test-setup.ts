import "@testing-library/jest-dom";

// jsdom does not implement `window.matchMedia`, which the theme module
// (state/theme.ts) calls to resolve the system color-scheme preference. Provide
// a minimal, non-matching stub so importing the theme store never throws in the
// test environment. Tests that need a specific match can override it.
if (typeof window !== "undefined" && typeof window.matchMedia !== "function") {
  window.matchMedia = (query: string): MediaQueryList =>
    ({
      matches: false,
      media: query,
      onchange: null,
      addListener: () => undefined,
      removeListener: () => undefined,
      addEventListener: () => undefined,
      removeEventListener: () => undefined,
      dispatchEvent: () => false,
    }) as MediaQueryList;
}
