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
| **Ollama (local)** | OpenAI-compatible (native) chat + native model discovery | Direct for chat/streaming (same code path as OpenAI adapter, different base URL); `list_models` overridden to call Ollama's native `GET /api/tags` at the server root (not `/v1`) | `base_url` default `http://localhost:11434/v1`, no key; treated as a local provider like LM Studio |
| **Embedded (local, in-process)** | llama.cpp GGUF inference in-process (no HTTP) | Direct: the `engine` crate's `EmbeddedEngine` implements the same `ChatProvider` contract and emits OpenAI-shaped `ChatDelta`s; no shared HTTP client is involved. `list_models` returns the imported local `.gguf` models | no `base_url`/key (runs in-process); `base_url`, when set, names the local models directory; treated as a local provider, seeded at zero token price |
| **Azure OpenAI** | OpenAI-compatible with deployment routing | Near-direct: rewrite path to `/openai/deployments/{deployment}/chat/completions`, add `api-version` query and `api-key` header | `extra.deployment`, `extra.api_version`, `base_url` = resource endpoint |
| **Anthropic** | Messages API (`/v1/messages`) | Translation shim: map roles, split system prompt to top-level `system`, map tool schema to Anthropic `tools`, translate SSE deltas | `base_url` default `https://api.anthropic.com`, `anthropic-version` header |
| **Google Gemini** | `generateContent` / `streamGenerateContent` | Translation shim: map messages to `contents`/`parts`, map tools to `functionDeclarations`, translate streamed chunks | `extra.project`, key or OAuth per config |
| **AWS Bedrock** | Provider-specific model bodies, SigV4-signed | Translation shim: SigV4 request signing, per-model body shaping (Anthropic-on-Bedrock, Titan, etc.), translate event stream | `extra.region`, AWS credential resolution via keystore/credential provider |

Adapters fall into two families:

- **Native OpenAI-compatible** (OpenAI, LM Studio, Ollama, Azure OpenAI, and any user-supplied `GenericOpenAI` endpoint): reuse a shared HTTP + SSE client with thin differences (path/header rewriting for Azure; Ollama additionally overrides model discovery to its native `GET /api/tags` endpoint at the server root).
- **Translation shims** (Anthropic Messages API, Gemini `generateContent`, Bedrock SigV4 and per-model bodies): implement request/response translation and stream normalization so the rest of the system sees the OpenAI-compatible shape.

Because LM Studio speaks the native format, local inference uses the exact same code path as OpenAI with only `base_url` differing. This is deliberate: it keeps the local path simple and reliable. Ollama is handled the same way for chat and streaming (OpenAI-compatible at `http://localhost:11434/v1`), with one addition: because Ollama does not surface installed models under the OpenAI `/v1/models` path, its adapter overrides `list_models` to call the native `GET /api/tags` endpoint (rooted at the server root, not `/v1`, and returning `{"models":[{"name":...}]}`). Like LM Studio, Ollama is classified as a local provider for the routing privacy gate and seeded at zero token price, so discovered Ollama models surface under Local in the model picker.

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

The frontend holds a cache for responsiveness but treats the core as authoritative. Core events (`message_delta`, `message_complete`, `conversation_updated`, `mcp_state_changed`, `permission_requested`) keep the cache in sync.

### 7.4 Streaming and token state

- A message begins `Pending`, transitions to `Streaming` as `message_delta` events arrive, then `Complete` (or `Error`).
- Token usage and the final route metadata are attached on `message_complete` and persisted.
- The frontend renders partial content live; on completion it reconciles with the persisted message.

### 7.5 Concurrency

- The core is async (Tokio). Each conversation's active turn runs as an independent task, so **multiple conversations can stream concurrently** from the same or different providers.
- Provider adapters and MCP server handles are `Arc`-shared and internally safe for concurrent use; per-conversation state is isolated.
- Persistence writes are serialized through the connection pool; reads are concurrent. A per-conversation lock prevents interleaved writes within a single conversation while allowing parallelism across conversations.

---

## 8. UI Surfaces

All five surfaces are React feature modules. Each calls the core through typed IPC wrappers (`invoke`) and subscribes to typed Tauri events. No surface accesses secrets or provider endpoints directly.

### 8.1 Chat interface

- **Purpose**: the primary conversation view: message list, streaming assistant output, tool-call visualization, composer, and per-message route badge (which provider/model answered and why).
- **Components**: `MessageList`, `MessageBubble` (with tool-call/tool-result rendering), `StreamingIndicator`, `Composer`, `RouteBadge`, `PermissionPrompt` (modal for MCP `ask` mode).
- **State**: active conversation id, message list (hydrated + live deltas), composer draft, pending permission requests.
- **IPC commands**: `send_message(conversationId, content, overrideRoute?)`, `stop_generation(conversationId)`, `resolve_permission(requestId, decision)`, `get_messages(conversationId)`.
- **Events consumed**: `message_delta`, `message_complete`, `message_error`, `permission_requested`.

