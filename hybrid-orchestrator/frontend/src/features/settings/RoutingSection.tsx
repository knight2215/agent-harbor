// Routing settings section (FEAT-003).
//
// Explains the per-conversation routing modes surfaced by the RoutingModeToggle
// near the Chat view. The per-conversation control itself intentionally stays
// next to Chat (Section 8.2); this section documents what each mode does and
// serves as the home for future global routing defaults.

export function RoutingSection() {
  return (
    <section className="settings__panel" role="region" aria-label="Routing">
      <h3 className="settings__section-title">Routing</h3>
      <p className="settings__section-desc">
        Each conversation picks how models are chosen using the control next to the chat composer.
        The available modes are:
      </p>
      <ul>
        <li>
          <strong>Auto</strong> — the router balances quality, cost, and speed automatically, always
          honoring privacy constraints.
        </li>
        <li>
          <strong>Prefer Local</strong> — biases the automatic router toward locally-hosted models.
        </li>
        <li>
          <strong>Prefer Quality</strong> — biases the automatic router toward the
          highest-capability models.
        </li>
        <li>
          <strong>Manual</strong> — pins a specific provider and model for the conversation.
        </li>
      </ul>
      <p className="settings__section-desc">
        Privacy-tagged conversations are always routed locally regardless of the selected mode.
      </p>
    </section>
  );
}
