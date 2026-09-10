//! The end-to-end message pipeline (architecture.md Section 2.2, P4.6):
//! `message -> route -> provider -> tools -> persist`.
//!
//! [`run_turn`] assembles the whole turn for one user message. It is
//! framework-agnostic (no Tauri): it takes the [`SessionManager`], the routing
//! [`PolicyRegistry`], the [`ProviderRegistry`], the connected MCP server
//! handles for the conversation, a [`PermissionGate`], and the [`CoreEvent`]
//! sender, plus the target conversation id, the new user message content, and an
//! optional per-message [`ManualRoute`] override. It streams the answer over the
//! event channel and persists the assistant message when done.
//!
//! ## Flow (Section 2.2)
//!
//! 1. Persist the incoming user [`Message`], then load the
//!    [`crate::models::Conversation`] (privacy tags, conversation pin, persona)
//!    and its persona if any.
//! 2. Build the provider-shaped [`ChatMessage`] history from the conversation's
//!    persisted messages, prepending the persona `system_prompt` as a leading
//!    `System` message when present.
//! 3. Build a [`RoutingRequest`] from the conversation, persona, the per-message
//!    override, the supplied candidate models, and the provably-local provider
//!    id set (so a `LocalOnly`/`Confidential` tag is enforced on provable
//!    locality, never a zero price), then resolve a
//!    [`routing::RoutingDecision`] via the [`PolicyRegistry`]. A
//!    [`routing::RoutingError`] emits [`CoreEvent::MessageError`] and persists an
//!    `Error`-status message.
//! 4. Resolve the adapter from the [`ProviderRegistry`] by
//!    `decision.provider_id`. A missing adapter fails the same way.
//! 5. Build the [`ChatRequest`] (model, messages, persona temperature/max
//!    tokens, `stream: true`). Attach MCP tools ONLY when the chosen model's
//!    [`providers::Capabilities::tools`] is set (capability negotiation, Section
//!    4.4) and at least one server is connected.
//! 6. Run the Phase 3 tool loop: stream the response (falling back to a single
//!    non-streaming call when the model cannot stream), emit
//!    [`CoreEvent::MessageDelta`] per text chunk, and, when the turn ends with
//!    tool calls, invoke each via [`ToolBridge::handle_tool_call`] (permission
//!    gating happens inside the gate), feed the assistant + tool messages back,
//!    and continue. The loop is capped at [`MAX_TOOL_LOOP_ITERATIONS`] turns.
//! 7. On completion, persist the assistant [`Message`] (`Complete`, with the
//!    [`RouteMetadata`] and [`TokenUsage`]) and emit
//!    [`CoreEvent::MessageComplete`].
//!
//! Any failure along the way emits [`CoreEvent::MessageError`] and persists an
//! `Error`-status assistant message so the conversation record is consistent.

use std::collections::BTreeSet;
use std::sync::Arc;

use chrono::Utc;
use futures_util::StreamExt;
use uuid::Uuid;

use providers::{
    AvailableModel, ChatMessage, ChatRequest, ChatResponse, MessageRole, ProviderRegistry,
    ToolCall as ProviderToolCall, Usage,
};
use routing::{PolicyRegistry, RoutingRequest};

use crate::events::CoreEvent;
use crate::models::{
    ManualRoute, Message, MessageContent, MessageStatus, Role, RouteMetadata, TokenUsage,
};
use crate::permission::PermissionGate;
use crate::session::{SessionError, SessionManager};
use crate::tools_bridge::{effective_tool_servers, ToolBridge};

use mcp_client::McpServerHandle;
use tokio::sync::mpsc::UnboundedSender;

/// Upper bound on provider<->tool round trips within a single turn. A model that
/// keeps requesting tools past this many turns is treated as stuck; the turn
/// fails with [`CoreEvent::MessageError`] rather than looping forever
/// (architecture.md Section 5.5 "bounded tool loop").
pub const MAX_TOOL_LOOP_ITERATIONS: usize = 8;

