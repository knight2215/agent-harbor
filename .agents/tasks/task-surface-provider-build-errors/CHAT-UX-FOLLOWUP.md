# Follow-up: Chat surface UX + "hello to qwen3 produced no response" (user-reported)

This is a SEPARATE follow-up captured from a live user steering message during the
`task-surface-provider-build-errors` work. It is intentionally NOT folded into that task,
whose scope (per the orchestrator briefing) is limited to provider build-error surfacing,
the Gemini-by-key build fix, and provider base_url UX/validation. These chat items need
their own task, tracing, and tests. Recorded here so the orchestrator can triage/relay.

## Reported symptoms (v0.7.3 build)
1. The composer does not behave like a chat input: pressing **Enter** does not send the
   message, and **Shift+Enter** does not insert a newline.
2. Messages do not **thread** (no conversational threading like the Kiro UI).
3. The user typed **"hello"** after the app correctly detected the local **qwen3:8b**
   model from Ollama, but got **no assistant response and no error message** (silent
   no-op).

## Confirmed root cause for #1 (Enter to send / Shift+Enter newline)
`hybrid-orchestrator/frontend/src/features/chat/Composer.tsx` renders a `<textarea>` with
`onChange` only and NO `onKeyDown` handler. The form only submits via the "Send" button
click, so Enter never triggers `submit()`. A `<textarea>` also inserts a newline on plain
Enter by default, which is the opposite of the requested behavior.

### Suggested fix (for the future task)
Add an `onKeyDown` to the textarea:
- `Enter` without Shift => `event.preventDefault(); submit();`
- `Enter` with Shift => let the default newline insertion happen.
- Guard for IME composition (`event.nativeEvent.isComposing`) so CJK input is not
  swallowed.
Add a vitest in `chat.test.tsx` mocking `invoke` for `send_message` (and any command the
chat mounts) that asserts: plain Enter calls the store `sendMessage` and clears the draft;
Shift+Enter does not send and preserves/extends the draft.

## Needs tracing for #2 and #3 (out of scope here; do NOT guess a fix)
- #3 (no response, no error) is the more serious issue. Trace the send path:
  `state/conversations.ts` `sendMessage` -> `ipc/commands.ts` `sendMessage` ->
  `send_message` command (`crates/tauri-app/src/commands.rs`) -> pipeline `run_turn`
  (`crates/orchestrator-core/src/pipeline.rs`) -> the streaming `CoreEvent`s
  (`messageStarted`/`messageDelta`/`messageComplete`/`messageError`) and how the frontend
  event bridge subscribes to them. A silent no-op with a detected Ollama model suggests
  either the turn is not being spawned, the streamed events are not reaching the webview
  event listener, or an error is being swallowed instead of surfaced as `messageError`.
  NOTE: this cannot be reproduced in the INTEGRATIONS_ONLY sandbox (no live Ollama), so it
  needs host-based / user-build validation.
- #2 (threading) is a UI/architecture change for `MessageList`/`MessageBubble` and should
  be specced against architecture.md Section 8.1 before implementation.

## Relationship to the current task
The current task's error-surfacing work will make provider-side failures visible in the
Diagnostics panel and picker, but it does NOT address a chat turn that produces neither a
streamed reply nor a `messageError`. That is a distinct pipeline/event-bridge concern.
