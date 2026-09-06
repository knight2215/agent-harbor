//! End-to-end integration test for the FEAT-002 message pipeline (architecture.md
//! Section 2.2, P4.6): `message -> route -> provider -> tool loop -> persist`.
//!
//! It drives [`orchestrator_core::run_turn`] with NO live network:
//!
//!   1. an in-memory [`SessionManager`] over `Db::open_in_memory`;
//!   2. a routing [`PolicyRegistry`] plus an `available` candidate list whose
//!      single row names the mock provider, so the automatic policy selects it
//!      (and a manual-override variant makes the decision `Manual`);
//!   3. a scripted mock [`ChatProvider`] (registered on a `ProviderRegistry` via
//!      `insert_instance`) whose first `chat` returns a tool call for the
//!      namespaced fixture `echo` tool and whose second returns final text; and
//!   4. the bundled `tool-loop-mock-server` `[[bin]]` connected over stdio
//!      (spawned via `env!("CARGO_BIN_EXE_...")`, exactly like `tool_loop.rs`).
//!
//! It asserts: `MessageDelta` events are emitted; the tool loop invokes the
//! fixture tool under `PermissionMode::Allow` (permission gating exercised in
//! the assembled pipeline); the final assistant message is PERSISTED with status
//! `Complete`; and its route metadata carries the expected provider/model, a
//! non-empty rationale, and the correct `RouteSource`. A final variant asserts
//! an `Ask`-mode prompt BLOCKS then UNBLOCKS via `PermissionRegistry::resolve`
//! within the assembled pipeline.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures_util::stream::{self, BoxStream};

use mcp_client::McpServerHandle;
use orchestrator_core::{
    run_turn, ConversationInit, CoreEvent, Decision, MessageContent, MessageStatus, PermissionGate,
    PermissionRegistry, PipelineError, PrivacyTag, Role, RouteSource, SessionManager, TurnContext,
};
use persistence::Db;
use providers::{
    AvailableModel, Capabilities, ChatChoice, ChatDelta, ChatMessage, ChatProvider, ChatRequest,
    ChatResponse, FinishReason, MessageRole, ModelInfo, ProviderError, ProviderRegistry,
    TokenPrice, Usage,
};

use domain::{McpServerConfig, McpTransport, PermissionMode};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use uuid::Uuid;

/// Path to the bundled mock MCP server binary (resolved by Cargo, `.exe` on
/// Windows).
const MOCK_SERVER: &str = env!("CARGO_BIN_EXE_tool-loop-mock-server");

/// The provider id the mock is registered under and that the routing candidate
/// names, so the decision resolves to the mock.
const PROVIDER_ID: &str = "mock-provider";
const MODEL: &str = "mock-model";

/// A mock `ChatProvider` that replays a scripted queue of `chat` responses.
struct ScriptedProvider {
    responses: Mutex<VecDeque<ChatResponse>>,
}

impl ScriptedProvider {
    fn new(responses: Vec<ChatResponse>) -> Self {
        ScriptedProvider {
            responses: Mutex::new(responses.into_iter().collect()),
        }
    }
}

#[async_trait]
impl ChatProvider for ScriptedProvider {
    fn id(&self) -> &str {
        PROVIDER_ID
    }

    fn capabilities(&self, _model: &str) -> Capabilities {
        // Non-streaming so the pipeline uses the `chat` path (which the scripted
        // queue backs) and synthesizes a single delta per turn.
        Capabilities {
            tools: true,
            streaming: false,
            ..Capabilities::default()
        }
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        Ok(vec![ModelInfo::new(MODEL)])
    }

    async fn chat(&self, _req: ChatRequest) -> Result<ChatResponse, ProviderError> {
        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| ProviderError::Other("no scripted response left".to_string()))
    }

    async fn chat_stream(
        &self,
        _req: ChatRequest,
    ) -> Result<BoxStream<'static, Result<ChatDelta, ProviderError>>, ProviderError> {
        // Unused: `capabilities().streaming` is false, so the pipeline calls
        // `chat`.
        Ok(Box::pin(stream::empty()))
    }
}