/// Everything the pipeline needs to run one turn. Assembled by the caller (the
/// tauri-app `send_message` command or the integration test) from its own
/// subsystems so the pipeline stays framework-agnostic.
///
/// It intentionally does NOT derive `Debug`: it holds a [`ProviderRegistry`]
/// (whose instances are `Arc<dyn ChatProvider>` trait objects that are not
/// `Debug`) and MCP handles, none of which are display-safe to format. Callers
/// that need to log build the display-safe pieces (ids, the resolved route)
/// themselves.
pub struct TurnContext<'a> {
    /// The session orchestration seam (loads + persists messages/conversations).
    pub session_manager: &'a SessionManager,
    /// The routing policy registry (Section 6.4) whose active policy decides.
    pub policies: &'a PolicyRegistry,
    /// The live provider instances (Section 4.5) the decision resolves against.
    pub providers: &'a ProviderRegistry,
    /// Connected MCP server handles for this conversation. May be EMPTY: with no
    /// connected servers the pipeline attaches no tools and runs a plain turn.
    pub servers: Vec<Arc<McpServerHandle>>,
    /// The permission gate the tool bridge consults before each invocation
    /// (Ask/Allow/Deny, Section 5.6). Shares the [`crate::PermissionRegistry`]
    /// the shell resolves prompts against.
    pub gate: PermissionGate,
    /// The core-event sender streamed to the frontend (deltas, completion,
    /// errors). Cloned as needed.
    pub events: UnboundedSender<CoreEvent>,
    /// The candidate provider/model list routing chooses among (Section 6.1).
    /// The caller supplies this (e.g. from `providers::list_available_models`),
    /// keeping the network-touching enumeration out of the pipeline.
    pub available: Vec<AvailableModel>,
    /// The provider instance ids that are PROVABLY local (architecture.md
    /// Section 6.2). The caller derives this from each configured provider's
    /// concrete `ProviderKind` (LM Studio, and a loopback GenericOpenAI
    /// endpoint) so routing can enforce the privacy hard constraint on provable
    /// locality rather than trusting a zero price. May be EMPTY, in which case
    /// no candidate is provably local and any `LocalOnly`/`Confidential` tag
    /// fails closed.
    pub local_provider_ids: BTreeSet<String>,
}

/// Failure of an assembled turn. Every variant is display-safe (Section 9.1);
/// the pipeline also emits [`CoreEvent::MessageError`] and persists an
/// `Error`-status assistant message for each before returning.
#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    /// The target conversation does not exist.
    #[error("conversation not found: {0}")]
    ConversationNotFound(Uuid),
    /// Routing could not produce a decision (privacy/capability/manual/no
    /// candidate).
    #[error("routing failed: {0}")]
    Routing(String),
    /// The routed provider id has no built instance in the registry.
    #[error("no provider instance for {0:?}")]
    ProviderUnavailable(String),
    /// The provider adapter returned an error while generating.
    #[error("provider error: {0}")]
    Provider(String),
    /// The tool loop exceeded [`MAX_TOOL_LOOP_ITERATIONS`] without finishing.
    #[error("tool loop exceeded {0} iterations")]
    ToolLoopExceeded(usize),
    /// A persistence operation failed.
    #[error(transparent)]
    Session(#[from] SessionError),
}

/// The accumulated result of one streamed provider turn: the assistant text so
/// far, any tool calls it requested, why it stopped, and the token usage.
#[derive(Debug, Default)]
struct TurnOutcome {
    content: String,
    tool_calls: Vec<ProviderToolCall>,
    usage: Option<Usage>,
}

