//! Tool-invocation permission model (architecture.md Sections 5.6 / 9.4, P3.5).
//!
//! Each MCP server carries a [`PermissionMode`] (`Ask` / `Allow` / `Deny`) plus
//! optional per-tool overrides. Before a tool is invoked the bridge
//! (`tools_bridge`) consults the [`PermissionGate`] to decide the outcome:
//!
//! - `Allow`  -> invoke immediately.
//! - `Deny`   -> refuse; the bridge turns this into a structured tool-error
//!   message so the turn does not crash.
//! - `Ask`    -> emit a [`CoreEvent::PermissionRequested`] on the core event
//!   channel and BLOCK until the frontend resolves the request via
//!   [`PermissionRegistry::resolve`] (wired to the tauri-app
//!   `resolve_permission` command). The awaited [`Decision`] then
//!   allows or denies the invocation.
//!
//! The mechanism is framework-agnostic: it depends only on the core event
//! channel (an `mpsc::UnboundedSender<CoreEvent>`) and `tokio` oneshot channels,
//! never on Tauri. The tauri-app command layer holds an [`Arc`] to the
//! [`PermissionRegistry`] on its `AppState` and calls [`resolve`] when the user
//! answers a prompt (architecture.md Section 9.4).
//!
//! [`resolve`]: PermissionRegistry::resolve

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot;
use uuid::Uuid;

use crate::events::CoreEvent;
use crate::models::PermissionMode;

/// The user's answer to a pending [`CoreEvent::PermissionRequested`] prompt.
///
/// Serialized `rename_all = "camelCase"` because it crosses the Tauri IPC
/// boundary as the argument of the `resolve_permission` command (mirrored in
/// `frontend/src/types/index.ts` as `PermissionDecision`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Decision {
    /// Whether the invocation is permitted to proceed.
    pub allow: bool,
    /// Whether to remember this answer for the rest of the session so the same
    /// `(server, tool)` is not prompted again (Section 5.6 "remembered per tool
    /// for the session"). Purely advisory to the caller; the gate itself does
    /// not persist anything.
    #[serde(default)]
    pub remember: bool,
}

impl Decision {
    /// A one-shot allow decision.
    pub fn allow() -> Self {
        Decision {
            allow: true,
            remember: false,
        }
    }

    /// A one-shot deny decision.
    pub fn deny() -> Self {
        Decision {
            allow: false,
            remember: false,
        }
    }
}

/// The resolved outcome of a permission check, returned by [`PermissionGate`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionOutcome {
    /// The invocation may proceed.
    Allow,
    /// The invocation is refused; `reason` is a display-safe message the bridge
    /// folds into a structured tool-error result.
    Deny { reason: String },
}

/// Registry of in-flight `Ask`-mode permission requests.
///
/// Each pending request is keyed by its `request_id` and holds the sender half
/// of a `oneshot` channel; [`resolve`](Self::resolve) delivers the [`Decision`]
/// and unblocks the awaiting invocation. Framework-agnostic: the tauri-app
/// `resolve_permission` command forwards to [`resolve`](Self::resolve).
#[derive(Clone, Default)]
pub struct PermissionRegistry {
    pending: Arc<Mutex<HashMap<Uuid, oneshot::Sender<Decision>>>>,
}

impl PermissionRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        PermissionRegistry {
            pending: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Register a fresh pending request and return the receiver the caller will
    /// await for the user's [`Decision`].
    fn register(&self, request_id: Uuid) -> oneshot::Receiver<Decision> {
        let (tx, rx) = oneshot::channel();
        self.pending
            .lock()
            .expect("permission registry poisoned")
            .insert(request_id, tx);
        rx
    }

    /// Drop a still-pending request (used when the prompt could not be emitted).
    fn cancel(&self, request_id: &Uuid) {
        self.pending
            .lock()
            .expect("permission registry poisoned")
            .remove(request_id);
    }

