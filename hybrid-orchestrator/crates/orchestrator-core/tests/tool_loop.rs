//! Integration test for the FEAT-002 function-calling bridge + permission model
//! (architecture.md Sections 5.4 / 5.5 / 5.6 / 9.4, P3.4 / P3.5 / P3.6).
//!
//! It drives the full Section 5.5 loop with NO live network:
//!
//!   1. a MOCK [`ChatProvider`] whose first `chat()` returns an assistant
//!      message with a tool call for a namespaced fixture tool, and whose second
//!      `chat()` returns a final assistant text message; and
//!   2. the bundled cross-platform `tool-loop-mock-server` `[[bin]]` connected
//!      over stdio (spawned via `env!("CARGO_BIN_EXE_...")`, `.exe`-aware on
//!      Windows).
//!
//! It asserts: tools are attached to the `ChatRequest`; the model's tool call
//! resolves to the fixture server; permission is AUTO-ALLOWED
//! (`PermissionMode::Allow`); the tool result is appended as a `tool` message;
//! and the (mock) generation continues to a final message. It also covers the
//! P3.6 failure modes as structured tool-error messages: argument-schema
//! validation failure, permission `Deny`, and `Ask`-mode blocking then
//! unblocking via `resolve_permission`.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures_util::stream::{self, BoxStream};

use mcp_client::McpServerHandle;
use orchestrator_core::{CoreEvent, Decision, PermissionGate, PermissionRegistry, ToolBridge};
use providers::{
    Capabilities, ChatDelta, ChatMessage, ChatProvider, ChatRequest, ChatResponse, MessageRole,
    ModelInfo, ProviderError,
};
use providers::{ChatChoice, FinishReason};

use domain::{McpServerConfig, McpTransport, PermissionMode};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use uuid::Uuid;

/// Path to the bundled mock MCP server binary (resolved by Cargo, `.exe` on
/// Windows).
const MOCK_SERVER: &str = env!("CARGO_BIN_EXE_tool-loop-mock-server");

/// A mock `ChatProvider` that replays a scripted queue of `chat()` responses.
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
        "scripted"
    }

    fn capabilities(&self, _model: &str) -> Capabilities {
        Capabilities {
            tools: true,
            ..Capabilities::default()
        }
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        Ok(vec![ModelInfo::new("mock-model")])
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
        // Unused by this test (the loop uses the non-streaming `chat`).
        Ok(Box::pin(stream::empty()))
    }
}

/// Assistant response that requests a single tool call for `tool_name` with
/// `arguments` (a JSON string, OpenAI encoding).
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
        model: Some("mock-model".to_string()),
    }
}

