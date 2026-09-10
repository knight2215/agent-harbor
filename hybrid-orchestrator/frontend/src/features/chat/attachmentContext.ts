// Attachment / repository context assembly (FEAT-003, architecture.md Section
// 8.1 chat surface).
//
// Pure helpers that turn the store's one-turn `Attachment[]` into a bounded,
// clearly-delimited context block PREPENDED to the trimmed draft before it is
// sent as the user message `content`. The pipeline treats `content` as a plain
// String, so this whole mechanism rides inside the existing turn with NO change
// to `run_turn` / `send_message` semantics.
//
// IMAGE HANDLING (documented limitation): `run_turn` maps only the text
// `content` into the provider history, so full multimodal image bytes cannot be
// passed through this path in THIS iteration. We keep the vision-capability GATE
// (an image is only ever added to the store when the selected model supports
// vision) and, for a vision-supported image, we include a clearly-labelled NOTE
// (name + mime + size) rather than silently dropping it. Passing the actual
// image bytes to a vision model is a documented follow-up.

import type { Attachment } from "../../state/conversations";
import type { AvailableModel, ManualRoute, WebSearchResultView } from "../../types";

/**
 * Whether the model effective for the NEXT turn supports vision, so the composer
 * can gate image attachments (FEAT-003). The effective model is:
 *   1. the transient per-message override (`pendingOverride`), else
 *   2. the conversation pin (`conversationPref`), else
 *   3. the routed/first available model (Auto).
 * Returns false when there are no models at all (nothing can accept an image).
 */
export function effectiveModelSupportsVision(
  models: AvailableModel[],
  override: ManualRoute | null,
  pin: ManualRoute | null,
): boolean {
  const list = Array.isArray(models) ? models : [];
  if (list.length === 0) return false;
  const route = override ?? pin;
  const selected =
    route !== null
      ? list.find((m) => m.providerId === route.providerId && m.model === route.model)
      : undefined;
  // Auto (or a route that no longer matches an available model) falls back to
  // the first available model, mirroring how routing picks a default candidate.
  const effective = selected ?? list[0];
  return effective.capabilities.vision;
}

/**
 * The HARD cap on the assembled context block, in bytes (128 KiB). This bounds
 * BOTH the repository-file selection (the picker disables selection past it) and
 * the final assembled block (truncated if it would exceed it), keeping the
 * prompt and the core well within `MAX_MESSAGE_LEN`.
 */
export const MAX_CONTEXT_BYTES = 128 * 1024;

/** UTF-8 byte length of a string (attachments are sized by bytes, not chars). */
export function byteLength(text: string): number {
  return new TextEncoder().encode(text).length;
}

/**
 * Sum the byte sizes of the given attachments (used by the repo picker to keep
 * a running total against {@link MAX_CONTEXT_BYTES}).
 */
export function totalBytes(attachments: Attachment[]): number {
  return attachments.reduce((sum, a) => sum + a.byteLen, 0);
}

/** Render one attachment as its delimited context section. */
function renderAttachment(attachment: Attachment): string {
  if (attachment.kind === "image") {
    // Vision-gated: only reached when the selected model supports vision. We
    // include a labelled note rather than the raw bytes (documented follow-up).
    return `### Attached image: ${attachment.name}\n(image, ${attachment.mimeType ?? "unknown type"}, ${attachment.byteLen} bytes; passed as a note in this build)`;
  }
  const label = attachment.kind === "repo" ? "Repository file" : "Attached file";
  const body = attachment.text ?? "";
  return `### ${label}: ${attachment.name}\n\`\`\`\n${body}\n\`\`\``;
}

/**
 * Assemble the final message `content` from the one-turn attachments and the
 * trimmed draft: a bounded, delimited context block PREPENDED to the draft.
 *
 * The context block is capped at {@link MAX_CONTEXT_BYTES}: sections are added
 * in order until the next one would exceed the cap, at which point a truncation
 * marker is appended and the rest are dropped. The user's draft is ALWAYS
 * preserved in full (it is appended after the block, not counted against the
 * cap) so a large attachment never eats the user's own message.
 */
export function assembleContent(attachments: Attachment[], draft: string): string {
  if (attachments.length === 0) {
    return draft;
  }
  const sections: string[] = [];
  let used = 0;
  let truncated = false;
  for (const attachment of attachments) {
    const section = renderAttachment(attachment);
    const sectionBytes = byteLength(section);
    if (used + sectionBytes > MAX_CONTEXT_BYTES) {
      truncated = true;
      break;
    }
    sections.push(section);
    used += sectionBytes;
  }
  const header = "The user attached the following context for this message:";
  const block = [header, ...sections].join("\n\n");
  const withMarker = truncated
    ? `${block}\n\n_(some attached context was omitted to stay within the size limit)_`
    : block;
  return draft.length > 0 ? `${withMarker}\n\n${draft}` : withMarker;
}

/**
 * Prepend a bounded, clearly-delimited "Web search results" block to `content`
 * (FEAT-004). Called by the composer when the 🌐 toggle is ON and a search
 * succeeded, BEFORE the message is sent, so the model answers with the fetched
 * results as context. The pipeline treats `content` as a plain String, so this
 * rides inside the existing turn with no change to `run_turn` / `send_message`.
 *
 * The block is capped at {@link MAX_CONTEXT_BYTES} using the SAME
 * add-until-it-would-exceed approach as {@link assembleContent}: results are
 * appended in order until the next one would exceed the cap, at which point a
 * truncation marker is added and the rest are dropped. The user's `content` is
 * ALWAYS preserved in full (it is appended after the block). An empty result
 * list returns `content` unchanged so an empty search never adds noise.
 */
export function assembleWebSearchContent(results: WebSearchResultView[], content: string): string {
  const list = Array.isArray(results) ? results : [];
  if (list.length === 0) {
    return content;
  }
  const sections: string[] = [];
  let used = 0;
  let truncated = false;
  for (const result of list) {
    const section = `- ${result.title}\n  ${result.url}\n  ${result.snippet}`;
    const sectionBytes = byteLength(section);
    if (used + sectionBytes > MAX_CONTEXT_BYTES) {
      truncated = true;
      break;
    }
    sections.push(section);
    used += sectionBytes;
  }
  const header =
    "Web search results for this message (use them to inform your answer, and cite URLs when relevant):";
  const block = [header, ...sections].join("\n\n");
  const withMarker = truncated
    ? `${block}\n\n_(some web results were omitted to stay within the size limit)_`
    : block;
  return content.length > 0 ? `${withMarker}\n\n${content}` : withMarker;
}
