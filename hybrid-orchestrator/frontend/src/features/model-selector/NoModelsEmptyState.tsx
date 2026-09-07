// NoModelsEmptyState (architecture.md Section 8.2 model selector).
//
// A concise, token-styled empty state shown wherever the model picker has no
// available models. Instead of a bare "No models available." line it guides the
// user toward configuring a provider (Settings -> Providers & Keys) or starting
// a local runtime, so the chat surface is never a dead end.

export function NoModelsEmptyState() {
  return (
    <p className="model-picker__empty">
      No models available yet. Add a provider under <strong>Settings → Providers &amp; Keys</strong>{" "}
      or start a local runtime to get started.
    </p>
  );
}
