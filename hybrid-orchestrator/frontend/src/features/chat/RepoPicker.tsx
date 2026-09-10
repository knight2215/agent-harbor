// RepoPicker (FEAT-003, architecture.md Section 8.1 chat surface).
//
// A compact popover for the composer's Repository (📁) control. It takes a
// listed directory's candidate files and lets the user select a BOUNDED subset
// to include as context, enforcing a total-size cap ({@link MAX_CONTEXT_BYTES}).
// Selection past the cap is disabled, and a running "X of Y KiB included" signal
// keeps the budget visible.
//
// The picker owns only its transient selection state. Confirming a selection
// fetches each selected file's contents via `read_text_file` (in the parent) and
// adds them to the store as `kind: "repo"` attachments; this component just
// reports WHICH relative paths were chosen and their sizes.

import { useMemo, useState } from "react";
import type { RepoFileEntry } from "../../types";
import { MAX_CONTEXT_BYTES } from "./attachmentContext";

export interface RepoPickerProps {
  /** The picked directory (display label). */
  dir: string;
  /** The listed candidate files. */
  files: RepoFileEntry[];
  /** True when the backend listing was capped. */
  truncated: boolean;
  /**
   * The bytes ALREADY consumed by existing attachments, so the cap applies
   * across everything included this turn (not just this picker's selection).
   */
  usedBytes: number;
  /** Confirm the selected relative paths (parent fetches their contents). */
  onConfirm: (selected: RepoFileEntry[]) => void;
  /** Close without changing attachments. */
  onCancel: () => void;
}

function formatKiB(bytes: number): string {
  return (bytes / 1024).toFixed(1);
}

export function RepoPicker({
  dir,
  files,
  truncated,
  usedBytes,
  onConfirm,
  onCancel,
}: RepoPickerProps) {
  // Selected relative paths (a Set for O(1) membership toggles).
  const [selected, setSelected] = useState<Set<string>>(new Set());

  const selectedBytes = useMemo(
    () => files.filter((f) => selected.has(f.relPath)).reduce((sum, f) => sum + f.byteLen, 0),
    [files, selected],
  );
  const includedBytes = usedBytes + selectedBytes;

  const toggle = (entry: RepoFileEntry) => {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(entry.relPath)) {
        next.delete(entry.relPath);
      } else {
        next.add(entry.relPath);
      }
      return next;
    });
  };

  const confirm = () => {
    onConfirm(files.filter((f) => selected.has(f.relPath)));
  };

  return (
    <div className="repo-picker" role="dialog" aria-label="Select repository files">
      <div className="repo-picker__header">
        <span className="repo-picker__dir" title={dir}>
          {dir}
        </span>
        <span className="repo-picker__budget" data-testid="repo-picker-budget">
          {formatKiB(includedBytes)} of {formatKiB(MAX_CONTEXT_BYTES)} KiB included
        </span>
      </div>
      {truncated && (
        <p className="repo-picker__truncated">
          Showing the first {files.length} files; the folder has more.
        </p>
      )}
      <ul className="repo-picker__list">
        {files.map((entry) => {
          const isSelected = selected.has(entry.relPath);
          // Disable a not-yet-selected file whose size would push past the cap.
          const wouldExceed = includedBytes + entry.byteLen > MAX_CONTEXT_BYTES;
          const disabled = !isSelected && wouldExceed;
          return (
            <li key={entry.relPath} className="repo-picker__item">
              <label className="repo-picker__label">
                <input
                  type="checkbox"
                  checked={isSelected}
                  disabled={disabled}
                  aria-label={`Include ${entry.relPath}`}
                  onChange={() => toggle(entry)}
                />
                <span className="repo-picker__name">{entry.relPath}</span>
                <span className="repo-picker__size">{formatKiB(entry.byteLen)} KiB</span>
              </label>
            </li>
          );
        })}
      </ul>
      <div className="repo-picker__actions">
        <button type="button" className="repo-picker__cancel" onClick={onCancel}>
          Cancel
        </button>
        <button
          type="button"
          className="repo-picker__confirm"
          disabled={selected.size === 0}
          onClick={confirm}
        >
          Include {selected.size > 0 ? `${selected.size} file${selected.size > 1 ? "s" : ""}` : ""}
        </button>
      </div>
    </div>
  );
}
