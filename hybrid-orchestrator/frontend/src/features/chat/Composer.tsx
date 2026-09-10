// Composer (architecture.md Section 8.1 chat surface; FEAT-002 Kiro redesign +
// FEAT-003 attach/repository context).
//
// A BOTTOM-ANCHORED message input with a compact control row wrapped around the
// textarea, replacing the old wall of controls stacked above the input. The
// control row hosts, inline:
//   - Attach (📎): opens the native file dialog (tauri-plugin-dialog) for text
//     and image files. Text files are read via `read_text_file`; images via
//     `read_file_base64` and attached ONLY when the selected model advertises
//     `vision` (else a visible non-fatal notice, and the image is NOT attached).
//   - Repository (📁): picks a folder, lists a bounded/filtered set of files via
//     `list_repo_files`, and lets the user select a subset within a total-size
//     cap; selected files' contents are read via `read_text_file`.
//   - a Web-search TOGGLE (🌐, aria-pressed) that flips a REAL store flag
//     (FEAT-004 consumer).
//   - the InlineModelControl (compact model dropdown reading/writing the shared
//     transient per-message override).
//   - the compact routing-mode control, the "Why this model?" info icon, the
//     warning-icon -> modal for provider enumeration errors, and a small
//     "Refresh models" icon.
//
// Attachments (text/repo file contents + vision-gated images) are held in the
// conversations store for the NEXT turn only. On send the composer PREPENDS a
// bounded, delimited context block (assembleContent) to the trimmed draft and
// passes the whole thing as the user message `content`; the store clears the
// attachments after a SUCCESSFUL send, mirroring how the transient override is
// consumed once. `run_turn` / `send_message` semantics are unchanged (the
// context rides inside the existing `content` String).
//
// Enter-to-send / Shift+Enter-newline (IME-safe) and the Stop button semantics
// are preserved EXACTLY, as is the `sendState === "failed"` role=alert.

import { useState } from "react";
import type { KeyboardEvent } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { listRepoFiles, readFileBase64, readTextFile, runWebSearch } from "../../ipc/commands";
import { useConversationsStore } from "../../state/conversations";
import type { Attachment } from "../../state/conversations";
import { useProvidersStore } from "../../state/providers";
import type { RepoFileEntry, RepoListing } from "../../types";
import {
  AutoRationaleTooltip,
  EnumerationErrorModal,
  InlineModelControl,
  RoutingModeToggle,
} from "../model-selector";
import {
  assembleContent,
  assembleWebSearchContent,
  effectiveModelSupportsVision,
  totalBytes,
} from "./attachmentContext";
import { RepoPicker } from "./RepoPicker";

/** File extensions offered as "text" in the attach dialog filter. */
const TEXT_EXTENSIONS = [
  "txt",
  "md",
  "markdown",
  "rs",
  "ts",
  "tsx",
  "js",
  "jsx",
  "json",
  "toml",
  "yaml",
  "yml",
  "py",
  "go",
  "java",
  "c",
  "cpp",
  "h",
  "css",
  "html",
  "sh",
];
/** Image extensions offered in the attach dialog filter (vision-gated). */
const IMAGE_EXTENSIONS = ["png", "jpg", "jpeg", "gif", "webp", "bmp"];

/** Whether a path's extension marks it as an image for the attach branch. */
function isImagePath(path: string): boolean {
  const dot = path.lastIndexOf(".");
  if (dot === -1) return false;
  return IMAGE_EXTENSIONS.includes(path.slice(dot + 1).toLowerCase());
}