/// Assistant response requesting a single tool call for `tool_name`.
fn tool_call_response(call_id: &str, tool_name: &str, arguments: &str) -> ChatResponse {
    ChatResponse {
        choices: vec![ChatChoice {
            index: 0,
            message: ChatMessage {
                role: MessageRole::Assistant,
                content: None,
                tool_calls: vec![providers::ToolCall {
                    id: call_id.to_string(),
                    kind: "function".to_string(),
                    function: providers::FunctionCall {
                        name: tool_name.to_string(),
                        arguments: arguments.to_string(),
                    },
                }],
                tool_call_id: None,
                name: None,
            },
            finish_reason: Some(FinishReason::ToolCalls),
        }],
        usage: None,
        model: Some(MODEL.to_string()),
    }
}

/// Final assistant text response, carrying token usage.
fn text_response(text: &str) -> ChatResponse {
    ChatResponse {
        choices: vec![ChatChoice {
            index: 0,
            message: ChatMessage::text(MessageRole::Assistant, text),
            finish_reason: Some(FinishReason::Stop),
        }],
        usage: Some(Usage {
            prompt_tokens: 11,
            completion_tokens: 7,
            total_tokens: 18,
        }),
        model: Some(MODEL.to_string()),
    }
}

fn stdio_config(id: Uuid, mode: PermissionMode) -> McpServerConfig {
    McpServerConfig {
        id,
        name: "loop-mock".to_string(),
        transport: McpTransport::Stdio {
            command: MOCK_SERVER.to_string(),
            args: vec![],
            env: vec![],
        },
        permission_mode: mode,
        enabled: true,
    }
}

/// The single candidate the routing policy chooses among: the mock provider's
/// tool-capable model, priced at zero so the automatic policy selects it.
fn available_models() -> Vec<AvailableModel> {
    vec![AvailableModel {
        provider_id: PROVIDER_ID.to_string(),
        model: MODEL.to_string(),
        capabilities: Capabilities {
            tools: true,
            streaming: false,
            ..Capabilities::default()
        },
        price: TokenPrice::ZERO,
    }]
}

/// Build a provider registry holding the scripted mock under `PROVIDER_ID`.
fn provider_registry(responses: Vec<ChatResponse>) -> ProviderRegistry {
    let mut registry = ProviderRegistry::new();
    registry.insert_instance(Arc::new(ScriptedProvider::new(responses)));
    registry
}

/// Build a connected handle to the fixture server in `mode`.
async fn connected_handle(id: Uuid, mode: PermissionMode) -> Arc<McpServerHandle> {
    let handle = Arc::new(McpServerHandle::new(stdio_config(id, mode)));
    handle.connect().await.expect("connect to mock server");
    handle
}

/// Drain all currently-buffered core events into a Vec.
fn drain(rx: &mut UnboundedReceiver<CoreEvent>) -> Vec<CoreEvent> {
    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }
    events
}