    /// Resolve a pending permission request with the user's `decision`.
    ///
    /// Returns `true` if a request with `request_id` was awaiting a decision (it
    /// is now unblocked), `false` if no such request exists (already resolved,
    /// timed out, or unknown id). Wired to the tauri-app `resolve_permission`
    /// command.
    pub fn resolve(&self, request_id: Uuid, decision: Decision) -> bool {
        let sender = self
            .pending
            .lock()
            .expect("permission registry poisoned")
            .remove(&request_id);
        match sender {
            // `send` fails only if the awaiting side was dropped; treat that as
            // "no longer awaiting".
            Some(tx) => tx.send(decision).is_ok(),
            None => false,
        }
    }
}

/// The permission gate consulted before each tool invocation (Section 5.6).
///
/// Holds the core event sender (to emit `Ask`-mode prompts) and the shared
/// [`PermissionRegistry`] (to await their resolution). It is `Clone` and cheap
/// to share across tasks.
#[derive(Clone)]
pub struct PermissionGate {
    events: UnboundedSender<CoreEvent>,
    registry: PermissionRegistry,
}

impl PermissionGate {
    /// Build a gate over the core event channel and a permission registry.
    pub fn new(events: UnboundedSender<CoreEvent>, registry: PermissionRegistry) -> Self {
        PermissionGate { events, registry }
    }

    /// The registry this gate resolves against (so the shell can wire the
    /// `resolve_permission` command to the same instance).
    pub fn registry(&self) -> &PermissionRegistry {
        &self.registry
    }

    /// Decide whether a pending invocation of `tool_name` on `server_id` may
    /// proceed, given the server's `mode` and any per-tool `overrides`.
    ///
    /// - `Allow` returns [`PermissionOutcome::Allow`] immediately.
    /// - `Deny` returns [`PermissionOutcome::Deny`] immediately.
    /// - `Ask` emits a [`CoreEvent::PermissionRequested`] and awaits the
    ///   frontend's [`Decision`] via the registry, mapping allow/deny onto the
    ///   outcome. If the event channel is closed (no shell listening) the
    ///   request fails closed as a deny so a tool can never run unprompted.
    ///
    /// `overrides` maps a BARE (server-local) tool name to a mode that takes
    /// precedence over the server default (Section 5.6 "optional per-tool
    /// overrides").
    pub async fn check(
        &self,
        server_id: Uuid,
        tool_name: &str,
        mode: PermissionMode,
        overrides: &HashMap<String, PermissionMode>,
        rationale: &str,
    ) -> PermissionOutcome {
        let effective = overrides.get(tool_name).copied().unwrap_or(mode);
        match effective {
            PermissionMode::Allow => PermissionOutcome::Allow,
            PermissionMode::Deny => PermissionOutcome::Deny {
                reason: format!("permission denied: server policy denies tool `{tool_name}`"),
            },
            // Pass the EFFECTIVE mode (after per-tool overrides), not the server
            // default, so the emitted `PermissionRequested.mode` reflects what
            // actually applies to this tool. When no override is present
            // `effective == mode`, so this is a no-op today; it matters once
            // per-tool overrides are wired.
            PermissionMode::Ask => {
                self.prompt(server_id, tool_name, effective, rationale)
                    .await
            }
        }
    }