### 8.2 Model selector (local vs cloud, per conversation)

- **Purpose**: choose how the active conversation is routed. Exposes three modes: Automatic (default), a per-conversation pin, and a one-off per-message override. Clearly groups **Local** (LM Studio) vs **Cloud** providers/models and shows availability and rough cost.
- **Components**: `RoutingModeToggle` (Auto / Manual), `ProviderModelPicker` (grouped Local vs Cloud with capability and price hints), `PerMessageOverrideControl` in the composer, `AutoRationaleTooltip` (shows why Auto picked a model).
- **State**: current conversation routing mode, selected pin (if any), transient per-message override, list of available provider/model options with capabilities.
- **IPC commands**: `list_available_models()` (provider + model + capabilities + price), `set_conversation_route(conversationId, route | null)`, `set_message_override(conversationId, route | null)`, `get_route_explanation(conversationId)`.
- **Events consumed**: `providers_changed` (availability updates), `conversation_updated`.
- Ties directly to Section 6: a pin sets `conversation_pref`; a one-off sets `manual_override`; clearing both returns to `Automatic`.
- **Per-message override ownership**: the transient per-message override is a single value owned by the active conversation's routing store (`state/conversations.ts`), not by either surface in isolation. The `PerMessageOverrideControl` rendered inside the composer only reads and writes that shared value; it does not hold its own copy. When the chat surface's `Composer` fires `send_message` (Section 8.1), it reads the current override from that store and passes it as the `overrideRoute?` argument, then the store clears it after the send so the override applies to exactly one message. This makes the override payload flow unambiguous: the model selector control edits it, the shared store owns it, and the chat surface's send call carries it.

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

---

## 9. Security Considerations

### 9.1 API key storage

- **OS keychain via the `keyring` crate** (macOS Keychain, Windows Credential Manager, Linux Secret Service). Keys are stored under a namespaced service id and referenced elsewhere only by a `SecretRef` handle.
- **Never in plaintext config**: `ProviderConfig` and the SQLite store hold only `SecretRef`, never the key material.
- **Never in the frontend**: keys are read inside the core at request time and attached to outbound provider requests there. No command returns a secret to the webview.
- **Entry and rotation**: keys are entered through a dedicated settings flow whose `set_provider_secret(providerId, secret)` command writes straight to the keystore and returns only a `SecretRef`. Rotation replaces the keystore entry; the reference is stable.

### 9.2 IPC boundary

- **Command allowlist**: only explicitly registered `#[tauri::command]` handlers are callable. There is no generic passthrough to the core.
- **Capability-scoped permissions**: Tauri capability files (`tauri-app/capabilities/`) grant the webview only the specific commands and core plugins it needs, following least privilege. Filesystem, shell, and network capabilities are not granted broadly to the frontend.
- **Input validation**: every command validates and deserializes its arguments (types, id existence, bounds, enum membership) before touching the core. Invalid input is rejected with a structured error, never partially applied.
- **No secrets across the bridge**: as in 9.1, secrets never cross IPC. Route badges and model lists carry labels, not credentials.
- **Event hygiene**: events emitted to the frontend carry only display-safe data (deltas, statuses, rationales), never keys or raw provider responses containing credentials.

### 9.3 LM Studio and local network calls

- **Loopback binding**: local inference targets `http://localhost:1234/v1` (or a user-set loopback URL). The default and validation encourage `localhost`/`127.0.0.1` so traffic stays on the machine.
- **Plaintext vs TLS tradeoff**: LM Studio's local server is typically plaintext HTTP on loopback. Because traffic never leaves the host, plaintext on loopback is acceptable; if a user points the base URL at a non-loopback host, the UI warns and recommends TLS, since plaintext off-host would expose prompts on the network.
- **SSRF and loopback considerations**: user-supplied base URLs (for LM Studio and any generic OpenAI-compatible provider) are validated. The app distinguishes loopback from remote hosts, warns on remote non-TLS targets, and blocks obviously dangerous internal targets where feasible. Because the core (not the frontend) makes these calls, the webview cannot be tricked into arbitrary requests: the set of reachable endpoints is limited to configured providers and MCP servers.
- **Validation on the write path**: this validation (`check_provider_base_url`) now runs when a Local Runtimes base URL is saved through the `set_local_runtime` command, which persists it into a `ProviderConfig`. A blocked URL rejects the save (persisting nothing) and an accepted non-loopback plaintext URL returns a non-blocking warning; the seam is no longer dead code awaiting a caller.
- **No implicit key on local**: the LM Studio adapter does not require or send an API key by default, avoiding accidental credential exposure to a local process.

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
| Security: keys, IPC, LM Studio local network, MCP sandboxing | Section 9 |
| Extensibility: new provider/MCP/policy without core changes | Section 10 |