/// Run one full turn for `content` in `conversation_id` (architecture.md Section
/// 2.2). Returns the persisted assistant [`Message`] on success.
///
/// On any failure this emits [`CoreEvent::MessageError`] and persists an
/// `Error`-status assistant message BEFORE returning the [`PipelineError`], so
/// callers can surface the error without a second persistence step.
pub async fn run_turn(
    ctx: &TurnContext<'_>,
    conversation_id: Uuid,
    content: String,
    manual_override: Option<ManualRoute>,
) -> Result<Message, PipelineError> {
    // The assistant message id is allocated up front so streamed deltas and the
    // final completion all reference the same id.
    let assistant_id = Uuid::new_v4();
    match run_turn_inner(ctx, conversation_id, content, manual_override, assistant_id).await {
        Ok(message) => Ok(message),
        Err(err) => {
            // Emit a display-safe error and persist an Error-status message so
            // the conversation record reflects the failed turn.
            let _ = ctx.events.send(CoreEvent::MessageError {
                conversation_id,
                message_id: assistant_id,
                message: err.to_string(),
            });
            // Best-effort: persist the error marker. A persistence failure here
            // is swallowed so the original error is the one returned.
            let _ = ctx
                .session_manager
                .append_message(error_message(
                    assistant_id,
                    conversation_id,
                    &err.to_string(),
                ))
                .await;
            Err(err)
        }
    }
}