#[tokio::test]
async fn automatic_routing_runs_tool_loop_and_persists_route() {
    let session_manager = SessionManager::new(Db::open_in_memory().await.unwrap());
    let conversation = session_manager
        .create_conversation(ConversationInit::default())
        .await
        .unwrap();

    let (tx, mut rx): (UnboundedSender<CoreEvent>, UnboundedReceiver<CoreEvent>) =
        unbounded_channel();

    let server_id = Uuid::new_v4();
    let handle = connected_handle(server_id, PermissionMode::Allow).await;
    let namespaced = format!("{server_id}__echo");

    let providers = provider_registry(vec![
        tool_call_response("call-1", &namespaced, r#"{"text":"hello"}"#),
        text_response("The tool said hello."),
    ]);
    let policies = routing::PolicyRegistry::new();
    let gate = PermissionGate::new(tx.clone(), PermissionRegistry::new());

    let ctx = TurnContext {
        session_manager: &session_manager,
        policies: &policies,
        providers: &providers,
        servers: vec![handle],
        gate,
        events: tx.clone(),
        available: available_models(),
        // No privacy tags in this turn, so locality is not exercised; the mock
        // provider is nonetheless declared local for completeness.
        local_provider_ids: [PROVIDER_ID.to_string()].into_iter().collect(),
    };

    let message = run_turn(&ctx, conversation.id, "please echo hello".to_string(), None)
        .await
        .expect("pipeline turn succeeds");

    // The persisted assistant message is Complete with the routed provider/model.
    assert_eq!(message.status, MessageStatus::Complete);
    assert_eq!(message.role, Role::Assistant);
    let route = message.route.expect("route metadata persisted");
    assert_eq!(route.provider_id, PROVIDER_ID);
    assert_eq!(route.model, MODEL);
    assert!(!route.rationale.is_empty(), "rationale must be populated");
    assert_eq!(route.source, RouteSource::Automatic);
    // Token usage from the final provider turn was persisted.
    let usage = message.usage.expect("usage persisted");
    assert_eq!(usage.total_tokens, 18);

    // The persisted conversation now holds user + assistant messages.
    let persisted = session_manager
        .list_messages(conversation.id)
        .await
        .unwrap();
    assert!(persisted.iter().any(|m| m.role == Role::User));
    let assistant = persisted
        .iter()
        .find(|m| m.role == Role::Assistant)
        .expect("assistant message persisted");
    match &assistant.content {
        MessageContent::Text { text } => assert_eq!(text, "The tool said hello."),
        other => panic!("unexpected content: {other:?}"),
    }

    // A MessageDelta and a MessageComplete were emitted; the tool loop ran (the
    // second turn's text delta only exists because the tool call was handled).
    let events = drain(&mut rx);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, CoreEvent::MessageDelta { delta, .. } if delta == "The tool said hello.")),
        "final text delta emitted: {events:?}"
    );
    assert!(
        events.iter().any(|e| matches!(
            e,
            CoreEvent::MessageComplete {
                status: MessageStatus::Complete,
                ..
            }
        )),
        "completion emitted: {events:?}"
    );
}

#[tokio::test]
async fn manual_override_records_manual_source() {
    let session_manager = SessionManager::new(Db::open_in_memory().await.unwrap());
    let conversation = session_manager
        .create_conversation(ConversationInit::default())
        .await
        .unwrap();

    let (tx, _rx) = unbounded_channel();

    // No tools needed: the model answers directly.
    let providers = provider_registry(vec![text_response("Direct answer.")]);
    let policies = routing::PolicyRegistry::new();
    let gate = PermissionGate::new(tx.clone(), PermissionRegistry::new());

    let ctx = TurnContext {
        session_manager: &session_manager,
        policies: &policies,
        providers: &providers,
        servers: Vec::new(),
        gate,
        events: tx.clone(),
        available: available_models(),
        local_provider_ids: [PROVIDER_ID.to_string()].into_iter().collect(),
    };

    let override_route = orchestrator_core::ManualRoute {
        provider_id: PROVIDER_ID.to_string(),
        model: MODEL.to_string(),
    };
    let message = run_turn(
        &ctx,
        conversation.id,
        "hi".to_string(),
        Some(override_route),
    )
    .await
    .expect("pipeline turn succeeds");

    let route = message.route.expect("route metadata persisted");
    assert_eq!(route.source, RouteSource::Manual);
    assert_eq!(route.provider_id, PROVIDER_ID);
    assert_eq!(route.model, MODEL);
}