export function Composer() {
  const activeConversationId = useConversationsStore((s) => s.activeConversationId);
  const conversations = useConversationsStore((s) => s.conversations);
  const sendMessage = useConversationsStore((s) => s.sendMessage);
  const stopGeneration = useConversationsStore((s) => s.stopGeneration);
  // A rejected `send_message` IPC call produces NO CoreEvents, so this store
  // state is the ONLY visible signal that a send failed (FEAT-002 / Issue 2).
  const sendState = useConversationsStore((s) => s.sendState);
  const sendError = useConversationsStore((s) => s.sendError);
  // The web-search toggle flips a REAL on/off flag owned by the conversations
  // store so FEAT-004 can consume it when assembling the send context.
  const webSearchEnabled = useConversationsStore((s) => s.webSearchEnabled);
  const setWebSearchEnabled = useConversationsStore((s) => s.setWebSearchEnabled);
  // One-turn attachments (text / repo files / vision-gated images), held in the
  // store so they clear after a successful send (FEAT-003).
  const attachments = useConversationsStore((s) => s.attachments);
  const addAttachment = useConversationsStore((s) => s.addAttachment);
  const removeAttachment = useConversationsStore((s) => s.removeAttachment);
  const pendingOverride = useConversationsStore((s) => s.pendingOverride);
  const models = useProvidersStore((s) => s.models);
  // Re-enumerate models on demand (e.g. after starting a local runtime or saving
  // a key) without reopening Settings. Fire-and-forget; failures surface via the
  // providers store (the warning-icon modal).
  const loadModels = useProvidersStore((s) => s.load);
  // A turn is in progress while any message is still streaming. The stop control
  // is only presented as live in that window; the underlying `stop_generation`
  // command is a validated no-op today (the Phase 4 pipeline has no cancel seam),
  // so the button stays disabled and is labelled unavailable rather than posing
  // as a working control (review issue #3).
  const streaming = useConversationsStore((s) =>
    (s.messages ?? []).some((m) => m.status === "streaming"),
  );
  const [draft, setDraft] = useState("");
  // A non-fatal notice shown near the composer (e.g. "This model doesn't support
  // images", or a file-read failure). Cleared when a new attach/repo action
  // starts so it never lingers stale.
  const [notice, setNotice] = useState<string | null>(null);
  // The active repository listing to pick from, or null when the picker closed.
  const [repoListing, setRepoListing] = useState<RepoListing | null>(null);

  const disabled = activeConversationId === null;

  const activeConversation = conversations.find((c) => c.id === activeConversationId) ?? null;
  const pin = activeConversation?.conversationPref ?? null;

  // Handle the Attach (📎) control: pick one or more text/image files, read each
  // via the appropriate backend command, and add it as a one-turn attachment.
  // Images are vision-GATED: an image is only attached when the effective model
  // supports vision, otherwise a visible non-fatal notice is shown.
  const onAttach = async () => {
    if (disabled) return;
    setNotice(null);
    let selected: string | string[] | null | undefined;
    try {
      selected = await open({
        multiple: true,
        directory: false,
        filters: [
          { name: "Text", extensions: TEXT_EXTENSIONS },
          { name: "Images", extensions: IMAGE_EXTENSIONS },
        ],
      });
    } catch (err: unknown) {
      setNotice(`Could not open the file dialog: ${String(err)}`);
      return;
    }
    if (selected === null || selected === undefined) return;
    const paths = Array.isArray(selected) ? selected : [selected];
    const visionOk = effectiveModelSupportsVision(models, pendingOverride, pin);
    for (const path of paths) {
      try {
        if (isImagePath(path)) {
          if (!visionOk) {
            setNotice("This model doesn't support images");
            continue;
          }
          const view = await readFileBase64(path);
          addAttachment({
            kind: "image",
            name: view.name,
            path: view.path,
            byteLen: view.byteLen,
            base64: view.base64,
            mimeType: view.mimeType,
          });
        } else {
          const view = await readTextFile(path);
          addAttachment({
            kind: "text",
            name: view.name,
            path: view.path,
            byteLen: view.byteLen,
            text: view.text,
          });
        }
      } catch (err: unknown) {
        setNotice(`Could not attach ${path}: ${String(err)}`);
      }
    }
  };

  // Handle the Repository (📁) control: pick a folder and list its candidate
  // files; the RepoPicker then lets the user select a bounded subset.
  const onPickRepository = async () => {
    if (disabled) return;
    setNotice(null);
    let selected: string | string[] | null;
    try {
      selected = await open({ multiple: false, directory: true });
    } catch (err: unknown) {
      setNotice(`Could not open the folder dialog: ${String(err)}`);
      return;
    }
    if (typeof selected !== "string") return;
    try {
      const listing = await listRepoFiles(selected);
      if (listing.files.length === 0) {
        setNotice("No eligible files found in that folder");
        return;
      }
      setRepoListing(listing);
    } catch (err: unknown) {
      setNotice(`Could not list that folder: ${String(err)}`);
    }
  };

  // Confirm the repository selection: fetch each selected file's contents and
  // add it as a `repo` attachment, then close the picker.
  const onConfirmRepo = async (entries: RepoFileEntry[]) => {
    const dir = repoListing?.dir ?? "";
    setRepoListing(null);
    for (const entry of entries) {
      // Join the picked dir and the relative path; the backend accepts either
      // separator, and the dialog returns a native absolute dir.
      const sep = dir.includes("\\") ? "\\" : "/";
      const full = `${dir}${dir.endsWith(sep) ? "" : sep}${entry.relPath}`;
      try {
        const view = await readTextFile(full);
        addAttachment({
          kind: "repo",
          name: entry.relPath,
          path: view.path,
          byteLen: view.byteLen,
          text: view.text,
        });
      } catch (err: unknown) {
        setNotice(`Could not include ${entry.relPath}: ${String(err)}`);
      }
    }
  };

  const submit = async () => {
    const content = draft.trim();
    if (content.length === 0 || disabled) return;
    // Prepend the bounded, delimited context block from the one-turn
    // attachments; the store clears them after a successful send.
    let assembled = assembleContent(attachments, content);
    // Clear the draft immediately so the input frees up while any web search
    // runs; the assembled content is already captured above.
    setDraft("");
    // Web search (FEAT-004): when the 🌐 toggle is ON, run a search for the raw
    // draft BEFORE sending and prepend the results as context. On failure OR
    // when unconfigured, show a VISIBLE non-fatal notice and STILL send the
    // plain message - never hang, never silently drop.
    if (webSearchEnabled) {
      setNotice(null);
      try {
        const results = await runWebSearch(content);
        assembled = assembleWebSearchContent(results, assembled);
      } catch (err: unknown) {
        const reason = err instanceof Error ? err.message : String(err);
        setNotice(`Web search unavailable: ${reason}. Sending without web results.`);
      }
    }
    void sendMessage(assembled);
  };

  // Enter-to-send / Shift+Enter-newline (Issue 1). Guard IME composition so a
  // CJK candidate-commit Enter is not swallowed as a send: `isComposing` (and
  // the legacy `keyCode === 229`) mark a keydown that belongs to the input
  // method, not the user pressing Enter to send.
  const onKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (event.key !== "Enter") return;
    if (event.nativeEvent.isComposing || event.keyCode === 229) return;
    if (event.shiftKey) return;
    event.preventDefault();
    void submit();
  };

  const chipLabel = (attachment: Attachment): string => {
    if (attachment.kind === "image") return `🖼️ ${attachment.name}`;
    if (attachment.kind === "repo") return `📁 ${attachment.name}`;
    return `📄 ${attachment.name}`;
  };

  return (
    <form
      className="composer"
      onSubmit={(event) => {
        event.preventDefault();
        void submit();
      }}
    >
      {sendState === "failed" && (
        <p className="composer__send-error" role="alert">
          Failed to send: {sendError}
        </p>
      )}
      {notice !== null && (
        <p className="composer__notice" role="status" data-testid="composer-notice">
          {notice}
        </p>
      )}
      {attachments.length > 0 && (
        <ul className="composer__chips" aria-label="Attachments">
          {attachments.map((attachment) => (
            <li key={`${attachment.kind}:${attachment.path}`} className="composer__chip">
              <span className="composer__chip-label" title={attachment.path}>
                {chipLabel(attachment)}
              </span>
              <button
                type="button"
                className="composer__chip-remove"
                aria-label={`Remove attachment ${attachment.name}`}
                onClick={() => removeAttachment(attachment.path, attachment.kind)}
              >
                <span aria-hidden="true">×</span>
              </button>
            </li>
          ))}
        </ul>
      )}
      <div className="composer__box">
        <textarea
          className="composer__input"
          aria-label="Message"
          value={draft}
          disabled={disabled}
          onChange={(event) => setDraft(event.target.value)}
          onKeyDown={onKeyDown}
        />
        <div className="composer__controls">
          <div className="composer__controls-left">
            <button
              type="button"
              className="composer__icon"
              aria-label="Attach file"
              title="Attach file"
              disabled={disabled}
              onClick={() => void onAttach()}
            >
              <span aria-hidden="true">📎</span>
              {attachments.length > 0 && (
                <span className="composer__icon-badge" data-testid="attach-count">
                  {attachments.length}
                </span>
              )}
            </button>
            <div className="composer__repo">
              <button
                type="button"
                className="composer__icon"
                aria-label="Add repository context"
                title="Add repository context"
                disabled={disabled}
                onClick={() => void onPickRepository()}
              >
                <span aria-hidden="true">📁</span>
              </button>
              {repoListing !== null && (
                <RepoPicker
                  dir={repoListing.dir}
                  files={repoListing.files}
                  truncated={repoListing.truncated}
                  usedBytes={totalBytes(attachments)}
                  onConfirm={(entries) => void onConfirmRepo(entries)}
                  onCancel={() => setRepoListing(null)}
                />
              )}
            </div>
            <button
              type="button"
              className="composer__icon"
              aria-label="Toggle web search"
              aria-pressed={webSearchEnabled}
              title={webSearchEnabled ? "Web search: on" : "Web search: off"}
              data-active={webSearchEnabled}
              onClick={() => setWebSearchEnabled(!webSearchEnabled)}
            >
              <span aria-hidden="true">🌐</span>
            </button>
            <InlineControls activeConversationId={activeConversationId} loadModels={loadModels} />
          </div>
          <div className="composer__actions">
            <button
              type="button"
              className="composer__stop"
              onClick={() => void stopGeneration()}
              disabled={!streaming}
              title={streaming ? "Stop (cancellation is not yet available)" : "Nothing to stop"}
            >
              Stop
            </button>
            <button type="submit" className="composer__send" disabled={disabled}>
              Send
            </button>
          </div>
        </div>
      </div>
    </form>
  );
}

// The inline model / routing / rationale / warning / refresh cluster, split out
// only to keep the main composer body readable; it holds no state of its own.
function InlineControls({
  activeConversationId,
  loadModels,
}: {
  activeConversationId: string | null;
  loadModels: () => Promise<void>;
}) {
  return (
    <>
      <InlineModelControl />
      <RoutingModeToggle />
      <AutoRationaleTooltip conversationId={activeConversationId} />
      <EnumerationErrorModal />
      <button
        type="button"
        className="composer__icon"
        aria-label="Refresh models"
        title="Refresh models"
        onClick={() => void loadModels()}
      >
        <span aria-hidden="true">↻</span>
      </button>
    </>
  );
}