/// The fallible body of [`run_turn`]; [`run_turn`] wraps it to centralize the
/// error-event + error-message handling.
async fn run_turn_inner(
    ctx: &TurnContext<'_>,
    conversation_id: Uuid,
    content: String,
    manual_override: Option<ManualRoute>,
    assistant_id: Uuid,
) -> Result<Message, PipelineError> {
    // 1. Persist the user message, then load the conversation + persona.
    //    Announce the user message (MessageStarted) so the active chat surface
    //    seeds a placeholder before any streaming begins, and emit
    //    ConversationUpdated so the history surface's recency/ordering refreshes
    //    once the message is persisted (Section 8.5).
    let user_message = user_message(conversation_id, &content);
    let user_message_id = user_message.id;
    // Announce the user message WITH its text: the full content is already known
    // (no streaming for the user turn), so the chat surface can render the
    // user's own words immediately on send rather than seeding an empty bubble
    // that no delta ever fills.
    let _ = ctx.events.send(CoreEvent::MessageStarted {
        conversation_id,
        message_id: user_message_id,
        role: Role::User,
        text: Some(content.clone()),
    });
    ctx.session_manager.append_message(user_message).await?;
    let _ = ctx
        .events
        .send(CoreEvent::ConversationUpdated { conversation_id });

    // Announce the assistant reply as a streaming placeholder so the deltas
    // below (which reference `assistant_id`) land on a message the frontend has
    // already seeded.
    let _ = ctx.events.send(CoreEvent::MessageStarted {
        conversation_id,
        message_id: assistant_id,
        role: Role::Assistant,
        // The assistant reply has no text yet: it streams via MessageDelta.
        text: None,
    });

    let conversation = ctx
        .session_manager
        .get_conversation(conversation_id)
        .await?
        .ok_or(PipelineError::ConversationNotFound(conversation_id))?;

    let persona = match conversation.persona_id {
        Some(persona_id) => ctx
            .session_manager
            .personas()
            .get(persona_id)
            .await
            .map_err(SessionError::from)?,
        None => None,
    };

    // 2. Build the provider-shaped message history from the persisted messages,
    //    prepending the persona system prompt when present.
    let persisted = ctx.session_manager.list_messages(conversation_id).await?;
    let mut messages: Vec<ChatMessage> = Vec::with_capacity(persisted.len() + 1);
    if let Some(persona) = &persona {
        if !persona.system_prompt.trim().is_empty() {
            messages.push(ChatMessage::text(
                MessageRole::System,
                persona.system_prompt.clone(),
            ));
        }
    }
    for message in &persisted {
        if let Some(chat) = to_chat_message(message) {
            messages.push(chat);
        }
    }

    // 3. Build the routing request and resolve a decision.
    //    The conversation-level effective hint comes from the per-conversation
    //    routing mode and takes precedence over the persona hint inside the auto
    //    policy. Manual mode (and Auto) map to `None` here, so the manual
    //    pin/override precedence is handled exactly as before via
    //    `conversation_pref` / `manual_override`; the mode contributes no
    //    automatic bias in those cases.
    let conversation_hint = conversation.routing_mode.and_then(|m| m.effective_hint());
    let request = RoutingRequest {
        messages: messages.clone(),
        privacy_tags: conversation.privacy_tags.clone(),
        persona: persona.clone(),
        routing_hint: conversation_hint,
        manual_override,
        conversation_pref: conversation.conversation_pref.clone(),
        available: ctx.available.clone(),
        local_provider_ids: ctx.local_provider_ids.clone(),
        // DEFERRED (no schema yet): a `CostBudget` needs a persisted source (a
        // per-conversation or per-period budget on `Conversation`/`AppConfig`),
        // which does not exist in the Phase 4 data model. The cost SIGNAL is
        // fully wired and unit-tested in the routing crate (`estimate_cost` /
        // `within_budget` / the out-of-budget penalty), so it activates the
        // moment a budget source is threaded here; until then a budget-less turn
        // is the correct behavior (an absent budget is a valid "no budget"
        // signal). Populating a real budget is tracked with persona/config
        // budgets (a later phase), not Phase 4.
        budget: None,
    };
    let decision = ctx
        .policies
        .resolve(&request)
        .await
        .map_err(|e| PipelineError::Routing(e.to_string()))?;

    // 4. Resolve the provider adapter for the chosen provider id.
    let provider = ctx
        .providers
        .get(&decision.provider_id)
        .ok_or_else(|| PipelineError::ProviderUnavailable(decision.provider_id.clone()))?;

    // 5. Build the chat request; attach tools only when the model supports them
    //    AND servers are connected (capability negotiation, Section 4.4).
    let capabilities = provider.capabilities(&decision.model);
    let mut chat_request = ChatRequest::new(decision.model.clone(), messages);
    chat_request.stream = capabilities.streaming;
    if let Some(params) = persona.as_ref().map(|p| &p.parameters) {
        chat_request.temperature = params.temperature;
        chat_request.max_tokens = params.max_tokens;
    }

    // Tool gating (Sections 5.6 / 9.4): expose ONLY tools from servers that pass
    // BOTH the conversation gate and the active persona gate, as computed by
    // `effective_tool_servers` over the connected handles. Each gate restricts
    // only when its allow-list is non-empty; an empty gate imposes no
    // restriction.
    //
    // This enforces the persona `allowed_tool_servers` restriction even when the
    // conversation's `enabled_tool_servers` is empty (the current default, since
    // the creation flow does not populate it): a persona that allows only server
    // A exposes only A regardless of whether the conversation opted in. Genuine
    // backward compatibility is preserved for the no-gate case: a conversation
    // with an empty list and no persona restriction still exposes every
    // connected server.
    let connected_ids: Vec<Uuid> = ctx.servers.iter().map(|h| h.config().id).collect();
    let allowed_servers = effective_tool_servers(&connected_ids, &conversation, persona.as_ref());
    let gated_servers: Vec<Arc<McpServerHandle>> = ctx
        .servers
        .iter()
        .filter(|h| allowed_servers.contains(&h.config().id))
        .cloned()
        .collect();

    let bridge = if !gated_servers.is_empty() && capabilities.tools {
        let bridge = ToolBridge::new(gated_servers, ctx.gate.clone(), ctx.events.clone());
        bridge.attach_tools(&mut chat_request).await;
        Some(bridge)
    } else {
        None
    };

    // 6. Run the bounded tool loop.
    let mut last_usage: Option<Usage> = None;
    let mut final_text = String::new();
    let mut completed = false;

    for _ in 0..MAX_TOOL_LOOP_ITERATIONS {
        let outcome = stream_turn(
            provider.as_ref(),
            chat_request.clone(),
            capabilities.streaming,
            conversation_id,
            assistant_id,
            &ctx.events,
        )
        .await
        .map_err(PipelineError::Provider)?;

        last_usage = outcome.usage.or(last_usage);

        // The turn requested tools when it accumulated any tool calls (a
        // `ToolCalls` finish reason with no accumulated calls is treated as
        // final rather than looping on nothing).
        if !outcome.tool_calls.is_empty() {
            let Some(bridge) = bridge.as_ref() else {
                // The model asked for tools but none are attachable (no bridge):
                // treat the text so far as final rather than looping.
                final_text = outcome.content;
                completed = true;
                break;
            };
            // Push the assistant tool-call message, then each tool result, and
            // continue the loop for another provider turn.
            chat_request
                .messages
                .push(assistant_tool_call_message(&outcome));
            for call in &outcome.tool_calls {
                let tool_message = bridge.handle_tool_call(call).await;
                chat_request.messages.push(tool_message);
            }
            continue;
        }

        // No tool calls: this is the final assistant message.
        final_text = outcome.content;
        completed = true;
        break;
    }

    if !completed {
        return Err(PipelineError::ToolLoopExceeded(MAX_TOOL_LOOP_ITERATIONS));
    }

    // 7. Persist the completed assistant message + route + usage, then emit
    //    MessageComplete.
    let route = decision.into_route_metadata();
    let usage = last_usage.map(to_token_usage);
    let message = complete_message(
        assistant_id,
        conversation_id,
        &final_text,
        route.clone(),
        usage,
    );
    let persisted_message = ctx.session_manager.append_message(message).await?;

    let _ = ctx.events.send(CoreEvent::MessageComplete {
        conversation_id,
        message_id: assistant_id,
        status: MessageStatus::Complete,
        route: Some(route),
        usage,
    });
    // The assistant message is persisted: refresh the history surface's recency
    // and any non-active conversation view (Section 8.5).
    let _ = ctx
        .events
        .send(CoreEvent::ConversationUpdated { conversation_id });

    Ok(persisted_message)
}

