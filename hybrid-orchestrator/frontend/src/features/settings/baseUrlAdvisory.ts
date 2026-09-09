// Shared NON-BLOCKING client advisory for a generic OpenAI-compatible base URL.
//
// Both the cloud "Providers & Keys" section (ProviderKeysSection) and the
// "Local Runtimes" section (LocalRuntimesSection) let a user paste a base URL
// into a generic OpenAI-compatible field, and both need to steer the user away
// from the same two real-world mistakes: pasting a Kiro `.../session/<id>` URL
// or a Gemini `...:generateContent` model URL (plus a bare
// `generativelanguage.googleapis.com` host) instead of an OpenAI-compatible API
// root. The two sections previously copy-pasted this logic verbatim; it now
// lives here so the two cannot drift.
//
// The advisory is guidance ONLY: it NEVER blocks saving (the backend remains
// the enforcement point and attaches its own corroborating advisory). Each
// section supplies its own trailing `suffix` so the example URL / Gemini-kind
// hint can differ while the shared detection and lead sentence stay identical.

/**
 * A NON-BLOCKING client advisory for a generic OpenAI-compatible base URL: the
 * entered value looks like a web/session URL or a full model endpoint (a Kiro
 * `.../session/<id>` URL, a Gemini `...:generateContent` URL, or a
 * `generativelanguage.googleapis.com` host) rather than an OpenAI-compatible API
 * root. Returns the guidance text (the shared lead sentence followed by the
 * caller-supplied `suffix`), or `null` when the URL looks like an API root.
 *
 * `suffix` lets each section append its own trailing hint (e.g. an example
 * `http://localhost:1234/v1` root, or a "use the Gemini kind" pointer) while the
 * detection logic and lead sentence remain a single source of truth.
 */
export function adviseGenericOpenAiBaseUrl(url: string, suffix: string): string | null {
  const trimmed = url.trim();
  if (trimmed === "") return null;
  const pathAndHost = trimmed.split("#")[0].split("?")[0];
  const looksLikeSession = pathAndHost.includes("/session/");
  const looksLikeGenerate = pathAndHost.endsWith(":generateContent");
  let host = "";
  try {
    host = new URL(trimmed).hostname.toLowerCase();
  } catch {
    host = "";
  }
  const isGeminiHost = host === "generativelanguage.googleapis.com";
  if (looksLikeSession || looksLikeGenerate || isGeminiHost) {
    return `This looks like a web/session or model endpoint URL, not an OpenAI-compatible API base URL. ${suffix}`;
  }
  return null;
}