/// Final assistant text response.
fn text_response(text: &str) -> ChatResponse {
    ChatResponse {
        choices: vec![ChatChoice {
            index: 0,
            message: ChatMessage::text(MessageRole::Assistant, text),
            finish_reason: Some(FinishReason::Stop),
        }],
        usage: None,
        model: Some("mock-model".to_string()),
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

/// Build a connected handle to the fixture server in `mode` plus a bridge over
/// it, returning the bridge, the server id, and the core-event receiver.
async fn connected_bridge(
    mode: PermissionMode,
) -> (ToolBridge, Uuid, UnboundedReceiver<CoreEvent>) {
    let id = Uuid::new_v4();
    let handle = Arc::new(McpServerHandle::new(stdio_config(id, mode)));
    handle.connect().await.expect("connect to mock server");

    let (tx, rx): (UnboundedSender<CoreEvent>, UnboundedReceiver<CoreEvent>) = unbounded_channel();
    let gate = PermissionGate::new(tx.clone(), PermissionRegistry::new());
    let bridge = ToolBridge::new([handle], gate, tx);
    (bridge, id, rx)
}

#[tokio::test]
async fn full_tool_loop_auto_allowed_and_continues() {
    let (bridge, server_id, _rx) = connected_bridge(PermissionMode::Allow).await;

    // The model will call the namespaced `echo` tool.
    let namespaced = format!("{server_id}__echo");
    let provider = ScriptedProvider::new(vec![
        tool_call_response("call-1", &namespaced, r#"{"text":"hello"}"#),
        text_response("The tool said hello."),
    ]);

    // Build the initial request and ATTACH TOOLS from the connected server.
    let mut request = ChatRequest::new(
        "mock-model",
        vec![ChatMessage::text(MessageRole::User, "please echo hello")],
    );
    bridge.attach_tools(&mut request).await;
    assert!(
        request.tools.iter().any(|t| t.function.name == namespaced),
        "the echo tool is attached to the request"
    );

    // First turn: the model requests the tool call.
    let first = provider.chat(request.clone()).await.unwrap();
    let assistant = first.choices[0].message.clone();
    let tool_call = assistant.tool_calls[0].clone();
    assert_eq!(tool_call.function.name, namespaced);

    // The bridge resolves + auto-allows + invokes + normalizes to a `tool` msg.
    let tool_msg = bridge.handle_tool_call(&tool_call).await;
    assert_eq!(tool_msg.role, MessageRole::Tool);
    assert_eq!(tool_msg.tool_call_id.as_deref(), Some("call-1"));
    let content = tool_msg.content.clone().unwrap();
    assert!(content.contains("echoed"), "tool result content: {content}");
    assert!(!content.contains("\"isError\":true"), "not an error result");

    // Feed the assistant + tool messages back and continue generation.
    request.messages.push(assistant);
    request.messages.push(tool_msg);
    let second = provider.chat(request).await.unwrap();
    assert_eq!(
        second.choices[0].message.content.as_deref(),
        Some("The tool said hello.")
    );
}

#[tokio::test]
async fn schema_validation_failure_is_structured_tool_error() {
    let (bridge, server_id, _rx) = connected_bridge(PermissionMode::Allow).await;

    // `echo` REQUIRES a string `text`; send an integer to violate the schema.
    let namespaced = format!("{server_id}__echo");
    let call = providers::ToolCall {
        id: "call-bad".to_string(),
        kind: "function".to_string(),
        function: providers::FunctionCall {
            name: namespaced,
            arguments: r#"{"text":123}"#.to_string(),
        },
    };

    let msg = bridge.handle_tool_call(&call).await;
    assert_eq!(msg.role, MessageRole::Tool);
    assert_eq!(msg.tool_call_id.as_deref(), Some("call-bad"));
    let content = msg.content.unwrap();
    assert!(content.contains("\"isError\":true"), "content: {content}");
    assert!(content.contains("text"), "mentions the offending field");
}

#[tokio::test]
async fn unknown_server_is_structured_tool_error() {
    let (bridge, _server_id, _rx) = connected_bridge(PermissionMode::Allow).await;

    // A namespaced name whose server id is not connected.
    let namespaced = format!("{}__echo", Uuid::new_v4());
    let call = providers::ToolCall {
        id: "call-nope".to_string(),
        kind: "function".to_string(),
        function: providers::FunctionCall {
            name: namespaced,
            arguments: "{}".to_string(),
        },
    };

    let msg = bridge.handle_tool_call(&call).await;
    let content = msg.content.unwrap();
    assert!(content.contains("\"isError\":true"), "content: {content}");
}

#[tokio::test]
async fn permission_deny_is_structured_tool_error() {
    let (bridge, server_id, _rx) = connected_bridge(PermissionMode::Deny).await;

    let namespaced = format!("{server_id}__echo");
    let call = providers::ToolCall {
        id: "call-deny".to_string(),
        kind: "function".to_string(),
        function: providers::FunctionCall {
            name: namespaced,
            arguments: r#"{"text":"hi"}"#.to_string(),
        },
    };

    let msg = bridge.handle_tool_call(&call).await;
    assert_eq!(msg.tool_call_id.as_deref(), Some("call-deny"));
    let content = msg.content.unwrap();
    assert!(content.contains("\"isError\":true"), "content: {content}");
    assert!(content.contains("permission denied"), "content: {content}");
}

#[tokio::test]
async fn ask_mode_emits_event_then_unblocks_on_resolve() {
    // Build an Ask-mode bridge and keep a handle on its registry so we can
    // resolve the prompt.
    let id = Uuid::new_v4();
    let handle = Arc::new(McpServerHandle::new(stdio_config(id, PermissionMode::Ask)));
    handle.connect().await.expect("connect to mock server");

    let (tx, mut rx) = unbounded_channel();
    let registry = PermissionRegistry::new();
    let gate = PermissionGate::new(tx.clone(), registry.clone());
    let bridge = ToolBridge::new([handle], gate, tx);

    let namespaced = format!("{id}__echo");
    let call = providers::ToolCall {
        id: "call-ask".to_string(),
        kind: "function".to_string(),
        function: providers::FunctionCall {
            name: namespaced,
            arguments: r#"{"text":"hi"}"#.to_string(),
        },
    };

    // Drive the call on a task; it should emit a prompt and BLOCK.
    let task = tokio::spawn(async move { bridge.handle_tool_call(&call).await });

    // The Ask-mode prompt event is emitted with a request id.
    let request_id = match rx.recv().await.expect("permission event") {
        CoreEvent::PermissionRequested {
            request_id,
            server_id,
            ref tool_name,
            ..
        } => {
            assert_eq!(server_id, id);
            assert_eq!(tool_name, "echo");
            request_id
        }
        other => panic!("unexpected event: {other:?}"),
    };

    // Resolving allow unblocks the invocation, which then succeeds.
    assert!(registry.resolve(request_id, Decision::allow()));
    let msg = task.await.unwrap();
    assert_eq!(msg.tool_call_id.as_deref(), Some("call-ask"));
    let content = msg.content.unwrap();
    assert!(content.contains("echoed"), "content: {content}");
    assert!(!content.contains("\"isError\":true"), "content: {content}");
}