/// Drive one provider turn to completion, emitting a [`CoreEvent::MessageDelta`]
/// for each streamed text chunk and accumulating the assistant text, tool
/// calls, finish reason, and usage.
///
/// When the model cannot stream (`!streaming`), a single non-streaming
/// [`providers::ChatProvider::chat`] call is made and its one choice is
/// synthesized into a single delta so the frontend still sees streamed text.
async fn stream_turn(
    provider: &dyn providers::ChatProvider,
    request: ChatRequest,
    streaming: bool,
    conversation_id: Uuid,
    assistant_id: Uuid,
    events: &UnboundedSender<CoreEvent>,
) -> Result<TurnOutcome, String> {
    if !streaming {
        let response = provider.chat(request).await.map_err(|e| e.to_string())?;
        return Ok(outcome_from_response(
            response,
            conversation_id,
            assistant_id,
            events,
        ));
    }

    let mut stream = provider
        .chat_stream(request)
        .await
        .map_err(|e| e.to_string())?;

    let mut outcome = TurnOutcome::default();
    // Tool-call fragments arrive per-index and must be concatenated.
    let mut tool_builders: Vec<ToolCallBuilder> = Vec::new();

    while let Some(item) = stream.next().await {
        let delta = item.map_err(|e| e.to_string())?;
        if let Some(text) = delta.content {
            if !text.is_empty() {
                outcome.content.push_str(&text);
                let _ = events.send(CoreEvent::MessageDelta {
                    conversation_id,
                    message_id: assistant_id,
                    delta: text,
                });
            }
        }
        for fragment in delta.tool_calls {
            let index = fragment.index as usize;
            if tool_builders.len() <= index {
                tool_builders.resize_with(index + 1, ToolCallBuilder::default);
            }
            let builder = &mut tool_builders[index];
            if let Some(id) = fragment.id {
                builder.id = id;
            }
            if let Some(name) = fragment.function_name {
                builder.name = name;
            }
            if let Some(args) = fragment.arguments_fragment {
                builder.arguments.push_str(&args);
            }
        }
        // The finish reason is not needed to decide the loop (an accumulated
        // tool call is the signal); it is intentionally not tracked.
    }

    outcome.tool_calls = tool_builders
        .into_iter()
        .filter(|b| !b.name.is_empty())
        .map(ToolCallBuilder::build)
        .collect();
    Ok(outcome)
}

/// Accumulator for a streamed tool call whose fields arrive as fragments.
#[derive(Debug, Default)]
struct ToolCallBuilder {
    id: String,
    name: String,
    arguments: String,
}