#[tokio::test]
async fn ask_mode_blocks_then_unblocks_within_pipeline() {
    let session_manager = SessionManager::new(Db::open_in_memory().await.unwrap());
    let conversation = session_manager
        .create_conversation(ConversationInit::default())
        .await
        .unwrap();

    let (tx, mut rx) = unbounded_channel();

    let server_id = Uuid::new_v4();
    let handle = connected_handle(server_id, PermissionMode::Ask).await;
    let namespaced = format!("{server_id}__echo");

    let providers = provider_registry(vec![
        tool_call_response("call-ask", &namespaced, r#"{"text":"hi"}"#),
        text_response("Done."),
    ]);
    let policies = routing::PolicyRegistry::new();
    let registry = PermissionRegistry::new();
    let gate = PermissionGate::new(tx.clone(), registry.clone());

    // Drive the turn on a task: the Ask-mode tool call BLOCKS until resolved.
    let session_clone = session_manager.clone();
    let providers = Arc::new(providers);
    let providers_task = Arc::clone(&providers);
    let policies_task = policies.clone();
    let tx_task = tx.clone();
    let conv_id = conversation.id;
    let handle_task = handle.clone();
    let available = available_models();
    let local_provider_ids = [PROVIDER_ID.to_string()].into_iter().collect();
    let task = tokio::spawn(async move {
        let ctx = TurnContext {
            session_manager: &session_clone,
            policies: &policies_task,
            providers: providers_task.as_ref(),
            servers: vec![handle_task],
            gate,
            events: tx_task,
            available,
            local_provider_ids,
        };
        run_turn(&ctx, conv_id, "please echo hi".to_string(), None).await
    });

    // The Ask-mode prompt is emitted; resolve it with allow to unblock the loop.
    let request_id = loop {
        match rx.recv().await.expect("permission event") {
            CoreEvent::PermissionRequested {
                request_id,
                server_id: ev_server,
                ref tool_name,
                ..
            } => {
                assert_eq!(ev_server, server_id);
                assert_eq!(tool_name, "echo");
                break request_id;
            }
            // Ignore any deltas that arrive before the prompt.
            _ => continue,
        }
    };
    assert!(registry.resolve(request_id, Decision::allow()));

    let message = task.await.unwrap().expect("pipeline turn succeeds");
    assert_eq!(message.status, MessageStatus::Complete);
    match &message.content {
        MessageContent::Text { text } => assert_eq!(text, "Done."),
        other => panic!("unexpected content: {other:?}"),
    }
}

#[tokio::test]
async fn routing_error_emits_message_error_and_persists_error_status() {
    // The failure side of the pipeline: a LocalOnly conversation whose only
    // candidate is NOT provably local makes routing FAIL CLOSED with
    // PrivacyConstraintUnsatisfiable. Assert the pipeline emits MessageError and
    // persists an Error-status assistant message (its own contribution, distinct
    // from the routing-crate unit tests that only cover the RoutingError itself).
    let session_manager = SessionManager::new(Db::open_in_memory().await.unwrap());
    let conversation = session_manager
        .create_conversation(ConversationInit {
            privacy_tags: vec![PrivacyTag::LocalOnly],
            ..ConversationInit::default()
        })
        .await
        .unwrap();

    let (tx, mut rx) = unbounded_channel();

    // The scripted provider is never reached: routing fails before any call.
    let providers = provider_registry(vec![text_response("unreachable")]);
    let policies = routing::PolicyRegistry::new();
    let gate = PermissionGate::new(tx.clone(), PermissionRegistry::new());

    // A single CLOUD candidate (nonzero price) and, crucially, an EMPTY
    // provably-local set: nothing can satisfy the LocalOnly tag.
    let available = vec![AvailableModel {
        provider_id: PROVIDER_ID.to_string(),
        model: MODEL.to_string(),
        capabilities: Capabilities {
            tools: false,
            streaming: false,
            ..Capabilities::default()
        },
        price: TokenPrice::new(2.5, 10.0),
    }];

    let ctx = TurnContext {
        session_manager: &session_manager,
        policies: &policies,
        providers: &providers,
        servers: Vec::new(),
        gate,
        events: tx.clone(),
        available,
        local_provider_ids: std::collections::BTreeSet::new(),
    };

    let err = run_turn(&ctx, conversation.id, "secret data".to_string(), None)
        .await
        .expect_err("routing must fail closed under LocalOnly with no local model");
    assert!(matches!(err, PipelineError::Routing(_)));

    // A MessageError was emitted over the event channel.
    let events = drain(&mut rx);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, CoreEvent::MessageError { .. })),
        "MessageError emitted: {events:?}"
    );

    // An Error-status assistant message was persisted so the record is complete.
    let persisted = session_manager
        .list_messages(conversation.id)
        .await
        .unwrap();
    let assistant = persisted
        .iter()
        .find(|m| m.role == Role::Assistant)
        .expect("error-status assistant message persisted");
    assert_eq!(assistant.status, MessageStatus::Error);
    assert!(assistant.route.is_none());
}
