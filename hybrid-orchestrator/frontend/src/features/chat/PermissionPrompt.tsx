// PermissionPrompt (architecture.md Section 8.1 / 9.4 Ask-mode gate).
//
// A modal shown while there is a pending Ask-mode tool-permission request. It
// shows the first queued request and resolves it via the conversations store's
// `resolvePermission(requestId, { allow, remember })`; the store dequeues it and
// the next pending request (if any) takes its place. Renders nothing when the
// queue is empty.

import { useState } from "react";
import { useConversationsStore } from "../../state/conversations";

export function PermissionPrompt() {
  const pending = useConversationsStore((s) => s.pendingPermissions);
  const resolvePermission = useConversationsStore((s) => s.resolvePermission);
  const [remember, setRemember] = useState(false);

  const request = pending[0] ?? null;
  if (request === null) return null;

  const decide = (allow: boolean) => {
    void resolvePermission(request.requestId, { allow, remember });
    setRemember(false);
  };

  return (
    <div className="permission-prompt" role="dialog" aria-modal="true" aria-label="Tool permission">
      <div className="permission-prompt__body">
        <p className="permission-prompt__title">
          Allow <strong>{request.toolName}</strong>?
        </p>
        <p className="permission-prompt__rationale">{request.rationale}</p>
        <label className="permission-prompt__remember">
          <input
            type="checkbox"
            checked={remember}
            onChange={(event) => setRemember(event.target.checked)}
          />
          Remember for this session
        </label>
        <div className="permission-prompt__actions">
          <button type="button" onClick={() => decide(false)}>
            Deny
          </button>
          <button type="button" onClick={() => decide(true)}>
            Allow
          </button>
        </div>
      </div>
    </div>
  );
}
