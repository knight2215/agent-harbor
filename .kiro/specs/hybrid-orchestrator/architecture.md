# Hybrid Orchestrator: Architecture Specification

Status: Draft v1
Applies to: A cross-platform desktop application that orchestrates local (LM Studio) and cloud AI agents, acts as an MCP client, and routes work automatically or manually.

This document is the implementation-ready design for the application. It is a written specification only. It describes a target system built on Tauri (Rust core) with a React and TypeScript frontend.

## Table of Contents

1. Overview and Goals
2. High-Level Architecture
3. Module/Crate and Directory Structure
4. Provider Adapter Abstraction
5. MCP Client Integration
6. Routing Policy Engine
7. Session and State Management
8. UI Surfaces
9. Security Considerations
10. Extensibility Strategy

---

## 1. Overview and Goals

### 1.1 What this application is

The Hybrid Orchestrator is a desktop application that acts as the central orchestration layer between a user and a heterogeneous set of AI backends. It balances **local agents**, served by [LM Studio](https://lmstudio.ai/)'s local OpenAI-compatible inference server, against **cloud agents and LLMs** exposed by hosted provider APIs.

The application itself is the orchestrator. Concretely, it:

- Calls LM Studio's local OpenAI-compatible HTTP server (typically `http://localhost:1234/v1`) for local inference.
- Calls cloud LLM APIs (OpenAI, Anthropic, AWS Bedrock, Google Gemini, Azure OpenAI) through provider adapters.
- Acts as a **Model Context Protocol (MCP) client** to one or more local MCP servers, which expose application functions and external tools to the model as callable functions.
- Decides, per message, which provider and model should handle a request, using an automatic routing policy by default and honoring manual overrides when present.

### 1.2 Primary goals

1. **Hybrid routing.** Keep sensitive or cheap-to-run work local and send heavy or specialized work to the cloud, automatically by default, with full manual control.
2. **Open provider support.** Treat any OpenAI-compatible endpoint as a first-class citizen, and provide adapters for the major cloud providers that are not natively OpenAI-compatible.
3. **Tool use through MCP.** Expose local application capabilities and external tools to models through a standard, pluggable MCP client.
4. **Extensibility first.** Providers, MCP servers, and routing policies are pluggable modules. Adding one must not require modifying the core.
5. **Security by default.** API keys never live in plaintext config or in the frontend. The IPC boundary is narrow and validated. Local network calls are constrained to loopback.

### 1.3 Non-goals

The following are explicitly out of scope for this design:

- **No runtime dependency on Kiro or its ACP wire protocol.** Kiro and its internal ACP (Agent Client Protocol) wire format are used, at most, as a **development-time tool** to help author and scaffold this project. Kiro's ACP wire protocol is not a public, documented standard, and the Kiro CLI is not a general-purpose runtime harness. The shipped application MUST NOT depend on ACP or the Kiro CLI at runtime, MUST NOT spawn the Kiro CLI, and MUST NOT speak ACP to any process. All runtime agent communication happens over the provider adapters and the MCP client described in this document.
- **No custom model training or fine-tuning.** The application consumes inference endpoints; it does not train models.
- **No hosting of a public multi-tenant service.** This is a single-user desktop application. Multi-user server deployment is not a goal.
- **No proprietary cloud sync backend.** State is local first. Optional export/import is in scope; a hosted sync service is not.
- **No browser extension or mobile client** in this version. The design keeps the core portable so these could be added later, but they are not delivered here.

### 1.4 Key terms

| Term | Meaning |
| --- | --- |
| Provider | A backend that serves model inference (e.g. OpenAI, LM Studio). |
| Adapter | Code that maps a provider's native API onto the internal `ChatProvider` contract. |
| Model | A specific model offered by a provider (e.g. `gpt-4o`, a local GGUF model in LM Studio). |
| MCP server | A separate process exposing tools/resources over the Model Context Protocol. |
| Routing policy | A strategy that chooses a provider and model for a given request. |
| Agent persona | A reusable configuration bundle: system prompt, default model or routing preference, allowed tools, parameters. |
| Session / conversation | An ordered sequence of messages plus its configuration and metadata. |

---

## 2. High-Level Architecture

### 2.1 Layered view

The application is split into three layers: a React/TypeScript **frontend**, the **Tauri IPC boundary**, and a Rust **orchestration core**. The frontend never talks to providers, MCP servers, or the keystore directly. Everything crosses the IPC boundary as validated commands and events.

```
+-------------------------------------------------------------------------+
|                     Frontend (React + TypeScript)                        |
|                                                                          |
|  Chat  |  Model Selector  |  Tool/MCP Manager  |  Agent Editor  | History|
|                                                                          |
|                    UI state (Zustand/Redux), no secrets                  |
+-------------------------------------------------------------------------+
                    |   invoke(command)        ^   emit(event)
                    v                          |
+=========================================================================+
|                    Tauri IPC Boundary (secure bridge)                    |
|   Command allowlist  |  input validation  |  capability-scoped perms     |
+=========================================================================+
                    |                          ^
                    v                          |
+-------------------------------------------------------------------------+
|                    Rust Orchestration Core                               |
|                                                                          |
|  +------------------+   +------------------+   +--------------------+     |
|  | Session Manager  |   | Routing Policy   |   | Provider Registry  |     |
|  | (conversations,  |-->| Engine           |-->| + Adapters         |---->| (cloud APIs)
|  |  messages, state)|   | (auto + manual)  |   | OpenAI/Anthropic/  |     |
|  +------------------+   +------------------+   | Bedrock/Gemini/    |     |
|          |                     |              | Azure/LM Studio    |---->| (localhost:1234)
|          |                     |              +--------------------+     |
|          v                     v                                         |
|  +------------------+   +------------------+   +--------------------+     |
|  | Config /         |   | MCP Client       |   | Secrets / Keystore |     |
|  | Persistence Store |  | (stdio + HTTP/SSE)|  | (OS keychain)      |     |
|  | (SQLite)         |   |                  |   |                    |     |
|  +------------------+   +------------------+   +--------------------+     |
|                               |                                          |
+-------------------------------|------------------------------------------+
                                v
                        Local MCP servers (separate processes)
```

The core contains six long-lived subsystems:

- **Session Manager**: owns conversations, messages, agent personas, and their persistence.
- **Routing Policy Engine**: selects a provider/model per request (automatic default plus manual override).
- **Provider Registry and Adapters**: registers and instantiates provider adapters behind a common trait.
- **MCP Client**: connects to MCP servers, discovers tools, and invokes them.
- **Secrets/Keystore**: stores and retrieves API keys from the OS keychain.
- **Config/Persistence Store**: durable local storage (SQLite) plus versioned config.

### 2.2 End-to-end data flow for a single user message

The following sequence shows a message that triggers a tool call and streams a response.

```
User types message and presses send
  |
  v
[Frontend] invoke("send_message", { conversationId, content, overrideRoute? })
  |
  v
[IPC] validate args -> dispatch to Session Manager
  |
  v
[Session Manager] append user message, load conversation config + persona
  |
  v
[Routing Engine] build RoutingRequest (messages, privacy tags, persona prefs,
                 manual override) -> RoutingDecision { provider, model, rationale }
  |
  v
[Provider Registry] resolve adapter for chosen provider; fetch apiKey/baseURL
                    from Keystore (never returned to frontend)
  |
  v
[MCP Client] provide the set of discovered tools as function/tool definitions
             attached to the chat request
  |
  v
[Adapter] translate internal ChatRequest -> provider-native request; open stream
  |
  v  (streaming tokens)
[Core] emit event "message_delta" { conversationId, messageId, delta }
  |
  |-- if model requests a tool call:
  |     [MCP Client] invoke tool -> result -> feed back to adapter as tool result
  |     (permission check / prompt may occur here)
  |     continue stream
  |
  v
[Adapter] stream completes -> final message assembled
  |
  v
[Session Manager] persist assistant message, token usage, chosen route metadata
  |
  v
[Core] emit event "message_complete" { conversationId, messageId, usage, route }
  |
  v
[Frontend] renders streamed content and final metadata (chosen provider/model shown)
```

Key properties of this flow:

- Routing is resolved once per request unless a tool loop or explicit re-route occurs.
- Secrets are read inside the core only. The frontend receives the chosen provider/model label for display but never the credential.
- Streaming is delivered via Tauri events, not by holding a command open, so the UI stays responsive and multiple conversations can stream concurrently.

---

## 3. Module/Crate and Directory Structure

### 3.1 Why Tauri (baseline, deviation allowed with justification)

The baseline desktop framework is **Tauri**, chosen over Electron for:

- **Bundle size**: Tauri uses the OS-native webview instead of shipping a full Chromium runtime, producing installers that are typically an order of magnitude smaller.
- **Performance and memory**: no bundled browser engine means lower idle memory and faster startup.
- **Secure IPC**: Tauri's command and capability model gives a narrow, explicitly allowlisted bridge between frontend and backend, which suits the security posture in Section 9.
- **Extensibility**: a Rust core is a natural fit for a plugin-style architecture (traits + registries), async provider I/O, and process management for MCP servers.

This is a baseline, not a hard mandate. A future switch (for example to a pure-Rust GUI, or to Electron for a specific platform need) is allowed only if the change is documented here with explicit justification and a migration note. The core crates below are deliberately kept independent of Tauri so the shell can be replaced without rewriting orchestration logic.

### 3.2 Cargo workspace layout

The Rust side is a Cargo workspace. Each concern is a crate so it can be tested and evolved independently, and so the Tauri shell is a thin adapter over reusable libraries.

```
hybrid-orchestrator/
  Cargo.toml                      # workspace manifest
  crates/
    orchestrator-core/            # domain models, session manager, message pipeline
      src/
        lib.rs
        session.rs                # Conversation, Message, Session lifecycle
        pipeline.rs               # message -> route -> provider -> tools -> persist
        events.rs                 # core event types emitted to the shell
    providers/                    # ChatProvider trait + all adapters
      src/
        lib.rs
        contract.rs               # ChatProvider trait, ChatRequest/Response types
        registry.rs               # ProviderRegistry (dynamic registration)
        capability.rs             # capability descriptors + negotiation
        adapters/
          openai.rs               # native OpenAI-compatible
          lmstudio.rs             # native OpenAI-compatible (local baseURL)
          azure_openai.rs         # OpenAI-compatible with Azure deployment routing
          anthropic.rs            # translation shim (Messages API)
          gemini.rs               # translation shim (generateContent)
          bedrock.rs              # translation shim (SigV4 signing)
          generic_openai.rs       # any user-supplied OpenAI-compatible baseURL
    mcp-client/                   # MCP client: transports, lifecycle, tool discovery
      src/
        lib.rs
        transport/
          stdio.rs
          http_sse.rs
        session.rs                # spawn/connect/handshake/list/invoke/teardown
        tools.rs                  # tool descriptors -> function-calling schema
    routing/                      # routing policy engine
      src/
        lib.rs
        policy.rs                 # RoutingPolicy trait, RoutingRequest/Decision
        signals.rs                # complexity, privacy, cost estimators
        policies/
          auto_default.rs         # default automatic policy
          manual_override.rs      # honors per-message/per-conversation overrides
          registry.rs             # policy registration + selection
    persistence/                  # SQLite storage + versioned config schema
      src/
        lib.rs
        db.rs                     # sqlx pool, migrations
        repositories.rs           # conversations, messages, personas, tool configs
        config.rs                 # versioned app config load/save/migrate
    secrets/                      # keystore abstraction over OS keychain
      src/
        lib.rs                    # SecretStore trait + keyring-backed impl
    tauri-app/                    # the Tauri binary (thin shell)
      Cargo.toml
      tauri.conf.json
      capabilities/               # capability-scoped permission files
        default.json
      src/
        main.rs
        commands.rs               # #[tauri::command] handlers -> core calls
        state.rs                  # managed app state (Arc-wrapped subsystems)
        events.rs                 # bridge core events -> Tauri emit
  frontend/                       # React + TypeScript (Vite)
    index.html
    package.json
    vite.config.ts
    src/
      main.tsx
      app.tsx
      ipc/
        commands.ts               # typed wrappers over invoke()
        events.ts                 # typed listeners over Tauri event API
      state/
        conversations.ts          # store (Zustand)
        providers.ts
        tools.ts
        personas.ts
      features/
        chat/                     # UI surface 1
        model-selector/           # UI surface 2
        tool-manager/             # UI surface 3
        agent-editor/             # UI surface 4
        history/                  # UI surface 5
      components/                 # shared UI primitives
      types/                      # shared TS types mirrored from core DTOs
```

### 3.3 Crate responsibilities

| Crate | Responsibility | Key public types |
| --- | --- | --- |
| `orchestrator-core` | Owns domain models and the message pipeline. Coordinates routing, providers, MCP, persistence. Emits domain events. Framework-agnostic (no Tauri dependency). | `Conversation`, `Message`, `SessionManager`, `Pipeline`, `CoreEvent` |
| `providers` | Defines the `ChatProvider` contract and all adapters. Owns the `ProviderRegistry` and capability negotiation. | `ChatProvider`, `ChatRequest`, `ChatResponse`, `ProviderRegistry`, `Capabilities` |
| `mcp-client` | Full MCP client: transports (stdio, HTTP/SSE), server lifecycle, tool discovery, invocation, teardown. | `McpClient`, `McpServerHandle`, `ToolDescriptor`, `ToolInvocation` |
| `routing` | Routing policy engine. Trait, signals, default automatic policy, manual override, policy registry. | `RoutingPolicy`, `RoutingRequest`, `RoutingDecision`, `PolicyRegistry` |
| `persistence` | SQLite via `sqlx`, migrations, repositories, versioned config load/migrate. | `Db`, `ConversationRepo`, `PersonaRepo`, `AppConfig` |
| `secrets` | Keystore abstraction backed by the OS keychain (via the `keyring` crate). Never exposes secrets across IPC. | `SecretStore`, `SecretRef` |
| `tauri-app` | Thin Tauri shell. Wires managed state, exposes `#[tauri::command]` handlers, bridges core events to `emit`. Holds the capability files. | command handlers, `AppState` |

The dependency direction is strictly downward: `tauri-app` depends on the libraries; the libraries do not depend on `tauri-app`. `orchestrator-core` depends on `providers`, `mcp-client`, `routing`, `persistence`, and `secrets` through their public traits, which keeps each swappable.

### 3.4 Target build and test tooling

- **Rust core**: `cargo build`, `cargo test` per crate; `cargo clippy` for lint.
- **Frontend**: Vite + npm; `vitest` and React Testing Library for component tests.
- **App**: Tauri CLI (`tauri dev`, `tauri build`) orchestrates both sides into a signed installer.

---

## 4. Provider Adapter Abstraction

### 4.1 The OpenAI-compatible base contract

The internal contract is a Rust trait modeled on the OpenAI Chat Completions shape, because it is the widest common denominator across providers and is exactly what LM Studio serves natively.

```rust
/// The internal contract every provider adapter implements.
#[async_trait::async_trait]
pub trait ChatProvider: Send + Sync {
    /// Stable provider id, e.g. "openai", "lmstudio", "anthropic".
    fn id(&self) -> &str;

    /// What this provider/model can do (streaming, tools, vision, etc.).
    fn capabilities(&self, model: &str) -> Capabilities;

    /// List models the provider currently offers (may hit the network).
    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError>;

    /// Non-streaming completion.
    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, ProviderError>;

    /// Streaming completion. Yields deltas until the stream ends.
    async fn chat_stream(
        &self,
        req: ChatRequest,
    ) -> Result<BoxStream<'static, Result<ChatDelta, ProviderError>>, ProviderError>;
}

pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,      // system/user/assistant/tool roles
    pub tools: Vec<ToolSpec>,            // function-calling tool schemas (from MCP)
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    pub stream: bool,
    pub extra: serde_json::Value,        // provider-specific passthrough
}

pub struct Capabilities {
    pub streaming: bool,
    pub tools: bool,        // function/tool calling
    pub vision: bool,       // image inputs
    pub json_mode: bool,
    pub max_context: Option<u32>,
}
```

`ChatMessage`, `ToolSpec`, `ChatResponse`, and `ChatDelta` use the OpenAI-compatible message and tool-call shape internally. Adapters translate to and from their native formats.

### 4.2 Adapter construction: swappable baseURL and apiKey

Every adapter is constructed from a small config so endpoints and credentials are swappable at runtime:

```rust
pub struct ProviderConfig {
    pub id: String,
    pub kind: ProviderKind,          // OpenAI, Anthropic, Bedrock, Gemini, Azure, LmStudio, GenericOpenAI, Ollama, Embedded
    pub base_url: Option<String>,    // swappable; defaults per kind
    pub api_key_ref: Option<SecretRef>, // reference into the keystore, not the key itself
    pub extra: serde_json::Value,    // e.g. Azure deployment, Bedrock region, Gemini project
}
```

The `api_key_ref` is a handle the core resolves against the keystore at call time. The raw key is never stored in this struct and never crosses IPC.

For local runtimes (LM Studio and generic OpenAI-compatible endpoints), the `set_local_runtime` command writes this `ProviderConfig` from the Local Runtimes settings surface: it validates the entered `base_url` through the Section 9.3 posture (`check_provider_base_url`) before persisting and upserts by a stable per-kind id, so a configured local endpoint enumerates its models under Local and routes like any other provider.

### 4.3 Mapping the six targets onto the contract

| Provider | Native wire format | Adapter strategy | Notable config |
| --- | --- | --- | --- |
| **OpenAI** | OpenAI Chat Completions (native) | Direct: pass through with minimal mapping | `base_url` default `https://api.openai.com/v1`, `api_key_ref` |
| **LM Studio (local)** | OpenAI-compatible (native) | Direct: same code path as OpenAI adapter, different base URL | `base_url` default `http://localhost:1234/v1`, usually no key |
| **Ollama (local)** | OpenAI-compatible (native) chat + native model discovery | Direct for chat/streaming (same code path as OpenAI adapter, different base URL); `list_models` overridden to call Ollama's native `GET /api/tags` at the server root (not `/v1`) | `base_url` default `http://127.0.0.1:11434/v1` (IPv4 loopback literal, to avoid `localhost` resolving to `::1` when Ollama binds `127.0.0.1`), no key; treated as a local provider like LM Studio |
| **Embedded (local, in-process)** | llama.cpp GGUF inference in-process (no HTTP) | Direct: the `engine` crate's `EmbeddedEngine` implements the same `ChatProvider` contract and emits OpenAI-shaped `ChatDelta`s; no shared HTTP client is involved. `list_models` returns the imported local `.gguf` models | no `base_url`/key (runs in-process); `base_url`, when set, names the local models directory; treated as a local provider, seeded at zero token price |
| **Azure OpenAI** | OpenAI-compatible with deployment routing | Near-direct: rewrite path to `/openai/deployments/{deployment}/chat/completions`, add `api-version` query and `api-key` header | `extra.deployment`, `extra.api_version`, `base_url` = resource endpoint |
| **Anthropic** | Messages API (`/v1/messages`) | Translation shim: map roles, split system prompt to top-level `system`, map tool schema to Anthropic `tools`, translate SSE deltas | `base_url` default `https://api.anthropic.com`, `anthropic-version` header |
| **Google Gemini** | `generateContent` / `streamGenerateContent` | Translation shim: map messages to `contents`/`parts`, map tools to `functionDeclarations`, translate streamed chunks | `extra.project`, key or OAuth per config |
| **AWS Bedrock** | Provider-specific model bodies, SigV4-signed | Translation shim: SigV4 request signing, per-model body shaping (Anthropic-on-Bedrock, Titan, etc.), translate event stream | `extra.region`, AWS credential resolution via keystore/credential provider |

Adapters fall into two families:

- **Native OpenAI-compatible** (OpenAI, LM Studio, Ollama, Azure OpenAI, and any user-supplied `GenericOpenAI` endpoint): reuse a shared HTTP + SSE client with thin differences (path/header rewriting for Azure; Ollama additionally overrides model discovery to its native `GET /api/tags` endpoint at the server root).
- **Translation shims** (Anthropic Messages API, Gemini `generateContent`, Bedrock SigV4 and per-model bodies): implement request/response translation and stream normalization so the rest of the system sees the OpenAI-compatible shape.

Because LM Studio speaks the native format, local inference uses the exact same code path as OpenAI with only `base_url` differing. This is deliberate: it keeps the local path simple and reliable. Ollama is handled the same way for chat and streaming (OpenAI-compatible at `http://127.0.0.1:11434/v1`), with one addition: because Ollama does not surface installed models under the OpenAI `/v1/models` path, its adapter overrides `list_models` to call the native `GET /api/tags` endpoint (rooted at the server root, not `/v1`, and returning `{"models":[{"name":...}]}`). That discovery request sends an explicit `Accept: application/json` header so a content-negotiating server returns JSON, and its `TagsResponse` decoder deliberately does not set `deny_unknown_fields`, so the rich real payload (each model carrying `details` and `capabilities` beyond `name`) decodes without error. Like LM Studio, Ollama is classified as a local provider for the routing privacy gate and seeded at zero token price, so discovered Ollama models surface under Local in the model picker.

The **embedded engine (Strategy B)** is the zero-install local option: it runs GGUF models in-process via the llama.cpp family, so no separate Ollama or LM Studio install is required. It lives in its own `engine` crate and implements the same `ChatProvider` contract as every HTTP adapter, emitting OpenAI-shaped `ChatDelta`s so the routing engine, the pipeline, and the UI treat it uniformly. It is registered as `ProviderKind::Embedded` through an `EmbeddedFactory`, classified as provably-local for the routing privacy gate (it runs in-process, so it is always local), and seeded at zero token price, so imported GGUF models surface under Local in the model picker. Crate isolation: the heavy native llama.cpp binding is an optional dependency gated behind a cargo feature `llama` that is DEFAULT OFF. Under default features the crate is a dependency-light stub whose `chat`/`chat_stream` return a clear "not compiled with llama" error, so the routine per-crate build (and everything that depends on it: providers, tauri-app) compiles with no native library, C++ toolchain, or network. Enabling `llama` turns on the real llama.cpp-backed path; it is compiled only by a dedicated gated CI job with a C++ toolchain, never in the offline build.

On AWS Bedrock credentials specifically: to stay consistent with the keystore-only invariant in Section 9.1, AWS access keys entered in the app are stored as a `SecretRef` in the OS keychain exactly like every other provider's key, and the Bedrock adapter resolves them through that `SecretRef` at call time. The standard AWS credential provider chain (environment variables, shared config/profile, and instance or SSO roles) remains an optional secondary path for users who already manage AWS credentials outside the app; when that path is used no secret enters the app at all, so it does not weaken the "no secret material outside the keychain" invariant. In both cases the raw credentials never cross the IPC boundary.

### 4.4 Capability negotiation

Each adapter reports `Capabilities` per model. The pipeline consults capabilities before building a request:

- If the model does not support `tools`, MCP tools are omitted and the request is adjusted (or the router avoids that model when tools are required).
- If `streaming` is unsupported, the core falls back to `chat` and synthesizes a single delta.
- `vision` and `json_mode` gate optional request features.
- `max_context` informs truncation/summarization decisions in the session manager.

Capabilities are also surfaced to the routing engine as a hard constraint: a `RoutingDecision` must select a model whose capabilities satisfy the request (for example, a request needing tools cannot be routed to a tool-incapable model).

### 4.5 ProviderRegistry and dynamic registration

```rust
pub struct ProviderRegistry {
    factories: HashMap<ProviderKind, Box<dyn ProviderFactory>>,
    instances: HashMap<String, Arc<dyn ChatProvider>>,
}

pub trait ProviderFactory: Send + Sync {
    fn kind(&self) -> ProviderKind;
    fn build(&self, cfg: &ProviderConfig, secrets: &dyn SecretStore)
        -> Result<Arc<dyn ChatProvider>, ProviderError>;
}
```

- Built-in factories are registered at startup, one per `ProviderKind`.
- User-configured providers (including multiple LM Studio endpoints or several OpenAI-compatible services) are instantiated from `ProviderConfig` rows in persistence.
- Adding a new provider means implementing `ChatProvider` + `ProviderFactory` and registering the factory. No changes to the core pipeline are required (see Section 10).

---

## 5. MCP Client Integration

### 5.1 Role of the built-in MCP client

The core embeds a full MCP client. MCP servers are separate processes that expose **tools** (callable functions), and optionally resources and prompts. The client discovers those tools and presents them to the model as function-calling tool specs, then executes tool calls the model requests and feeds results back into the conversation.

MCP servers are **pluggable modules**: the user adds or removes them at runtime through the Tool/MCP Server Manager UI (Section 8.3). The set of active servers is persisted and reconnected on startup.

### 5.2 Transports

Two transports are supported, matching the MCP standard:

- **stdio**: the client spawns the server as a child process and speaks JSON-RPC over stdin/stdout. Used for local, filesystem-adjacent tools.
- **HTTP/SSE**: the client connects to an already-running server over HTTP, using Server-Sent Events for server-to-client streaming. Used for network-reachable or long-lived servers.

```rust
pub enum McpTransport {
    Stdio { command: String, args: Vec<String>, env: Vec<(String, String)> },
    HttpSse { url: String, headers: Vec<(String, String)> },
}
```

### 5.3 Lifecycle

```
add/enable server
  |
  v
spawn (stdio) or connect (HTTP/SSE)
  |
  v
initialize handshake  (protocol version, client capabilities <-> server capabilities)
  |
  v
list tools  (and resources/prompts if used)  -> cache ToolDescriptors
  |
  v
[ready]  -- tools exposed to models as function-calling specs
  |
  v
invoke tool(s) on demand during a chat turn
  |
  v
teardown  (graceful shutdown / kill child; on disable, remove, or app exit)
```

The client maintains a `McpServerHandle` per server with its connection state, negotiated capabilities, and cached tool list. Tool lists are refreshed on reconnect and on demand.

### 5.4 Tool discovery and exposure to the model

Discovered tools are converted from MCP tool descriptors (name, description, JSON Schema input) into the internal `ToolSpec` used in `ChatRequest.tools`. Tool names are namespaced by server (for example `filesystem__read_file`) to avoid collisions across servers. When a model returns a tool call, the client:

1. Resolves the namespaced tool name back to the owning server.
2. Validates arguments against the tool's input schema.
3. Runs a permission check (Section 5.6) before invocation.
4. Invokes the tool and normalizes the result into a `tool` role message.

### 5.5 Tool-invocation sequence

```
Model (via adapter) emits tool_call { name: "filesystem__read_file", args }
  |
  v
[MCP Client] map name -> server "filesystem", tool "read_file"
  |
  v
[MCP Client] validate args against tool input JSON Schema
  |
  v
[Permission] check policy: auto-allow / prompt user / deny
  |          (if prompt: emit event to frontend, await user decision)
  v
[MCP Client] send tools/call JSON-RPC to server -> receive result (or error)
  |
  v
[Core] append tool result as a `tool` message; continue the model stream
  |
  v
Model incorporates result and continues generating the answer
```

### 5.6 Error and permission handling

- **Permission model**: each server has a permission mode (`ask`, `allow`, `deny`) and optional per-tool overrides. In `ask` mode the core emits a permission-request event and waits for the user's decision from the UI. Decisions can be remembered per tool for the session or persisted.
- **Transport errors**: connection loss marks the server `disconnected`; the client attempts bounded reconnect with backoff. Tools from a disconnected server are hidden from new requests.
- **Invocation errors**: schema validation failures, timeouts, and server-returned errors are captured and returned to the model as a structured tool error result so it can recover or report gracefully, rather than crashing the turn.
- **Resource limits**: per-tool timeout and output size caps prevent a misbehaving server from stalling a conversation.

---

## 6. Routing Policy Engine

### 6.1 Trait and decision type

Routing is pluggable behind a trait. A policy takes a `RoutingRequest` (context about the pending message) and returns a `RoutingDecision`.

```rust
#[async_trait::async_trait]
pub trait RoutingPolicy: Send + Sync {
    fn id(&self) -> &str;
    async fn decide(&self, req: &RoutingRequest) -> Result<RoutingDecision, RoutingError>;
}

pub struct RoutingRequest {
    pub messages: Vec<ChatMessage>,
    pub privacy_tags: Vec<PrivacyTag>,     // e.g. Local-Only, Confidential
    pub persona: Option<AgentPersona>,     // persona routing preferences
    pub manual_override: Option<ManualRoute>, // per-message override, if any
    pub conversation_pref: Option<ManualRoute>, // per-conversation pin, if any
    pub available: Vec<AvailableModel>,    // provider/model + capabilities + price
    pub budget: Option<CostBudget>,        // token/price budget signals
}

pub struct RoutingDecision {
    pub provider_id: String,
    pub model: String,
    pub rationale: String,   // human-readable explanation, shown in the UI
    pub source: RouteSource, // Manual | ConversationPin | Automatic
}
```

### 6.2 Automatic default policy and its signals

The default policy (`auto_default`) runs when no manual route applies. It scores candidate models using three signal families and picks the best-fit candidate.

1. **Task complexity estimation**. Heuristics estimate difficulty from the request: message length and count, presence of code or structured reasoning cues, required context window, and whether tools/vision are needed. Low-complexity turns prefer a capable local model; high-complexity turns prefer a stronger cloud model.
2. **Privacy tags**. Tags on the conversation or message express data-handling constraints. A `Local-Only` (or `Confidential`) tag is a **hard constraint**: the request MUST be routed to a local provider (LM Studio). If no local model can satisfy the request under that tag, routing fails closed with a clear error rather than silently sending data to the cloud.
3. **Cost signals**. Per-provider token pricing plus an optional budget (per conversation or per period) bias selection toward cheaper or local models when quality needs are met, and allow escalation to premium cloud models when complexity warrants and budget allows. Pricing data comes from user-entered per-provider token rates stored in the versioned app config (Section 10.4), seeded with optional bundled defaults per `ProviderKind` that the user can review and override. Local providers such as LM Studio default to zero token cost. This is the single source the cost signal and `list_available_models()` (Section 8.2) both read. Because these rates are static config rather than a live pricing feed, stale pricing is a known risk: rates that drift out of date silently skew routing toward or away from a provider, so the config surface flags when a rate was last edited and defaults are treated as approximate until confirmed by the user.

The scoring combines these into a ranked list, filtered first by hard constraints (privacy, required capabilities from Section 4.4), then ordered by a weighted blend of quality-for-complexity and cost. The chosen candidate's `rationale` records which signals drove the decision so the UI can display "why this model".

### 6.3 Manual override and precedence

Manual control exists at two granularities:

- **Per-message override** (`manual_override`): the user pins a provider/model for a single send (for example via the model selector's "use this model for this message" action).
- **Per-conversation pin** (`conversation_pref`): the user pins a provider/model for the whole conversation.

**Precedence rules** (highest wins):

1. **Per-message manual override** always wins when present. `source = Manual`.
2. **Per-conversation pin** applies when there is no per-message override. `source = ConversationPin`.
3. **Automatic policy** applies when neither manual form is present. `source = Automatic`.

Manual routes still pass through hard-constraint validation for safety: a manual choice that violates a `Local-Only` privacy tag is rejected with an explanatory error, so a manual selection cannot accidentally leak local-only data to the cloud. Aside from that safety gate, manual selection overrides all automatic scoring.

### 6.4 Registration and swapping

```rust
pub struct PolicyRegistry {
    policies: HashMap<String, Arc<dyn RoutingPolicy>>,
    active: String,   // id of the policy used for automatic decisions
}
```

- Built-in policies (`auto_default`, plus a `manual_override` resolver that implements the precedence above) are registered at startup.
- The active automatic policy is selectable in settings and persisted.
- A new policy is added by implementing `RoutingPolicy` and registering it; the engine and pipeline are untouched (see Section 10). The `manual_override` precedence logic wraps whatever automatic policy is active, so custom policies only need to implement the automatic decision.

---

## 7. Session and State Management

### 7.1 Data models

```rust
pub struct Conversation {
    pub id: Uuid,
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub persona_id: Option<Uuid>,
    pub conversation_pref: Option<ManualRoute>, // per-conversation model pin
    pub privacy_tags: Vec<PrivacyTag>,
    pub enabled_tool_servers: Vec<Uuid>,         // which MCP servers are active here
}

pub struct Message {
    pub id: Uuid,
    pub conversation_id: Uuid,
    pub role: Role,                 // System | User | Assistant | Tool
    pub content: MessageContent,    // text, tool calls, tool results, attachments
    pub created_at: DateTime<Utc>,
    pub route: Option<RouteMetadata>, // which provider/model answered + rationale
    pub usage: Option<TokenUsage>,
    pub status: MessageStatus,      // Pending | Streaming | Complete | Error
}

pub struct AgentPersona {
    pub id: Uuid,
    pub name: String,
    pub system_prompt: String,
    pub default_route: Option<ManualRoute>,   // preferred provider/model
    pub routing_hint: Option<RoutingHint>,    // e.g. prefer-local, prefer-quality
    pub allowed_tool_servers: Vec<Uuid>,
    pub parameters: ModelParameters,          // temperature, max_tokens, etc.
}

pub struct McpServerConfig {
    pub id: Uuid,
    pub name: String,
    pub transport: McpTransport,
    pub permission_mode: PermissionMode,      // Ask | Allow | Deny
    pub enabled: bool,
}

pub struct ProviderConfig { /* see Section 4.2 */ }
```

### 7.2 Persistence approach

- **Store**: SQLite via `sqlx`, embedded in the core through the `persistence` crate. SQLite fits a single-user desktop app: transactional, file-based, no server, easy backup/export.
- **Migrations**: versioned SQL migrations run at startup. The config schema carries a `schema_version` for forward-compatible upgrades (Section 10.4).
- **Tables** (indicative): `conversations`, `messages`, `agent_personas`, `mcp_servers`, `providers`, `app_config`. Secrets are not stored here; only `SecretRef` handles are.
- **Repositories**: typed repository structs in `persistence` provide CRUD used by the session manager.

### 7.3 Core state vs frontend state

| Concern | Owner | Notes |
| --- | --- | --- |
| Source of truth (conversations, messages, personas, configs) | Rust core (SQLite) | Durable, authoritative |
| Secrets/API keys | Keystore (core only) | Never sent to frontend |
| Active MCP connections and provider instances | Rust core (in-memory) | Rebuilt from config on startup |
| In-flight streaming buffers | Rust core, streamed to frontend via events | Frontend accumulates deltas |
| UI view state (open panels, selection, drafts) | Frontend store (Zustand) | Ephemeral, may cache reads |
| Displayed conversation data | Frontend cache, hydrated from core | Invalidated by core events |

The frontend holds a cache for responsiveness but treats the core as authoritative. Core events (`message_delta`, `message_complete`, `conversation_updated`, `mcp_state_changed`, `permission_requested`, `providers_changed`) keep the cache in sync.

**Event fan-out and startup hydration.** The shell opens exactly ONE `onCoreEvent` subscription in a single root `useEffect` (empty deps) and dispatches every `CoreEvent` to each store's `applyCoreEvent`, so the surfaces stay in sync without subscribing independently. That same root effect also HYDRATES the caches that are not tied to a specific surface's mount: in particular it triggers the providers store's `load()` once at startup, so the chat model picker is populated on launch without the user first opening Settings (surface-owned stores such as personas/tools/history still load on their own component's mount). Provider-config mutations then keep that cache fresh: the `set_cloud_provider`, `clear_cloud_provider`, `set_local_runtime`, `clear_local_runtime`, and the embedded-model `import`/`select`/`load`/`unload` commands emit `CoreEvent::ProvidersChanged` after a SUCCESSFUL mutation (fire-and-forget through the core-event sender, mirroring the MCP state events; a validation or persist error emits nothing), and the providers store refetches `list_available_models` on that event. A user-facing "Refresh models" affordance near the chat picker calls the same `load()` for on-demand re-enumeration.

### 7.4 Streaming and token state

- A message begins `Pending`, transitions to `Streaming` as `message_delta` events arrive, then `Complete` (or `Error`).
- Token usage and the final route metadata are attached on `message_complete` and persisted.
- The frontend renders partial content live; on completion it reconciles with the persisted message.
- **`message_started` announces each message before its first delta** so the frontend can seed a placeholder keyed on `messageId` (without it the deltas reference an id the store has never seen and are dropped). It carries an OPTIONAL `text` field: for the persisted USER message the pipeline populates `text` with the full user content (there is no user streaming), so the chat surface renders the user's own words IMMEDIATELY on send instead of an empty bubble that no delta ever fills; for the assistant reply `text` is absent and the content arrives via subsequent `message_delta` chunks. The field is serialized camelCase and omitted from the wire when absent (`skip_serializing_if`), so it is backward compatible with any consumer predating it. Because the seeded user text comes from the event (not a client guess), it is correct on send AND durable across a later `get_messages` reload.

### 7.5 Concurrency

- The core is async (Tokio). Each conversation's active turn runs as an independent task, so **multiple conversations can stream concurrently** from the same or different providers.
- Provider adapters and MCP server handles are `Arc`-shared and internally safe for concurrent use; per-conversation state is isolated.
- Persistence writes are serialized through the connection pool; reads are concurrent. A per-conversation lock prevents interleaved writes within a single conversation while allowing parallelism across conversations.

---

## 8. UI Surfaces

All five surfaces are React feature modules. Each calls the core through typed IPC wrappers (`invoke`) and subscribes to typed Tauri events. No surface accesses secrets or provider endpoints directly.

### 8.1 Chat interface

- **Purpose**: the primary conversation view: message list, streaming assistant output, tool-call visualization, composer, and per-message route badge (which provider/model answered and why).
- **Components**: `WelcomeScreen` (default landing), `MessageList`, `MessageBubble` (with tool-call/tool-result rendering), `StreamingIndicator`, `Composer` (bottom-anchored, with the inline model/routing controls folded in), `RouteBadge`, `PermissionPrompt` (modal for MCP `ask` mode).
- **State**: active conversation id, message list (hydrated + live deltas), composer draft, pending permission requests, the composer web-search toggle flag.
- **IPC commands**: `send_message(conversationId, content, overrideRoute?)`, `stop_generation(conversationId)`, `resolve_permission(requestId, decision)`, `get_messages(conversationId)`.
- **Events consumed**: `message_started` (seeds the placeholder; carries the user text, Section 7.4), `message_delta`, `message_complete`, `message_error`, `permission_requested`.

- **One conversation per "new conversation then send" flow, and the user's own message is visible on send**: starting a fresh conversation from the `WelcomeScreen` and then typing the first message creates EXACTLY ONE conversation and shows exactly one row in the list (never a duplicate "New Conversation"). Two lifecycle rules make this hold. (1) The store's `createConversation` UPSERTS the returned conversation by id rather than blindly appending: the same conversation also arrives through `loadConversations` when the pipeline emits `conversation_updated` on the first message persist, so a blind append plus that refetch could otherwise surface the created conversation twice; upserting reconciles to a single row regardless of ordering (the core stays authoritative per Section 7.3). (2) `MessageList` loads the message history AT MOST ONCE per active conversation (guarded by the last-loaded id), because every entry point that activates a conversation (`WelcomeScreen`'s `openConversation`, the sidebar / History `resumeConversation`) already loads its messages; an unconditional mount-effect reload was both redundant and harmful, since a reload resolving mid-turn (before the user/assistant rows converge in the DB) would `set({ messages })` over the freshly seeded placeholders and blank the just-typed user message. Combined with the `message_started` user `text` (Section 7.4), the user's own message renders immediately on send and survives any later reload.

- **Composer key handling (send vs newline, IME-safe)**: the `Composer` textarea sends the draft on plain `Enter` and inserts a newline on `Shift+Enter`, so multi-line messages are still composable without a mouse. The `Enter`-to-send path is guarded against IME composition (`event.nativeEvent.isComposing` / `keyCode === 229`) so committing a CJK candidate with `Enter` composes text rather than firing a send. The explicit Send button and form submit continue to work unchanged; all three paths funnel through the same send handler so the per-message override flow from Section 8.2 is honored regardless of how the send was triggered.

- **Threaded, role-styled conversation view**: `MessageList`/`MessageBubble` render the active conversation as a scrollable, role-styled thread rather than a flat log. User turns and assistant turns are visually distinguished and aligned to opposite edges, a streaming assistant turn gets a distinct in-progress bubble, and the list auto-scrolls to the newest turn as deltas arrive. Styling is expressed entirely through the app's `var(--ah-*)` design tokens (harbor-blue theme, light and dark), so the thread inherits the shared palette instead of hard-coded colors. This layer is presentational over the existing conversations store data: the message data model and the streaming `CoreEvent` flow (Section 7.4) are unchanged.

- **No silent no-op on send (diagnostics-first, turn "nothing happened" into a readout)**: a send can never be a silent no-op, mirroring the model-selector self-reporting in Section 8.2. A rejected `send_message` IPC call emits NO `CoreEvent`s, so the failure is surfaced directly from the conversations store's `sendState`/`sendError` as a visible `role="alert"` "Failed to send: &lt;reason&gt;" affordance instead of the message silently disappearing. A pipeline turn failure surfaces as a `message_error` bubble that now carries the display-safe reason text (previously a generic "failed to generate" string that dropped the reason), and an errored message with no seeded placeholder still appears as a visible error bubble rather than vanishing. Per Section 9.1/9.2 the surfaced reason is display-safe: it names the failure class only and never carries key material or a raw provider response.

- **Backend send parity (resilient per-row registry build)**: `send_message` builds the provider registry per configured row (starting from the builtin registry, then `build_from_config` per row) exactly like `list_available_models_inner` (Section 8.2), instead of the old fail-fast `providers::build_registry` that short-circuited on the FIRST un-buildable row and made the whole send reject. Isolating an un-buildable provider row this way means one bad row no longer aborts an otherwise routable turn (this swallowed rejection was the root cause of the silent no-op above). `run_turn` (Section 7.4) still emits `MessageError` and persists an `Error`-status message on ANY turn failure, so the reason stays display-safe (Sections 6/7/9) and visible in the thread.

- **Kiro-style bottom-anchored composer + welcome landing + nested Conversations (FEAT-002 redesign)**: the chat surface is redesigned so its controls are compact and inline rather than a wall of controls stacked above the input. The `Composer` is bottom-anchored: the message thread scrolls above it while the composer stays pinned, and its control row (wrapped around the textarea) hosts inline affordances rather than large standalone blocks: the `InlineModelControl` (a compact model dropdown that reads/writes the SAME shared per-message override from Section 8.2, never its own copy), the compact `RoutingModeToggle` (the same Auto / Prefer Local / Prefer Quality / Manual radiogroup, shrunk to an inline segmented control), a "Why this model?" info icon (`AutoRationaleTooltip`, now icon-triggered instead of always-on body text), a warning icon that opens a small `role="dialog"` modal listing provider enumeration errors (`EnumerationErrorModal`, shown ONLY when `errors` is non-empty, replacing the always-on red text near the picker), a small `aria-label`led "Refresh models" icon, and three context affordances: **Attach** (📎), **Repository** (📁), and a **Web-search toggle** (🌐, `aria-pressed`). The Web-search toggle flips a real on/off flag owned by the conversations store (consumed by the web-search feature); Attach/Repository land as affordances wired to the file dialog + backend in their own features. Send/Stop and the IME-safe Enter/Shift+Enter handling and the `role="alert"` send-failure affordance are unchanged. The default landing view of the Chat destination is a `WelcomeScreen` (large logo + short intro + a prominent "New conversation" button): the chat surface (`region` "Chat") renders ONLY when `activeConversationId !== null`, so a fresh launch no longer looks like an already-open empty conversation. In the sidebar, the Conversations list is nested UNDER the Chat nav item (Chat > conversation list + New conversation, reusing the History surface's `ConversationList` + `NewConversationButton`), replacing the old always-on History block; History and Settings remain top-level destinations.

- **Attach / upload files (incl. images) + repository context (FEAT-003)**: the composer's **Attach** (📎) and **Repository** (📁) affordances are real working features that feed BOUNDED context into the next chat turn WITHOUT changing the pipeline. Attach opens the native file dialog (`tauri-plugin-dialog`) for text and image files; a text file is read by the `read_text_file` command (validated non-empty path, capped at `MAX_ATTACH_BYTES` = 256 KiB, UTF-8 only, binary-by-extension rejected) and an image by `read_file_base64` (capped at 4 MiB, MIME guessed from the extension). An image is attached ONLY when the model effective for the next turn (the per-message override, else the conversation pin, else the routed/first-available model) advertises the `vision` capability (Section 4.1 `Capabilities.vision`); otherwise a visible non-fatal notice ("This model doesn't support images") is shown and the image is NOT attached. Repository opens a folder picker, and `list_repo_files` walks the directory skipping version-control / dependency / build-output directories (`.git`, `node_modules`, `target`, `dist`, `build`, …) and binary-by-extension files, capping the entry count (500) and flagging `truncated` when capped; the UI presents a checkbox list under a hard total-size cap (128 KiB across selected repo files, selection disabled past the cap with an "X of Y KiB included" signal) and fetches each selected file's contents via `read_text_file`. Attachments (text/repo file contents + vision-gated images) are held in the conversations store for one turn; on send the composer PREPENDS a bounded, clearly-delimited context block (per-file fenced `### Attached file: <name>` / `### Repository file: <name>` blocks; a vision-gated image is included as a labelled note) to the trimmed draft and passes the whole thing as the existing `content` String, so `run_turn`/`send_message` semantics are unchanged. The assembled block is size-capped (same 128 KiB budget) and the attachments are CLEARED after a successful send (mirroring how the transient per-message override is consumed once). The failure-prone pure helpers (MIME guessing, the directory skip-list, binary filtering, byte-cap checks, base64) live in the host-verifiable `domain::file_context` module so they are `cargo test`-covered offline even though `tauri-app` builds only in CI. **Documented limitation**: `run_turn` maps only the text `content` into the provider history, so full multimodal image bytes are not passed through this path in this iteration; the vision GATE + visible notice are enforced regardless, and passing raw image bytes to a vision model is a follow-up.

- **Pluggable web search behind the 🌐 toggle (FEAT-004)**: the composer's Web-search toggle is a real working feature, not a placeholder. When it is ON and the user sends, the composer first calls `run_web_search(query)` with the raw draft, then PREPENDS a bounded, clearly-delimited "Web search results" block (one `- <title>` / `<url>` / `<snippet>` entry per hit, size-capped to the same 128 KiB budget as attachment context, reusing the `attachmentContext` assembly approach) to the `content` before `send_message`, so the model answers with the fetched results as context. The provider is PLUGGABLE behind a small trait + enum in the host-verifiable `providers` crate (`providers::web_search`): a `WebSearchProvider` trait (`async search(query, opts) -> Result<Vec<WebSearchResult{title,url,snippet}>, WebSearchError>`) and a `WebSearchKind` enum with **Tavily as the recommended default** (purpose-built for LLM/agent search, a simple JSON API, and a free tier), scaffolded `Brave` / `SerpApi` variants so the pluggability is real, and a **`Custom` variant (the user's OWN endpoint)** that builds a Tavily-JSON-compatible provider rooted at a user-supplied `base_url` (required for `Custom`; `WebSearchError::Config` when missing) so a user can add their own search provider instead of only picking from the preselected list. The Tavily adapter POSTs to a CONFIGURABLE `base_url` (default `https://api.tavily.com`) at `/search` with a JSON body `{ api_key, query, max_results }` and maps the response `results[]` (`title` / `url` / `content`) into `WebSearchResult`s; the configurable base_url is what lets the wiremock tests point it at a local MockServer (request-build, response-parse, error, empty-results, no live network). `WebSearchError` is display-safe and NEVER carries the key. Configuration lives in a Settings "Web Search" section (provider-kind dropdown Tavily default, API-key password input, optional max-results) mirroring Providers & Keys: `set_web_search_provider(kind, apiKey, maxResults?, baseUrl?)` validates and stores the key as an opaque `SecretRef` under a stable keychain handle (`web-search`, per Section 9.1, mirroring `set_cloud_provider`) and persists an ADDITIVE `web_search` field (`WebSearchConfig { enabledProvider, maxResults, baseUrl }`) on `AppConfig`; for the `custom` kind it requires `baseUrl` and validates it through the SAME base-url posture check (`check_provider_base_url`) the network-peer / local-runtime write paths use (rejecting Blocked link-local/metadata targets, warning on non-loopback plaintext HTTP). The **Settings "Web Search" section adds a "Custom (your own endpoint)" option and a "Search endpoint URL" input** (explicit `aria-label` "Web search endpoint URL"), shown for the custom kind, validated non-empty client-side and rehydrated from `baseUrl`. `get_web_search_config()` rehydrates a display-safe view (`kind` / `hasApiKey` / `maxResults` / `baseUrl`, never the key); `clear_web_search_provider()` deletes the secret and resets the config (including `baseUrl`). At search time `run_web_search` builds the configured provider against the persisted `baseUrl` when the kind needs a custom endpoint (falling back to the built-in default otherwise). `run_web_search` loads the config, resolves the key CORE-INTERNALLY at search time, builds the configured provider, and returns display-safe `WebSearchResultView`s. **Graceful degradation (never a silent hang)**: when web search is unconfigured (no provider/key) or the provider fails, `run_web_search` returns a clear `CommandError`; the composer shows a VISIBLE non-fatal notice (`role="status"`: "Web search unavailable: <reason>. Sending without web results.") and STILL sends the plain message. The toggle defaults OFF. **Live-verification note**: web search needs the user's own key AND live network, so its real behavior is verified on the user's build; in the sandbox it is unit-tested with mocked HTTP only.

### 8.2 Model selector (local vs cloud, per conversation)

- **Purpose**: choose how the active conversation is routed. Exposes three modes: Automatic (default), a per-conversation pin, and a one-off per-message override. Clearly groups **Local** (LM Studio) vs **Cloud** providers/models and shows availability and rough cost.
- **Components**: `RoutingModeToggle` (Auto / Prefer Local / Prefer Quality / Manual), `ProviderModelPicker` (grouped Network vs Local vs Cloud with capability and price hints), `InlineModelControl` (the compact composer-inline presentation of the shared per-message override; `PerMessageOverrideControl` remains the equivalent full-width presentation), `AutoRationaleTooltip` (an info-icon "Why this model?" popover), `EnumerationErrorModal` (a warning-icon-triggered modal for per-provider enumeration errors).

- **Network peers grouped distinctly (FEAT-006)**: `ProviderModelPicker` classifies a configured provider row into one of three groups: **Network** (a LAN peer, detected by the `network-peer-` provider-id prefix the backend stamps, see Section 9.3), **Local** (a provably on-host runtime), or **Cloud**. A LAN peer is classified into Network FIRST so a keyless peer's models land in the distinct Network group and are never double-counted under Local, letting the user tell an off-host peer apart from an on-host local runtime. Peers enumerate and route through the existing providers-store `load()` path with no pipeline change, because each is an ordinary OpenAI-compatible `ProviderConfig` row.
- **State**: current conversation routing mode, selected pin (if any), transient per-message override, list of available provider/model options with capabilities.
- **IPC commands**: `list_available_models()` (provider + model + capabilities + price), `set_conversation_route(conversationId, route | null)`, `set_message_override(conversationId, route | null)`, `get_route_explanation(conversationId)`.
- **Events consumed**: `providers_changed` (availability updates, now actually emitted by the provider-config mutation commands), `conversation_updated`.
- Ties directly to Section 6: a pin sets `conversation_pref`; a one-off sets `manual_override`; clearing both returns to `Automatic`.
- **Per-message override ownership**: the transient per-message override is a single value owned by the active conversation's routing store (`state/conversations.ts`), not by either surface in isolation. The `PerMessageOverrideControl` rendered inside the composer only reads and writes that shared value; it does not hold its own copy. When the chat surface's `Composer` fires `send_message` (Section 8.1), it reads the current override from that store and passes it as the `overrideRoute?` argument, then the store clears it after the send so the override applies to exactly one message. This makes the override payload flow unambiguous: the model selector control edits it, the shared store owns it, and the chat surface's send call carries it.

- **Enumeration errors surfaced (graceful degradation)**: `list_available_models` returns a structured result `{ models, errors }` rather than silently dropping providers that fail to enumerate. Each provider whose `list_models` fails is captured as a display-safe `ProviderEnumerationError { providerId, message }`, while every healthy provider still populates the picker. The selector renders those errors near the picker (and the Providers & Keys settings flow renders them there too) without blanking the list, so one misconfigured or unreachable provider no longer silently hides the rest. Routing is unaffected: `send_message` builds its candidates from `result.models` only. Per Section 9.1/9.2 the error `message` is display-safe: it names the failure (transport, HTTP status, decode, or auth) but never carries key material or a raw provider response, exactly like `ProviderError`'s `Display`.

- **Cloud keys become real providers**: entering a key through the Providers & Keys settings flow now persists a real `ProviderConfig` via the `set_cloud_provider` command family, so the selected provider's models actually enumerate and appear under Cloud (previously the flow wrote only a keystore entry and a UI marker, so nothing surfaced). The chosen provider KIND maps directly to a `ProviderKind`; the "Kiro" choice maps to `ProviderKind::GenericOpenAI` with a REQUIRED OpenAI-compatible `base_url` rather than introducing a new `ProviderKind` variant, since Kiro is reached as an OpenAI-compatible endpoint and needs no bespoke adapter. See Section 9.1/9.2 for how the key is stored by reference and never crosses IPC.

- **Populated on launch, refreshed on change (no dead picker)**: the providers store is loaded ONCE at app startup by the shell's root effect (Section 7.3), so the chat picker reflects the configured providers/models the moment the app opens, WITHOUT requiring the user to first open Settings > Providers & Keys. Previously `load()` ran only from the Providers & Keys settings surface, so a user who configured a key or started a local runtime saw the chat picker stay empty ("No models available yet ...") until they happened to open that settings page. Two mechanisms now keep the picker live: (1) provider-config mutations (`set_cloud_provider`, `clear_cloud_provider`, `set_local_runtime`, `clear_local_runtime`, and the embedded-model `import`/`select`/`load`/`unload` commands) emit `CoreEvent::ProvidersChanged` on success, which the store refetches on; and (2) an always-available, `aria-label`led "Refresh models" button next to the chat picker calls `load()` for on-demand re-enumeration. Because `load()` now runs in the chat context, the enumeration errors described above actually surface near the chat picker when a provider fails (instead of the picker being silently empty), turning a "nothing happened" report into a precise, display-safe "Couldn't load models from &lt;providerId&gt;: &lt;reason&gt;" line.

- **Self-reporting diagnostics (turn "nothing happened" into a readout)**: two remaining causes of a silently empty picker are now made visible so the app self-reports exactly what happened on the load path. First, the store `load()` no longer swallows an IPC rejection: it tracks a `loadState` (`idle` / `loading` / `loaded` / `failed`) plus a display-safe `lastError`, so a rejected `list_available_models` is captured instead of leaving `models:[]`/`errors:[]` with no signal. The chat model-selector area renders a copyable status line (`data-testid="model-load-status"`) reading `loading…`, `loaded N models from M providers`, or `failed: <error>`, placed above the per-provider `ProviderEnumerationErrors`. Second, a dedicated display-safe `provider_diagnostics` command reports, per configured provider row, exactly what the enumeration path saw: it returns `ProviderDiagnosticsReport { configuredCount, totalModelCount, providerCountWithModels, providers: [ ProviderDiagnostic { id, kind, baseUrl, instanceBuilt, modelCount, error } ] }` (camelCase over IPC). Its `_inner` mirrors `list_available_models_inner`'s seeding, registry build, and shared embedded-instance setup exactly, so it reports the SAME reality the picker sees rather than a parallel guess. An always-reachable Settings "Diagnostics" section runs `provider_diagnostics` on mount and renders that per-provider readout (id, kind, `baseUrl` or "default", `instanceBuilt` yes/no, `modelCount`, and any `error`) with a "Run diagnostics" / Refresh button; it catches an IPC throw from the command so the panel is never blank. Per Section 9.1/9.2 the report is DISPLAY-SAFE: `baseUrl` is the persisted display string (or `default` when none), and no `api_key_ref`, resolved `SecretRef`, or key material is ever included; `error` strings derive from `ProviderError`'s `Display` and name the failure class only.

- **Invisible skip now reported (backend)**: `providers::list_available_models` previously did `let Some(instance) = registry.get(&cfg.id) else { continue };`, so a configured provider row whose instance was never built contributed neither a model nor an error and vanished from both the picker and the error list. That None branch now records a display-safe `ProviderEnumerationError { providerId, message }` (a static message stating the provider is configured but no instance was built and cannot be enumerated) instead of silently continuing. Complementing this, `list_available_models_inner` builds the registry per configured row (starting from the builtin registry, then `build_from_config` per row) rather than via a fail-fast `build_registry(...)?`, so one un-buildable row no longer aborts the whole enumeration and turns into a thrown `CommandError`; the un-buildable row instead surfaces through the enumeration path as a per-provider error, keeping per-provider isolation.

- **Real build error surfaced, not the generic fallback**: for a configured-but-unbuilt row the enumeration path now prefers the REAL provider build error over the static "no instance was built" message. `providers` gains a `list_available_models_with_build_errors(registry, configs, pricing, build_errors: &BTreeMap<String, String>)` seam; the existing three-argument `list_available_models` delegates to it with an empty map so external callers are unchanged. When `registry.get(&cfg.id)` is `None` and `build_errors` carries an entry keyed by `cfg.id`, that captured message is used for the `ProviderEnumerationError`, falling back to the static generic text only when no real cause was recorded. Both callers feed the same captured errors: `list_available_models_inner` and `provider_diagnostics_inner` no longer discard the per-row `registry.build_from_config` `Err` via `let _ =`; each captures the failing row's `ProviderError` `Display` into a `BTreeMap<String, String>` keyed by `cfg.id` and passes it to the seam. Because diagnostics is DERIVED from this same shared enumeration, the picker's enumeration errors and the Diagnostics panel's per-provider `error` field cannot drift. Concretely, a Gemini keyring resolve failure now surfaces as `ProviderError::Auth` ("no secret found ..."), and a generic-OpenAI missing or invalid `base_url` surfaces as `ProviderError::Other` or a decode error, in place of the old opaque fallback. The invariant from Section 9.1/9.2 holds: every `ProviderError` variant's `Display` is display-safe and names the failure class only, never key material. The v0.7.3 non-fatal behavior is preserved: one failing provider never blanks the list, and the auto-seeded `ollama-local` row still builds and surfaces its model.

### 8.3 Tool / MCP server manager (add/remove tools)

- **Purpose**: add, edit, enable/disable, and remove MCP servers; inspect discovered tools; set permission modes; and view connection health.
- **Components**: `ServerList` (with status badges), `ServerForm` (transport = stdio command/args/env or HTTP/SSE url/headers), `ToolInspector` (per-server discovered tools + schemas), `PermissionModeControl` (Ask/Allow/Deny, per-tool overrides), `ConnectionHealth`.
- **State**: configured servers, per-server connection status and tool lists, form state.
- **IPC commands**: `list_mcp_servers()`, `add_mcp_server(config)`, `update_mcp_server(id, config)`, `remove_mcp_server(id)`, `set_mcp_enabled(id, bool)`, `refresh_mcp_tools(id)`, `set_tool_permission(serverId, toolName, mode)`.
- **Events consumed**: `mcp_state_changed` (connect/disconnect/tool-list refresh), `mcp_error`.
- Adding a server here is the runtime path for the pluggable MCP modules in Section 5.

### 8.4 Agent config editor (create/edit personas)

- **Purpose**: create and edit reusable agent personas: name, system prompt, default route or routing hint, allowed MCP servers/tools, and model parameters.
- **Components**: `PersonaList`, `PersonaEditor` (system prompt editor, parameter controls), `DefaultRoutePicker` (reuses the model selector picker), `AllowedToolsSelector` (which MCP servers/tools this persona may use), `RoutingHintControl` (prefer-local / prefer-quality / prefer-cheap).
- **State**: persona list, editing draft, validation state.
- **IPC commands**: `list_personas()`, `create_persona(persona)`, `update_persona(id, persona)`, `delete_persona(id)`, `assign_persona(conversationId, personaId)`.
- **Events consumed**: `personas_changed`.
- A persona's `default_route`/`routing_hint` feeds `RoutingRequest.persona` (Section 6.1), and `allowed_tool_servers` gates which tools reach the model.

### 8.5 Conversation history / session management

- **Purpose**: browse, search, rename, tag, export, and delete conversations; resume a session with its full state (persona, route pin, tags, enabled tools) restored.
- **Components**: `ConversationList` (sorted/filtered), `SearchBar`, `ConversationContextMenu` (rename, tag, duplicate, export, delete), `SessionSummary` (model usage, cost, tags), `NewConversationButton`.
- **State**: conversation index (id, title, timestamps, tags, last route), search/filter query, selection.
- **IPC commands**: `list_conversations(filter?)`, `create_conversation(init?)`, `rename_conversation(id, title)`, `set_conversation_tags(id, tags)`, `delete_conversation(id)`, `export_conversation(id, format)`, `open_conversation(id)`.
- **Events consumed**: `conversation_updated`, `conversation_created`, `conversation_deleted`.

### 8.6 Message context inputs: attachments, repository, and web search

The composer (Section 8.1) can enrich a single turn with three kinds of bounded context. All three feed the model WITHOUT changing the pipeline: the assembled context rides inside the existing `content` String that `send_message` already accepts, so `run_turn` (Section 7.4) is unchanged. Each context source is cleared after a successful send so it applies to exactly one turn, mirroring the per-message override (Section 8.2).

- **Attach (📎)**: opens the native file dialog via `tauri-plugin-dialog` for text and image files. A text file is read by the `read_text_file` command (non-empty path validated, binary-by-extension rejected, capped at `MAX_ATTACH_BYTES` = 256 KiB, UTF-8 only, cap re-checked after read); an image is read by `read_file_base64` (own 4 MiB cap, MIME guessed from the extension, base64 produced by a hand-rolled encoder in the host-verifiable `domain::file_context` module so no new crate dependency is pulled in). An image is attached ONLY when the model effective for the next turn (the per-message override, else the conversation pin, else the routed or first-available model) advertises the `vision` capability (Section 4.1 `Capabilities.vision`); otherwise a visible non-fatal notice ("This model doesn't support images") is shown and the image is not attached. Attachments render as removable chips. The failure-prone pure helpers (file-name and extension parsing, the directory skip-list, binary-extension detection, MIME guessing, byte-cap checks, base64) live in `domain::file_context` with inline unit tests so they are `cargo test`-covered offline even though `tauri-app` builds only in CI. **Documented limitation**: `run_turn` maps only the text `content` into the provider history, so a vision-gated image is currently included as a labelled note rather than raw image bytes; passing full multimodal image bytes to a vision model is a follow-up. The vision gate and the visible notice are enforced regardless.

- **Repository (📁)**: opens a folder picker, then `list_repo_files` walks the picked directory with an explicit-stack (non-recursive) bounded walk, skipping version-control, dependency, and build-output directories (`.git`, `node_modules`, `target`, `dist`, `build`, `.venv`, `venv`, `__pycache__`, `.next`, `.cache`) and binary-by-extension files, capping the entry count at `MAX_REPO_ENTRIES` = 500 and setting a `truncated` flag when capped. It returns `RepoListing { dir, files: [RepoFileEntry { relPath, byteLen }], truncated }` (camelCase over IPC). The UI presents a checkbox list under a hard total-size cap (128 KiB across selected repo files, selection disabled past the cap with an "X of Y KiB included" signal) and fetches each selected file's contents via `read_text_file`. Selected files are held alongside attachments and are removable.

- **Web search (🌐)**: a pluggable search behind the composer toggle (default OFF). The abstraction lives in the host-verifiable `providers::web_search` module: a `WebSearchProvider` trait (`async search(query, opts) -> Result<Vec<WebSearchResult { title, url, snippet }>, WebSearchError>`) and a `WebSearchKind` enum with **Tavily as the recommended default** (purpose-built for LLM and agent search, a simple JSON API, and a free tier), scaffolded `Brave` and `SerpApi` variants so the pluggability is real, and a **`Custom` variant (the user's OWN endpoint)** that builds a Tavily-JSON-compatible provider rooted at a required user-supplied `base_url`. The Tavily adapter reuses `providers::HttpSseClient` (reqwest) and POSTs to a configurable `base_url` (default `https://api.tavily.com`) at `/search` with `{ api_key, query, max_results }`, mapping `results[]` (`title` / `url` / `content`) into `WebSearchResult`; the configurable base_url is what lets the wiremock tests point it at a local MockServer (request-build, response-parse, error, empty-results, no live network). `WebSearchError` is display-safe and never carries the key. Configuration is stored via `set_web_search_provider` (key stored as an opaque `SecretRef` under the stable keychain handle `web-search`, additive `WebSearchConfig { enabledProvider, maxResults, baseUrl }` persisted on `AppConfig`, where `baseUrl` is the persisted custom endpoint for the `custom` kind, posture-checked like a provider `base_url`), rehydrated display-safe by `get_web_search_config`, and removed by `clear_web_search_provider`. At send time, when the toggle is ON, the composer calls `run_web_search(query)` (which resolves the key core-internally, builds the configured provider against the persisted `baseUrl` when the kind needs a custom endpoint, and returns display-safe `WebSearchResultView`s) and prepends a bounded, clearly-delimited "Web search results" block to the `content`. **Graceful degradation (never a silent hang)**: when web search is unconfigured or the provider fails, `run_web_search` returns a clear `CommandError`, the composer shows a VISIBLE non-fatal `role="status"` notice ("Web search unavailable: &lt;reason&gt;. Sending without web results."), and the plain message is STILL sent. **Live-verification note**: web search needs the user's own key AND live network, so its real behavior is verified on the user's build; in the sandbox it is unit-tested with mocked HTTP only.

- **New commands (each `#[tauri::command]` registered in `generate_handler!`, with an IPC wrapper in `frontend/src/ipc/commands.ts` and a hand-mirrored camelCase TS type in `frontend/src/types/index.ts`)**:
  - `read_text_file(path)` -> `FileContentView { path, name, byteLen, text }`.
  - `read_file_base64(path)` -> `FileBinaryView { path, name, mimeType, base64, byteLen }`.
  - `list_repo_files(dir)` -> `RepoListing { dir, files: [RepoFileEntry { relPath, byteLen }], truncated }`.
  - `set_web_search_provider(kind, apiKey, maxResults?, baseUrl?)` -> `WebSearchConfigView { kind, hasApiKey, maxResults, baseUrl }` (`baseUrl` is required for the `custom` kind and null otherwise).
  - `get_web_search_config()` -> `WebSearchConfigView | null`.
  - `clear_web_search_provider()` -> `()`.
  - `run_web_search(query)` -> `WebSearchResultView[] { title, url, snippet }`.
  - `add_network_peer(baseUrl, label?, apiKey?)` -> `NetworkPeerView { id, label, baseUrl, hasApiKey, warning }` (Section 9.3).
  - `list_network_peers()` -> `NetworkPeerView[]` (Section 9.3).
  - `remove_network_peer(id)` -> `()` (Section 9.3).
  - `set_model_sharing(enabled, port?)` -> `ModelSharingView { enabled, port, status }` (Section 9.3).
  - `get_model_sharing()` -> `ModelSharingView` (Section 9.3).
  - `discover_network_peers()` -> `DiscoveredPeerView[] { label, baseUrl }` (Section 9.3).

---

## 9. Security Considerations

### 9.1 API key storage

- **OS keychain via the `keyring` crate** (macOS Keychain, Windows Credential Manager, Linux Secret Service). Keys are stored under a namespaced service id and referenced elsewhere only by a `SecretRef` handle.
- **Never in plaintext config**: `ProviderConfig` and the SQLite store hold only `SecretRef`, never the key material.
- **Never in the frontend**: keys are read inside the core at request time and attached to outbound provider requests there. No command returns a secret to the webview.
- **Entry and rotation**: keys are entered through a dedicated settings flow whose command writes straight to the keystore and returns only a `SecretRef`. Rotation replaces the keystore entry; the reference is stable. For cloud providers this flow is `set_cloud_provider`, which stores the optional key via the `SecretStore` and persists a real `ProviderConfig` carrying only the resulting `SecretRef` as `api_key_ref` (with `list_cloud_providers` / `clear_cloud_provider` companions), mirroring `set_local_runtime`; the provider KIND is chosen in the UI, and the "Kiro" choice maps to `ProviderKind::GenericOpenAI` with a required OpenAI-compatible `base_url` rather than adding a new `ProviderKind`. The row is what makes the provider enumerable; the key itself never leaves the keychain. The web-search key (Section 8.6) and any LAN peer key (Section 9.3.1) use this same `SecretStore.store` path under a stable handle, so they inherit the same by-reference posture.

- **Known issue / follow-up (secret persistence across sessions)**: production uses `KeyringSecretStore` (the real OS keychain) and the write path stores via `SecretStore::store`, so the write path IS the keychain, not an in-memory store. The observed "no secret found for handle gemini-cloud" diagnostic is therefore NOT caused by an in-memory store in production; the likely causes to investigate separately are a store/resolve handle mismatch or a keychain entry evicted or cleared between sessions (tests using `InMemorySecretStore` can also mask a prod-only keyring backend failure on a given OS). This is recorded as a follow-up and is NOT fixed in this task; the new web-search and peer keys share whatever the eventual fix is because they use the identical store path.

### 9.2 IPC boundary

- **Command allowlist**: only explicitly registered `#[tauri::command]` handlers are callable. There is no generic passthrough to the core.
- **Capability-scoped permissions**: Tauri capability files (`tauri-app/capabilities/`) grant the webview only the specific commands and core plugins it needs, following least privilege. Filesystem, shell, and network capabilities are not granted broadly to the frontend.
- **Input validation**: every command validates and deserializes its arguments (types, id existence, bounds, enum membership) before touching the core. Invalid input is rejected with a structured error, never partially applied.
- **No secrets across the bridge**: as in 9.1, secrets never cross IPC. Route badges and model lists carry labels, not credentials.
- **Event hygiene**: events emitted to the frontend carry only display-safe data (deltas, statuses, rationales), never keys or raw provider responses containing credentials.
- **Display-safe enumeration errors**: the per-provider enumeration errors returned by `list_available_models` as `ProviderEnumerationError { providerId, message }` (Section 8.2) are display-safe by construction. Their `message` derives from `ProviderError`'s `Display`, which names the failure class (transport, HTTP status, decode, unsupported capability, auth, or other) but never includes key material or a raw provider response body, so surfacing an enumeration failure in the picker or settings cannot leak a credential.

### 9.3 LM Studio and local network calls

- **Loopback binding**: local inference targets `http://localhost:1234/v1` (or a user-set loopback URL). The default and validation encourage `localhost`/`127.0.0.1` so traffic stays on the machine.
- **Plaintext vs TLS tradeoff**: LM Studio's local server is typically plaintext HTTP on loopback. Because traffic never leaves the host, plaintext on loopback is acceptable; if a user points the base URL at a non-loopback host, the UI warns and recommends TLS, since plaintext off-host would expose prompts on the network.
- **SSRF and loopback considerations**: user-supplied base URLs (for LM Studio and any generic OpenAI-compatible provider) are validated. The app distinguishes loopback from remote hosts, warns on remote non-TLS targets, and blocks obviously dangerous internal targets where feasible. Because the core (not the frontend) makes these calls, the webview cannot be tricked into arbitrary requests: the set of reachable endpoints is limited to configured providers and MCP servers.
- **Validation on the write path**: this validation (`check_provider_base_url`) now runs when a Local Runtimes base URL is saved through the `set_local_runtime` command, which persists it into a `ProviderConfig`. A blocked URL rejects the save (persisting nothing) and an accepted non-loopback plaintext URL returns a non-blocking warning; the seam is no longer dead code awaiting a caller.
- **Advisory base_url shape guidance for GenericOpenAI (non-blocking)**: distinct from the BLOCKING link-local/metadata-IP rule (which remains the only case that rejects a save), a new ADVISORY check `advise_generic_openai_base_url` (merged into the returned view via `merge_warnings`) warns when a `GenericOpenAI` base URL clearly is not an API root. It flags URLs that look like a web/session address (containing `/session/`) or a model endpoint (containing `:generateContent`) and surfaces the hint through the returned view's `warning` field WITHOUT blocking the save. It is wired into both `set_cloud_provider_inner` and `set_local_runtime_inner`. The UX guidance is explicit in the settings surfaces (`ProviderKeysSection.tsx`, `LocalRuntimesSection.tsx`) via clearer placeholders and inline help plus matching non-blocking client advisories: the "Kiro" choice is a generic OpenAI-compatible endpoint that expects an API base URL like `https://host/v1`, and Gemini should be configured via the Gemini kind rather than pasted as a generic OpenAI-compatible runtime (its `:generateContent` model endpoint is not an OpenAI `/models` root).
- **No implicit key on local**: the LM Studio adapter does not require or send an API key by default, avoiding accidental credential exposure to a local process.

- **Web-search key by reference (Section 9.1 invariant)**: the web-search API key (Section 8.6) follows the same keychain-by-reference posture as provider keys. The plaintext key flows IN via `set_web_search_provider`, only an opaque `SecretRef` is stored under the stable handle `web-search` (the persisted `WebSearchConfig` carries `enabledProvider` / `maxResults` / an optional display-safe custom `baseUrl` and never the key), and the key is resolved core-internally at search time inside `run_web_search`. No command returns the key: `get_web_search_config` reports only `kind` / `hasApiKey` / `maxResults` / `baseUrl`, and `WebSearchError` / `WebSearchResultView` carry no key material. File reads for attachments and repository context (Section 8.6) are bounded and display-safe: per-file and total byte caps are enforced, binary-by-extension files are rejected, non-UTF-8 text is rejected, and the returned views carry only display fields.

### 9.3.1 Local network (LAN) model sharing

LAN model sharing (FEAT-006) lets a user consume models served by another machine on the local network, and optionally re-expose this machine's local models to peers. It is incorporated per explicit user request; it was not previously implemented (the repository had only Ollama loopback discovery). It has three parts, and its live cross-machine behavior is user-only because the build sandbox has no cross-machine network.

- **Consume a peer (solid + offline-testable)**: a LAN peer is persisted as an ordinary OpenAI-compatible `ProviderConfig` row of kind `GenericOpenAI` with a stable id `network-peer-<hash(base_url)>` (the `network-peer-` prefix marks it for `is_network_peer` and the picker's Network group in Section 8.2). Because it is a normal provider row, it enumerates and routes through the EXISTING path (`list_available_models_inner` and the `send_message` registry build both pick it up) with no pipeline change. The id hash makes re-adding the same base_url an idempotent upsert, and the prefix (distinct from the fixed single-slot `generic-openai-local` / `generic-openai-cloud` ids) lets many peers coexist alongside a local generic runtime and a cloud provider. The optional label is persisted in the row's `extra` JSON (display-only, never secret). `add_network_peer` validates `baseUrl` through the same posture as `check_provider_base_url`: a blocked link-local or metadata target REJECTS the call, while an accepted non-loopback plaintext target returns the existing non-blocking TLS warning in `NetworkPeerView.warning`. An optional peer key flows in and is stored as a `SecretRef` under the peer id (mirroring `set_cloud_provider`), never returned. `add_network_peer` and `remove_network_peer` emit `CoreEvent::ProvidersChanged` so peer models enumerate immediately. The consume path is proven offline by a `providers` wiremock test showing an OpenAI-compatible endpoint at an arbitrary base_url enumerating models via the shared native adapter, plus `_inner` tests for add / list / remove.

- **Privacy decision (recorded, off-host)**: a LAN peer is OFF-HOST, so it must NOT satisfy a `LocalOnly` / `Confidential` privacy tag. `local_provider_ids` admits a `GenericOpenAI` row into the locality set ONLY when its base_url is loopback, so a peer (a `GenericOpenAI` row at a non-loopback base_url) is correctly EXCLUDED from the locality set and a `LocalOnly` / `Confidential` conversation NEVER routes to a peer. This is pinned by a Rust test (`network_peer_is_excluded_from_local_provider_ids`) and preserves the loopback-local privacy semantics of Section 9.3 unchanged.

- **Share / serve (scaffolded seam, live use user-only)**: `providers::net_share::ModelShareServer` is the seam for re-exposing this machine's local models to peers. `render_models_response` is a pure, offline-tested function that produces the OpenAI `GET /v1/models` body a peer consumes (`{ object: list, data: [{ id, object: model, owned_by: agent-harbor }] }`). `try_bind` attempts to bind the configured port and returns a VISIBLE `ShareServerStatus` (`Running { port }` or `Unavailable { port, reason }`) so a taken port degrades to a non-fatal message rather than a silent hang. The shared model set is gathered so it keeps ONLY provably-on-host `local_provider_ids`, so a peer is never re-shared onward and cloud models are never exposed. `set_model_sharing(enabled, port?)` persists the additive `AppConfig.model_sharing` (`ModelSharingConfig { enabled (default false), port (default `DEFAULT_MODEL_SHARING_PORT` = 11435, chosen clear of Ollama 11434 and LM Studio 1234) }`, forward-compatible like `web_search`) and, on enable, attempts a visible bind; `get_model_sharing` rehydrates without re-binding. **Security posture**: sharing binds to the LAN and re-exposes local models, so it is OFF by default and the Settings UI states plainly that enabling it exposes this machine's local models to the local network. The full serving loop (no new server framework was added) and real LAN binding and reachability are user-only.

- **Discovery (scaffolded seam, live use user-only)**: `providers::net_share::PeerDiscovery` is a trait with `StubPeerDiscovery` (an mDNS/UDP-broadcast placeholder) that returns an EMPTY list without error in the sandbox. `discover_network_peers` runs discovery with a bounded timeout and maps to display-safe `DiscoveredPeerView`s; finding nothing (or discovery being unavailable) returns an empty list, and the Settings UI shows a non-fatal "no peers found / discovery unavailable" notice with a manual-entry fallback. No mDNS crate dependency was added; wiring a concrete mDNS/UDP probe is a documented follow-up and live discovery is user-only.

- **Settings surface**: a Settings "Network Sharing" section (`NetworkSharingSection`) manages configured peers (Add with base URL, optional label, optional key; Remove; rehydrate from `list_network_peers`, surfacing the base-url warning), offers "Discover peers" with one-click Add, and hosts the "Share my local models on the network" toggle (with an optional port and the current status from `get_model_sharing` plus the security note that this exposes local models to the LAN). A peer's optional key follows the Section 9.1 keychain-by-reference invariant (plaintext in, only a `SecretRef` stored under the peer id, resolved core-internally, never crossing IPC); `NetworkPeerView` / `ModelSharingView` / `DiscoveredPeerView` carry no key material.

### 9.4 MCP server sandboxing and permission prompts

- **Explicit permission model**: each MCP server has a permission mode (`Ask`/`Allow`/`Deny`) with optional per-tool overrides (Section 5.6). In `Ask` mode, tool invocation triggers a `permission_requested` event and blocks until the user decides.
- **Process isolation**: stdio servers run as child processes with a controlled environment (explicit `env`, no inherited secrets) and are terminated on disable/removal/exit. HTTP/SSE servers are reached only at their configured URL with configured headers.
- **Argument validation**: tool-call arguments are validated against the server-provided input schema before invocation, preventing malformed or injected payloads from reaching a tool.
- **Least exposure to models**: only tools from enabled servers allowed by the active persona/conversation are exposed to the model, limiting what a model can attempt to invoke.

---

## 10. Extensibility Strategy

The architecture is built around traits + registries so the three most common extensions add code without modifying the core pipeline.

### 10.1 Adding a new provider adapter

1. Implement `ChatProvider` for the new backend (request/response translation, streaming, capabilities) in `providers/src/adapters/`.
2. Implement a `ProviderFactory` for its `ProviderKind`.
3. Register the factory in the `ProviderRegistry` at startup.
4. The provider becomes configurable (baseURL, key ref, extras) and immediately usable by routing and the UI. No changes to `orchestrator-core` or `routing` are needed. Any OpenAI-compatible endpoint needs no new adapter at all: it is added at runtime as a `GenericOpenAI` `ProviderConfig`.

### 10.2 Adding a new MCP server

- Runtime, no code: the user adds a server through the Tool/MCP Server Manager (Section 8.3) by specifying its transport (stdio command or HTTP/SSE URL). The client handles handshake, discovery, and exposure automatically. This is the expected path since MCP servers are pluggable modules.

### 10.3 Adding a new routing policy

1. Implement `RoutingPolicy::decide` in `routing/src/policies/`.
2. Register it in the `PolicyRegistry`.
3. Select it as the active automatic policy in settings.
4. Manual-override precedence (Section 6.3) is applied by the engine wrapper, so a custom policy only needs to implement the automatic decision. The pipeline is untouched.

### 10.4 Versioned config schema and forward compatibility

- **Schema version**: `app_config` and persisted config records carry a `schema_version`. On startup, migrations upgrade older versions forward; unknown newer fields are preserved where possible rather than dropped.
- **Additive change bias**: new capabilities are added as optional fields with sensible defaults so older configs keep working and newer configs degrade gracefully on older builds.
- **Stable trait contracts**: `ChatProvider`, `RoutingPolicy`, and the MCP transport enums are the extension seams. Changes to them are versioned and documented, and adapters/policies target a stable trait surface.
- **Core independence from the shell**: because the orchestration crates do not depend on Tauri, the desktop shell can be upgraded or replaced (see Section 3.1) without breaking providers, routing, MCP, or persistence.

---

## Appendix A: Coverage summary

| Requirement | Where addressed |
| --- | --- |
| Orchestrator calls LM Studio + cloud, acts as MCP client | Sections 1, 2, 5 |
| Kiro/ACP is development-time only, not a runtime dependency | Section 1.3 (Non-goals) |
| High-level architecture + end-to-end data flow | Section 2 |
| Cargo workspace + directory structure, Tauri justification | Section 3 |
| OpenAI-compatible base contract + six provider mappings | Section 4 |
| MCP client: transports, lifecycle, tools, permissions | Section 5 |
| Routing: automatic (complexity/privacy/cost) + manual override + precedence | Section 6 |
| Session/state models, persistence, concurrency | Section 7 |
| Five UI surfaces individually specified | Section 8 (8.1 through 8.5) |
| Message context inputs: attachments, repository, web search | Section 8.6 |
| Local network (LAN) model sharing: consume, share, discovery | Sections 8.2, 9.3.1 |
| Security: keys, IPC, LM Studio local network, MCP sandboxing | Section 9 |
| Extensibility: new provider/MCP/policy without core changes | Section 10 |