impl ToolCallBuilder {
    fn build(self) -> ProviderToolCall {
        ProviderToolCall {
            id: self.id,
            kind: "function".to_string(),
            function: providers::FunctionCall {
                name: self.name,
                arguments: self.arguments,
            },
        }
    }
}

/// Fold a non-streaming [`ChatResponse`] into a [`TurnOutcome`], emitting the
/// assistant text as a single delta so a non-streaming model still produces a
/// [`CoreEvent::MessageDelta`].
fn outcome_from_response(
    response: ChatResponse,
    conversation_id: Uuid,
    assistant_id: Uuid,
    events: &UnboundedSender<CoreEvent>,
) -> TurnOutcome {
    let mut outcome = TurnOutcome {
        usage: response.usage,
        ..TurnOutcome::default()
    };
    if let Some(choice) = response.choices.into_iter().next() {
        outcome.tool_calls = choice.message.tool_calls;
        if let Some(text) = choice.message.content {
            if !text.is_empty() {
                outcome.content.push_str(&text);
                let _ = events.send(CoreEvent::MessageDelta {
                    conversation_id,
                    message_id: assistant_id,
                    delta: text,
                });
            }
        }
    }
    outcome
}

/// Build the assistant message carrying the requested tool calls, to feed back
/// into `request.messages` before the tool results (OpenAI tool-loop shape).
fn assistant_tool_call_message(outcome: &TurnOutcome) -> ChatMessage {
    ChatMessage {
        role: MessageRole::Assistant,
        content: if outcome.content.is_empty() {
            None
        } else {
            Some(outcome.content.clone())
        },
        tool_calls: outcome.tool_calls.clone(),
        tool_call_id: None,
        name: None,
    }
}

/// Map a persisted domain [`Message`] into a provider [`ChatMessage`]. Only
/// text content is threaded into the provider history; tool-call/tool-result/
/// attachment messages are represented via the live tool loop, not replayed
/// from history, so they are skipped here (returns `None`).
fn to_chat_message(message: &Message) -> Option<ChatMessage> {
    let MessageContent::Text { text } = &message.content else {
        return None;
    };
    let role = match message.role {
        Role::System => MessageRole::System,
        Role::User => MessageRole::User,
        Role::Assistant => MessageRole::Assistant,
        Role::Tool => MessageRole::Tool,
    };
    Some(ChatMessage::text(role, text.clone()))
}

/// Build a fresh `User` [`Message`] for the incoming content.
fn user_message(conversation_id: Uuid, text: &str) -> Message {
    Message {
        id: Uuid::new_v4(),
        conversation_id,
        role: Role::User,
        content: MessageContent::Text {
            text: text.to_string(),
        },
        created_at: Utc::now(),
        route: None,
        usage: None,
        status: MessageStatus::Complete,
    }
}

/// Build the completed assistant [`Message`] with its route + usage.
fn complete_message(
    id: Uuid,
    conversation_id: Uuid,
    text: &str,
    route: RouteMetadata,
    usage: Option<TokenUsage>,
) -> Message {
    Message {
        id,
        conversation_id,
        role: Role::Assistant,
        content: MessageContent::Text {
            text: text.to_string(),
        },
        created_at: Utc::now(),
        route: Some(route),
        usage,
        status: MessageStatus::Complete,
    }
}

/// Build an `Error`-status assistant [`Message`] recording a failed turn. The
/// error string is display-safe (Section 9.1).
fn error_message(id: Uuid, conversation_id: Uuid, error: &str) -> Message {
    Message {
        id,
        conversation_id,
        role: Role::Assistant,
        content: MessageContent::Text {
            text: error.to_string(),
        },
        created_at: Utc::now(),
        route: None,
        usage: None,
        status: MessageStatus::Error,
    }
}

/// Convert a provider [`Usage`] into the persisted [`TokenUsage`] (identical
/// shape; this is the single conversion point).
fn to_token_usage(usage: Usage) -> TokenUsage {
    TokenUsage {
        prompt_tokens: usage.prompt_tokens,
        completion_tokens: usage.completion_tokens,
        total_tokens: usage.total_tokens,
    }
}