    /// Emit the `Ask`-mode prompt and block on the user's decision.
    async fn prompt(
        &self,
        server_id: Uuid,
        tool_name: &str,
        mode: PermissionMode,
        rationale: &str,
    ) -> PermissionOutcome {
        let request_id = Uuid::new_v4();
        let rx = self.registry.register(request_id);

        let event = CoreEvent::PermissionRequested {
            request_id,
            server_id,
            tool_name: tool_name.to_string(),
            mode,
            rationale: rationale.to_string(),
        };
        if self.events.send(event).is_err() {
            // No shell is listening; fail closed so the tool never runs without
            // an explicit approval.
            self.registry.cancel(&request_id);
            return PermissionOutcome::Deny {
                reason: format!("permission denied: no UI available to approve tool `{tool_name}`"),
            };
        }

        match rx.await {
            Ok(decision) if decision.allow => PermissionOutcome::Allow,
            Ok(_) => PermissionOutcome::Deny {
                reason: format!("permission denied: user declined tool `{tool_name}`"),
            },
            // The sender was dropped without a decision (registry cleared);
            // fail closed.
            Err(_) => PermissionOutcome::Deny {
                reason: format!("permission denied: request for tool `{tool_name}` was cancelled"),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc::unbounded_channel;

    fn gate() -> (
        PermissionGate,
        tokio::sync::mpsc::UnboundedReceiver<CoreEvent>,
    ) {
        let (tx, rx) = unbounded_channel();
        let gate = PermissionGate::new(tx, PermissionRegistry::new());
        (gate, rx)
    }

    #[tokio::test]
    async fn allow_mode_permits_immediately() {
        let (gate, _rx) = gate();
        let outcome = gate
            .check(
                Uuid::nil(),
                "read",
                PermissionMode::Allow,
                &HashMap::new(),
                "read a file",
            )
            .await;
        assert_eq!(outcome, PermissionOutcome::Allow);
    }

    #[tokio::test]
    async fn deny_mode_refuses_immediately() {
        let (gate, _rx) = gate();
        let outcome = gate
            .check(
                Uuid::nil(),
                "rm",
                PermissionMode::Deny,
                &HashMap::new(),
                "delete everything",
            )
            .await;
        assert!(matches!(outcome, PermissionOutcome::Deny { .. }));
    }

    #[tokio::test]
    async fn per_tool_override_wins_over_server_mode() {
        let (gate, _rx) = gate();
        let mut overrides = HashMap::new();
        overrides.insert("rm".to_string(), PermissionMode::Deny);
        // Server default is Allow, but the per-tool override denies `rm`.
        let outcome = gate
            .check(
                Uuid::nil(),
                "rm",
                PermissionMode::Allow,
                &overrides,
                "delete",
            )
            .await;
        assert!(matches!(outcome, PermissionOutcome::Deny { .. }));
    }

    #[tokio::test]
    async fn ask_mode_emits_event_and_unblocks_on_resolve() {
        let (gate, mut rx) = gate();
        let registry = gate.registry().clone();
        let server_id = Uuid::new_v4();

        // Run the (blocking) check on a task; it should emit an event and wait.
        let check = tokio::spawn(async move {
            gate.check(
                server_id,
                "read",
                PermissionMode::Ask,
                &HashMap::new(),
                "read a file",
            )
            .await
        });

        // The prompt event is emitted with a fresh request id.
        let event = rx.recv().await.expect("expected a permission event");
        let request_id = match event {
            CoreEvent::PermissionRequested {
                request_id,
                server_id: ev_server,
                ref tool_name,
                ..
            } => {
                assert_eq!(ev_server, server_id);
                assert_eq!(tool_name, "read");
                request_id
            }
            other => panic!("unexpected event: {other:?}"),
        };

        // Resolving with allow unblocks the awaiting check.
        assert!(registry.resolve(request_id, Decision::allow()));
        assert_eq!(check.await.unwrap(), PermissionOutcome::Allow);
    }

    #[tokio::test]
    async fn ask_mode_deny_decision_refuses() {
        let (gate, mut rx) = gate();
        let registry = gate.registry().clone();

        let check = tokio::spawn(async move {
            gate.check(
                Uuid::new_v4(),
                "write",
                PermissionMode::Ask,
                &HashMap::new(),
                "write a file",
            )
            .await
        });

        let request_id = match rx.recv().await.unwrap() {
            CoreEvent::PermissionRequested { request_id, .. } => request_id,
            other => panic!("unexpected event: {other:?}"),
        };
        assert!(registry.resolve(request_id, Decision::deny()));
        assert!(matches!(
            check.await.unwrap(),
            PermissionOutcome::Deny { .. }
        ));
    }

    #[test]
    fn resolve_unknown_request_returns_false() {
        let registry = PermissionRegistry::new();
        assert!(!registry.resolve(Uuid::new_v4(), Decision::allow()));
    }

    #[test]
    fn decision_deserializes_camel_case_and_defaults_remember() {
        let d: Decision = serde_json::from_str(r#"{"allow":true}"#).unwrap();
        assert!(d.allow);
        assert!(!d.remember);
        let d: Decision = serde_json::from_str(r#"{"allow":false,"remember":true}"#).unwrap();
        assert!(!d.allow);
        assert!(d.remember);
    }
}
