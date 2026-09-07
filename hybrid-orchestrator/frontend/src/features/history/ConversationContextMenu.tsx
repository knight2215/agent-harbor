// ConversationContextMenu (architecture.md Section 8.5 history surface).
//
// The per-conversation actions menu. Each action delegates to the conversations
// store, which is the single owner of the IPC calls (the core is authoritative,
// Section 7.3):
//   - rename    -> renameConversation
//   - tag       -> setConversationTags
//   - duplicate -> duplicateConversation (seed a new conversation)
//   - export    -> exportConversation, then trigger a browser download of the
//                  returned display-safe string
//   - delete    -> deleteConversation
//
// The rename/tag prompts use the platform prompt for a minimal Phase 5 surface;
// richer inline editors are a later-phase concern.

import { useConversationsStore } from "../../state/conversations";
import type { Conversation, ExportFormat, PrivacyTag } from "../../types";

export interface ConversationContextMenuProps {
  conversation: Conversation;
}

/** Serialize a privacy tag back to its human-editable text form. */
function tagToText(tag: PrivacyTag): string {
  if (typeof tag === "string") return tag;
  return `custom:${tag.custom}`;
}

/** Parse a comma-separated tag list into PrivacyTag values. */
function parseTags(input: string): PrivacyTag[] {
  return input
    .split(",")
    .map((part) => part.trim())
    .filter((part) => part.length > 0)
    .map((part): PrivacyTag => {
      if (part === "localOnly" || part === "confidential") return part;
      const custom = part.startsWith("custom:") ? part.slice("custom:".length) : part;
      return { custom };
    });
}

/** Trigger a browser download of `content` as a file named `filename`. */
function downloadText(filename: string, content: string): void {
  const blob = new Blob([content], { type: "text/plain;charset=utf-8" });
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = filename;
  document.body.appendChild(anchor);
  anchor.click();
  anchor.remove();
  URL.revokeObjectURL(url);
}

export function ConversationContextMenu({ conversation }: ConversationContextMenuProps) {
  const renameConversation = useConversationsStore((s) => s.renameConversation);
  const setConversationTags = useConversationsStore((s) => s.setConversationTags);
  const duplicateConversation = useConversationsStore((s) => s.duplicateConversation);
  const exportConversation = useConversationsStore((s) => s.exportConversation);
  const deleteConversation = useConversationsStore((s) => s.deleteConversation);

  const rename = () => {
    const title = window.prompt("Rename conversation", conversation.title);
    if (title === null) return;
    const trimmed = title.trim();
    if (trimmed.length === 0) return;
    void renameConversation(conversation.id, trimmed);
  };

  const tag = () => {
    const current = conversation.privacyTags.map(tagToText).join(", ");
    const input = window.prompt("Tags (comma-separated)", current);
    if (input === null) return;
    void setConversationTags(conversation.id, parseTags(input));
  };

  const duplicate = () => {
    void duplicateConversation(conversation.id);
  };

  const exportAs = async (format: ExportFormat) => {
    const content = await exportConversation(conversation.id, format);
    const extension = format === "json" ? "json" : "md";
    downloadText(`${conversation.title}.${extension}`, content);
  };

  const remove = () => {
    void deleteConversation(conversation.id);
  };

  return (
    <div className="conversation-menu" role="menu" aria-label={`Actions for ${conversation.title}`}>
      <button type="button" role="menuitem" onClick={rename}>
        Rename
      </button>
      <button type="button" role="menuitem" onClick={tag}>
        Tag
      </button>
      <button type="button" role="menuitem" onClick={duplicate}>
        Duplicate
      </button>
      <button type="button" role="menuitem" onClick={() => void exportAs("markdown")}>
        Export Markdown
      </button>
      <button type="button" role="menuitem" onClick={() => void exportAs("json")}>
        Export JSON
      </button>
      <button type="button" role="menuitem" onClick={remove}>
        Delete
      </button>
    </div>
  );
}
