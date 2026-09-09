# Hybrid Orchestrator: Phased Implementation Task List

Status: Draft v1
Applies to: The Hybrid Orchestrator desktop application specified in `architecture.md` (this directory).

This document breaks the architecture into an ordered, phased build plan a team can execute. It references the exact crates, traits, modules, and UI surfaces defined in `architecture.md`. Read that document first: the crate layout is Section 3, the provider contract is Section 4, the MCP client is Section 5, the routing engine is Section 6, session/state is Section 7, the five UI surfaces are Section 8, security is Section 9, and extensibility is Section 10.

## How to read this plan

- Phases are ordered by dependency. A phase begins only after its listed dependencies are green.
- Each phase lists concrete tasks, the deliverable, its dependencies, which tasks can run in parallel, and a verification/acceptance check tied to real build and test commands (`cargo build`, `cargo test`, `cargo clippy`, `vitest`, `tauri build`), not text or grep checks.
- Parallelizable work is called out per phase under "Parallelization".
- Task ids use the form `P<phase>.<n>` so later phases can reference earlier tasks.

### Tooling baseline (from architecture Section 3.4)

- Rust core: `cargo build`, `cargo test`, `cargo clippy` per crate in the Cargo workspace.
- Frontend: Vite plus npm, with `vitest` and React Testing Library for component tests.
- App shell: Tauri CLI (`tauri dev`, `tauri build`) to bundle both sides into an installer.

### Development-time note on Kiro

Kiro, if used at all, is a development-time authoring and scaffolding tool only. The shipped application has no runtime dependency on Kiro or its ACP wire protocol, does not spawn the Kiro CLI, and does not speak ACP to any process. Nothing in this plan introduces such a dependency. All runtime agent communication goes through the provider adapters (Section 4) and the MCP client (Section 5).

---

## Phase 0: Scaffolding and green baseline

Goal: stand up the full skeleton (Tauri app, Cargo workspace with every crate from architecture Section 3.2, React/TypeScript frontend, CI) so that a baseline build and test run is green before any feature work begins.

Dependencies: none. This is the entry phase.

### Tasks

- **P0.1 Initialize the Cargo workspace.** Create the `hybrid-orchestrator/` workspace manifest (`Cargo.toml`) with member crates: `orchestrator-core`, `providers`, `mcp-client`, `routing`, `persistence`, `secrets`, and `tauri-app`. Each crate compiles as an empty library (or binary for `tauri-app`) with a placeholder `lib.rs`/`main.rs`.
- **P0.2 Establish crate dependency direction.** Wire dependencies so `tauri-app` depends on the libraries and `orchestrator-core` depends on `providers`, `mcp-client`, `routing`, `persistence`, and `secrets` through their crate boundaries. Libraries must not depend on `tauri-app`. Add one trivial compile-time smoke test per crate.
- **P0.3 Scaffold the Tauri shell.** Add `tauri-app/tauri.conf.json`, `capabilities/default.json`, `src/main.rs`, `src/commands.rs`, `src/state.rs`, `src/events.rs`. Expose one placeholder `#[tauri::command]` (for example `app_version`) so the IPC bridge is exercised end to end.
- **P0.4 Scaffold the React/TypeScript frontend.** Create `frontend/` with Vite, `package.json`, `vite.config.ts`, `src/main.tsx`, `src/app.tsx`, and empty feature folders `chat/`, `model-selector/`, `tool-manager/`, `agent-editor/`, `history/`. Add typed IPC stubs `src/ipc/commands.ts` and `src/ipc/events.ts` that call the placeholder command.
- **P0.5 Baseline test harness.** Add at least one passing `cargo test` per crate and one passing `vitest` test in the frontend (for example, the app component renders). Confirm `tauri dev` launches and the placeholder command round-trips.
- **P0.6 CI pipeline.** Add CI that runs `cargo build`, `cargo test`, `cargo clippy`, `vitest`, and a `tauri build` on a matrix of target OSes. CI must be green on the scaffold.
- **P0.7 Repository hygiene.** Add formatting config (`rustfmt`, `prettier`/`eslint`), a README with build instructions, and a lockfile policy.

### Deliverable

A buildable monorepo skeleton: the full crate layout from Section 3.2, a launching Tauri shell, a rendering React frontend, and green CI.

### Parallelization

- P0.1 first (it defines the workspace), then P0.2, P0.3, P0.4, and P0.7 can proceed in parallel.
- P0.5 depends on P0.2 through P0.4. P0.6 depends on P0.5.

### Verification / acceptance

- `cargo build` and `cargo test` pass across the whole workspace.
- `cargo clippy` reports no errors.
- `vitest` passes in `frontend/`.
- `tauri build` produces an installer, and `tauri dev` launches a window that successfully invokes the placeholder command.
- CI is green on all matrix targets.

---

## Phase 1: Orchestration core foundations

Goal: build the durable spine, that is the session/state data models, SQLite persistence, the Tauri command/event skeleton, and the secrets keystore, so later phases have real storage, real IPC, and real credential handling to build on.

Dependencies: Phase 0.

### Tasks

- **P1.1 Domain models (`orchestrator-core`).** Implement the data models from architecture Section 7.1: `Conversation`, `Message` (with `MessageContent`, `Role`, `MessageStatus`, `RouteMetadata`, `TokenUsage`), `AgentPersona`, `McpServerConfig`, and `ProviderConfig`. Add serde DTOs plus their mirrored TypeScript types in `frontend/src/types/`.
- **P1.2 Persistence layer (`persistence`).** Implement `Db` (an `sqlx` SQLite pool), versioned migrations, and repositories (`ConversationRepo`, `PersonaRepo`, and repos for messages, `mcp_servers`, and `providers`) per Section 7.2. Add `AppConfig` load/save with a `schema_version` field (Section 10.4). Secrets are never stored here, only `SecretRef` handles.
- **P1.3 Secrets keystore (`secrets`).** Implement the `SecretStore` trait and a `keyring`-backed implementation per Section 9.1, returning and resolving `SecretRef` handles. No API should ever return raw key material.
- **P1.4 Session manager (`orchestrator-core`).** Implement `SessionManager` over the repositories: create/list/rename/tag/delete conversations, append messages, assign personas, and manage per-conversation route pins and privacy tags. Include the per-conversation write lock from Section 7.5.
- **P1.5 Core events (`orchestrator-core`).** Define `CoreEvent` variants used across the app: `message_delta`, `message_complete`, `message_error`, `conversation_updated`, `conversation_created`, `conversation_deleted`, `mcp_state_changed`, `mcp_error`, `permission_requested`, `providers_changed`, `personas_changed`.
- **P1.6 Tauri command/event skeleton (`tauri-app`).** Implement `AppState` (Arc-wrapped subsystems) in `state.rs`, the core-event to Tauri `emit` bridge in `events.rs`, and validated command handlers in `commands.rs` for the session/persona surface (for example `list_conversations`, `create_conversation`, `rename_conversation`, `set_conversation_tags`, `delete_conversation`, `list_personas`, `create_persona`, `update_persona`, `delete_persona`). Enforce argument validation per Section 9.2.
- **P1.7 Secret entry command.** Implement `set_provider_secret(providerId, secret)` that writes straight to the keystore and returns only a `SecretRef` (Section 9.1). Confirm no command path returns a secret to the webview.

### Deliverable

A running core that persists conversations, messages, and personas to SQLite, stores API keys in the OS keychain by reference, and exposes a validated Tauri command/event surface for session and persona management.

### Parallelization

- P1.1 first. Then P1.2, P1.3, and P1.5 can run in parallel (models exist; each is a distinct crate).
- P1.4 depends on P1.1 and P1.2. P1.6 depends on P1.4 and P1.5. P1.7 depends on P1.3.

### Verification / acceptance

- `cargo test` in `persistence` covers migrations and CRUD round-trips against a temp SQLite file.
- `cargo test` in `secrets` verifies store/resolve of a `SecretRef` with no raw-key leakage (a test asserting no getter returns the plaintext).
- `cargo test` in `orchestrator-core` covers session lifecycle (create, append, persona assign, per-conversation lock).
- `tauri dev` demonstrates creating a conversation and a persona from a temporary test panel, with the data surviving an app restart.
- Whole-workspace `cargo build`, `cargo clippy`, and `vitest` remain green.

---

## Phase 2: Provider layer

Goal: implement the OpenAI-compatible `ChatProvider` contract and `ProviderRegistry`, then the native-compatible adapters first (LM Studio and OpenAI), then Azure OpenAI, then the translation shims (Anthropic, Gemini, Bedrock). This order front-loads the widest common denominator (Section 4.1) and defers the hardest translation work.

Dependencies: Phase 1 (needs `secrets` for `SecretRef` resolution and `persistence` for `ProviderConfig` rows).

### Tasks

- **P2.1 Provider contract (`providers/src/contract.rs`).** Define `ChatProvider` (async trait), `ChatRequest`, `ChatResponse`, `ChatDelta`, `ChatMessage`, `ToolSpec`, `Capabilities`, `ModelInfo`, and `ProviderError` per Section 4.1.
- **P2.2 Provider registry (`providers/src/registry.rs`).** Implement `ProviderRegistry`, `ProviderFactory`, `ProviderKind`, and `ProviderConfig` with a swappable `base_url` and `api_key_ref` (Section 4.2 and 4.5). Support building instances from persisted `ProviderConfig` rows.
- **P2.3 Shared HTTP/SSE client and capability negotiation (`providers/src/capability.rs`).** Build the reusable HTTP plus SSE streaming client used by all native-compatible adapters, and implement the capability descriptors and negotiation from Section 4.4.
- **P2.4 OpenAI adapter (`providers/src/adapters/openai.rs`).** Native pass-through against `https://api.openai.com/v1`. Streaming and non-streaming.
- **P2.5 LM Studio adapter (`providers/src/adapters/lmstudio.rs`).** Same code path as OpenAI with `base_url` default `http://localhost:1234/v1` and no key by default (Section 4.3). Also expose the `generic_openai.rs` path for any user-supplied OpenAI-compatible endpoint.
- **P2.6 Azure OpenAI adapter (`providers/src/adapters/azure_openai.rs`).** Near-direct: rewrite the path to `/openai/deployments/{deployment}/chat/completions`, add the `api-version` query and `api-key` header, read `extra.deployment` and `extra.api_version` (Section 4.3).
- **P2.7 Anthropic shim (`providers/src/adapters/anthropic.rs`).** Translation shim for the Messages API: split the system prompt to top-level `system`, map roles and tool schemas, normalize SSE deltas.
- **P2.8 Gemini shim (`providers/src/adapters/gemini.rs`).** Translation shim mapping messages to `contents`/`parts`, tools to `functionDeclarations`, and normalizing streamed chunks.
- **P2.9 Bedrock shim (`providers/src/adapters/bedrock.rs`).** Translation shim with SigV4 signing, per-model body shaping, and event-stream normalization; region via `extra.region`.
- **P2.10 Registry wiring and model listing command.** Register all built-in factories at startup and implement `list_available_models()` returning provider, model, capabilities, and price so the model selector (Phase 5) has data.

### Deliverable

A working provider layer where any configured provider (six named targets plus generic OpenAI-compatible endpoints) can list models and run streaming and non-streaming chat behind one trait, with credentials resolved only inside the core.

### Parallelization

- P2.1, then P2.2 and P2.3 in parallel.
- P2.4 and P2.5 in parallel (they share the native path). P2.6 follows once the native client is stable.
- P2.7, P2.8, and P2.9 (the three shims) are independent and can run fully in parallel across three developers once P2.1 through P2.3 land.
- P2.10 depends on all adapters being registrable.

### Verification / acceptance

- `cargo test` in `providers` covers per-adapter request translation and stream normalization using recorded/mock HTTP responses (no live network required in CI).
- A local integration check runs the LM Studio adapter against a running LM Studio server on `http://localhost:1234/v1` and returns a streamed completion (documented as a manual/optional gated test, not required in CI).
- `cargo test` verifies capability negotiation gates tool and streaming features correctly (Section 4.4).
- Whole-workspace `cargo build` and `cargo clippy` stay green.

---

## Phase 3: MCP client

Goal: implement the built-in MCP client: stdio and HTTP/SSE transports, server lifecycle, tool discovery, and the function-calling bridge that feeds discovered tools into provider requests and returns tool results into the conversation (architecture Section 5).

Dependencies: Phase 2 (tools are attached to `ChatRequest.tools`, so the provider contract must exist) and Phase 1 (for `McpServerConfig` persistence).

### Tasks

- **P3.1 Transports (`mcp-client/src/transport/`).** Implement `stdio.rs` (spawn child, JSON-RPC over stdin/stdout) and `http_sse.rs` (connect over HTTP with SSE for server-to-client streaming). Back both with the `McpTransport` enum from Section 5.2.
- **P3.2 Server lifecycle (`mcp-client/src/session.rs`).** Implement spawn/connect, the initialize handshake with capability negotiation, `list tools`, on-demand invoke, bounded reconnect with backoff, and teardown per Section 5.3. Maintain an `McpServerHandle` per server (state, negotiated capabilities, cached tools).
- **P3.3 Tool discovery and schema mapping (`mcp-client/src/tools.rs`).** Convert MCP `ToolDescriptor`s into internal `ToolSpec`s, namespaced by server (for example `filesystem__read_file`) to avoid collisions (Section 5.4).
- **P3.4 Function-calling bridge.** In `orchestrator-core`, attach discovered tools to `ChatRequest.tools`, resolve a model tool call back to its owning server, validate arguments against the tool input schema, invoke the tool, and append the result as a `tool` role message so the model can continue (Sections 5.4 and 5.5).
- **P3.5 Permission model.** Implement `PermissionMode` (`Ask`/`Allow`/`Deny`) with per-tool overrides. In `Ask` mode emit `permission_requested` and block until a `resolve_permission` decision arrives (Sections 5.6 and 9.4).
- **P3.6 Error and resource handling.** Structured tool-error results (no turn crash), per-tool timeouts and output-size caps, and hiding tools from disconnected servers (Section 5.6).

### Deliverable

An MCP client that connects to stdio and HTTP/SSE servers, discovers tools, exposes them to models as function-calling specs, and executes tool calls with permission gating and robust error handling.

### Parallelization

- P3.1 and P3.3 can start in parallel. P3.2 depends on P3.1.
- P3.4 depends on P3.2 and P3.3. P3.5 and P3.6 can then proceed in parallel.

### Verification / acceptance

- `cargo test` in `mcp-client` runs against a bundled mock/fixture MCP server over stdio: handshake, tool discovery, and a `tools/call` round-trip pass.
- `cargo test` covers namespacing collisions, argument-schema validation failures, timeouts, and disconnected-server tool hiding.
- An `orchestrator-core` integration test drives a full "model requests tool, permission auto-allowed, result fed back, generation continues" loop using a mock provider and the fixture MCP server.
- Whole-workspace `cargo build`, `cargo test`, and `cargo clippy` stay green.

---

## Phase 4: Routing policy engine

Goal: implement the pluggable routing engine: the `RoutingPolicy` trait, the automatic default policy using complexity/privacy/cost signals, and the manual-override plumbing with the precedence rules from architecture Section 6.

Dependencies: Phase 2 (routing selects from `AvailableModel` with capabilities) and Phase 3 (tool requirements are a routing constraint). Phase 1 supplies personas and privacy tags.

### Tasks

- **P4.1 Trait and types (`routing/src/policy.rs`).** Define `RoutingPolicy` (async trait), `RoutingRequest`, `RoutingDecision`, `ManualRoute`, `RouteSource`, `PrivacyTag`, `AvailableModel`, and `CostBudget` per Section 6.1.
- **P4.2 Signals (`routing/src/signals.rs`).** Implement the three signal families from Section 6.2: task complexity estimation, privacy tags as hard constraints, and cost signals with an optional budget.
- **P4.3 Automatic default policy (`routing/src/policies/auto_default.rs`).** Score candidates: filter first by hard constraints (privacy plus required capabilities from Section 4.4), then rank by a weighted blend of quality-for-complexity and cost. Populate `rationale` so the UI can show "why this model". Fail closed when a `Local-Only`/`Confidential` tag cannot be satisfied locally.
- **P4.4 Manual override and precedence (`routing/src/policies/manual_override.rs`).** Implement the precedence from Section 6.3: per-message override wins, then per-conversation pin, then automatic. Manual routes still pass hard-constraint validation so a manual choice cannot leak `Local-Only` data to the cloud.
- **P4.5 Policy registry (`routing/src/policies/registry.rs`).** Implement `PolicyRegistry` with a selectable, persisted `active` automatic policy. The manual-override resolver wraps whatever automatic policy is active.
- **P4.6 Pipeline integration (`orchestrator-core/src/pipeline.rs`).** Wire routing into the message pipeline: build a `RoutingRequest` from conversation/persona/tags/override, resolve a `RoutingDecision`, and pass it to the provider registry. Persist route metadata on `message_complete`.

### Deliverable

A routing engine that automatically selects a provider/model from complexity, privacy, and cost signals by default, honors manual per-message and per-conversation overrides with correct precedence, and records a human-readable rationale.

### Parallelization

- P4.1 first. Then P4.2 and P4.4 can run in parallel.
- P4.3 depends on P4.2. P4.5 depends on P4.3 and P4.4. P4.6 depends on P4.5.

### Verification / acceptance

- `cargo test` in `routing` covers: complexity scoring choosing local vs cloud, a `Local-Only` tag forcing local (and failing closed when impossible), cost bias, and capability filtering.
- `cargo test` verifies the precedence matrix: per-message override beats pin beats automatic, and a manual choice violating a privacy tag is rejected with an explanatory error.
- An `orchestrator-core` pipeline test runs a message end to end through routing to a mock provider and asserts the persisted route metadata and rationale.
- Whole-workspace `cargo build`, `cargo test`, and `cargo clippy` stay green.

---

## Phase 5: UI surfaces

Goal: implement all five React feature surfaces from architecture Section 8, each wired to the Tauri commands and events defined in earlier phases. No surface touches secrets or provider endpoints directly.

Dependencies: Phases 1 through 4 (the commands and events each surface consumes must exist). The chat surface depends most heavily on the full pipeline (Phase 4) and MCP permissions (Phase 3).

### Tasks

- **P5.1 IPC and state plumbing.** Complete typed wrappers in `frontend/src/ipc/commands.ts` and `events.ts`, and the Zustand stores `conversations.ts`, `providers.ts`, `tools.ts`, `personas.ts`. Treat the core as authoritative and invalidate caches on core events (Section 7.3).
- **P5.2 Chat interface (surface 1, Section 8.1).** Implement `MessageList`, `MessageBubble` (tool-call/tool-result rendering), `StreamingIndicator`, `Composer`, `RouteBadge`, and `PermissionPrompt`. Wire `send_message`, `stop_generation`, `resolve_permission`, `get_messages`; consume `message_delta`, `message_complete`, `message_error`, `permission_requested`.
- **P5.3 Model selector (surface 2, Section 8.2).** Implement `RoutingModeToggle`, `ProviderModelPicker` (grouped Local vs Cloud with capability and price hints), `PerMessageOverrideControl`, and `AutoRationaleTooltip`. Wire `list_available_models`, `set_conversation_route`, `set_message_override`, `get_route_explanation`; consume `providers_changed`, `conversation_updated`. Maps to routing pins/overrides from Phase 4.
- **P5.4 Tool/MCP server manager (surface 3, Section 8.3).** Implement `ServerList`, `ServerForm` (stdio command/args/env or HTTP/SSE url/headers), `ToolInspector`, `PermissionModeControl`, and `ConnectionHealth`. Wire `list_mcp_servers`, `add_mcp_server`, `update_mcp_server`, `remove_mcp_server`, `set_mcp_enabled`, `refresh_mcp_tools`, `set_tool_permission`; consume `mcp_state_changed`, `mcp_error`. This is the runtime add/remove path for Phase 3 servers.
- **P5.5 Agent config editor (surface 4, Section 8.4).** Implement `PersonaList`, `PersonaEditor`, `DefaultRoutePicker` (reuses the model picker), `AllowedToolsSelector`, and `RoutingHintControl`. Wire `list_personas`, `create_persona`, `update_persona`, `delete_persona`, `assign_persona`; consume `personas_changed`.
- **P5.6 Conversation history / session management (surface 5, Section 8.5).** Implement `ConversationList`, `SearchBar`, `ConversationContextMenu` (rename, tag, duplicate, export, delete), `SessionSummary`, and `NewConversationButton`. Wire `list_conversations`, `create_conversation`, `rename_conversation`, `set_conversation_tags`, `delete_conversation`, `export_conversation`, `open_conversation`; consume `conversation_updated`, `conversation_created`, `conversation_deleted`. Resuming a session restores persona, route pin, tags, and enabled tools.
- **P5.7 App shell and navigation.** Assemble the five surfaces into the app layout, wire any new `#[tauri::command]` handlers not yet present, and extend the capability files so the webview is granted only the commands it needs (Section 9.2).

### Deliverable

A fully interactive desktop app: users chat with streaming responses and route badges, switch models per conversation or per message, manage MCP servers and tool permissions, create and edit personas, and browse/resume/export conversation history.

### Parallelization

- P5.1 first (shared plumbing). Then P5.2 through P5.6 are five largely independent surfaces that can be built in parallel by different developers, since each targets a distinct feature folder and a distinct command/event set.
- P5.3 and P5.5 share the model picker component; coordinate so it is built once and reused.
- P5.7 depends on the surfaces being present.

### Verification / acceptance

- `vitest` plus React Testing Library cover each surface: rendering, command invocation (mocked `invoke`), and event-driven state updates (mocked event stream).
- An end-to-end manual check in `tauri dev`: send a message and watch it stream with a route badge; switch to a manual model and confirm the override takes effect; add a stdio MCP server and see its tools; approve a permission prompt; create a persona and assign it; rename, tag, export, and reopen a conversation.
- `tauri build` produces a working installer for the primary target OS.
- Whole-workspace `cargo build`, `cargo test`, `cargo clippy`, and `vitest` stay green.

---

## Phase 6: Security hardening and extensibility validation

Goal: close out the security posture from architecture Section 9 and prove the extensibility claims from Section 10 by exercising and documenting the three add-a-thing procedures end to end.

Dependencies: Phases 1 through 5 (hardening and extensibility validation act on the finished system).

### Tasks

- **P6.1 Keychain audit.** Verify that `ProviderConfig` and SQLite hold only `SecretRef` handles, that keys live in the OS keychain via `keyring`, and that no command or event returns raw key material to the webview (Section 9.1). Add regression tests that assert secret non-exposure.
- **P6.2 IPC allowlist and permissions.** Review `tauri-app/capabilities/` so the webview is granted only the specific commands and plugins it needs (least privilege). Confirm there is no generic passthrough to the core and that every command validates its arguments before touching the core (Section 9.2).
- **P6.3 LM Studio localhost handling.** Enforce loopback-preferred base-URL validation: default to `http://localhost:1234/v1`, warn and recommend TLS for non-loopback hosts, block obviously dangerous internal targets where feasible, and send no implicit API key on the local path (Section 9.3). Add tests for the URL validator (loopback vs remote vs blocked). Note: the Local Runtimes base URL is now wired to a persisted `ProviderConfig` through the `set_local_runtime` command (previously a display-only value in the Local Runtimes settings), so this validator runs on that write path via `check_provider_base_url` (a blocked URL rejects the save, an accepted non-loopback plaintext URL returns a non-blocking warning).
- **P6.4 MCP permission prompts and sandboxing.** Confirm `Ask` mode blocks on a `permission_requested` event, that stdio servers run with a controlled environment and no inherited secrets and are terminated on disable/removal/exit, and that only tools from enabled servers allowed by the active persona/conversation are exposed (Section 9.4).
- **P6.5 Extensibility: add a provider (Section 10.1).** Document and exercise the procedure end to end: implement `ChatProvider` plus `ProviderFactory` in `providers/src/adapters/`, register the factory, and confirm the provider becomes configurable and usable by routing and the UI with no changes to `orchestrator-core` or `routing`. Note that any OpenAI-compatible endpoint needs no new adapter (added at runtime as a `GenericOpenAI` `ProviderConfig`).
- **P6.6 Extensibility: add a routing policy (Section 10.3).** Document and exercise: implement `RoutingPolicy::decide` in `routing/src/policies/`, register it in the `PolicyRegistry`, select it as the active automatic policy, and confirm the manual-override precedence wrapper still applies and the pipeline is untouched.
- **P6.7 Extensibility: add an MCP server (Section 10.2).** Document the runtime, no-code path: add a server through the Tool/MCP Server Manager by specifying its transport, and confirm handshake, discovery, and exposure happen automatically.
- **P6.8 Extensibility docs.** Capture P6.5 through P6.7 as an "Extending the app" guide in the repository so the three seams (`ChatProvider`, `RoutingPolicy`, MCP transport) are documented for future contributors, along with the versioned config schema and forward-compatibility notes (Section 10.4).

### Deliverable

A hardened build with audited secret handling, a least-privilege IPC surface, safe local-network handling, enforced MCP permissions, and validated, documented extension procedures for providers, policies, and MCP servers.

### Parallelization

- P6.1 through P6.4 (hardening) are independent audits and can run in parallel.
- P6.5, P6.6, and P6.7 (extensibility validation) are independent and can run in parallel. P6.8 depends on them.

### Verification / acceptance

- `cargo test` regression suite proves secret non-exposure and correct base-URL validation.
- `cargo test` and an `orchestrator-core` integration test confirm MCP `Ask`-mode blocking, controlled child-process environment, and persona/conversation tool gating.
- Adding a demo provider and a demo policy compiles and works with zero edits to `orchestrator-core` or `routing` (proven by `cargo build` plus a targeted `cargo test`), and adding an MCP server at runtime in `tauri dev` discovers tools with no code change.
- `tauri build` produces a signed installer with the finalized capability files.
- Whole-workspace `cargo build`, `cargo test`, `cargo clippy`, and `vitest` are green, and CI passes on all matrix targets.

---

## Phase 8: Ollama integration

Goal: integrate [Ollama](https://ollama.com/) as a first-class local provider, reusing the OpenAI-compatible native chat path for chat/streaming at `http://127.0.0.1:11434/v1` and adding Ollama's native model discovery (`GET /api/tags`), so installed Ollama models surface as Local models in the picker and route through the existing Auto / Prefer Local / Prefer Quality / Manual modes (Section 4.3).

Dependencies: Phase 2 (the `ChatProvider` contract, shared native adapter, and registry) and Phase 4 (routing consumes the local classification and zero price). Frontend surfacing depends on Phase 5's model selector.

### Tasks

- **P8.1 Provider kind.** Add `ProviderKind::Ollama` to `domain` (serializes to `"ollama"` under the existing `rename_all = "camelCase"`), guarded by the camelCase serde test.
- **P8.2 Ollama adapter (`providers/src/adapters/ollama.rs`).** Reuse the shared `NativeAdapter` for chat/streaming with `base_url` default `http://127.0.0.1:11434/v1` and no key by default. Override `list_models` to call Ollama's native `GET /api/tags` at the server root (derived by stripping a trailing `/v1` from the chat `base_url`), decoding `{"models":[{"name":...}]}` into `ModelInfo` ids.
- **P8.3 Registry and pricing wiring.** Add `OllamaFactory`, register it in `builtin_registry()`, re-export it from the crate root, and seed `ProviderKind::Ollama` at `TokenPrice::ZERO` in `PricingTable::bundled_defaults()` alongside LM Studio.
- **P8.4 Local classification.** Classify `ProviderKind::Ollama` as provably-local in the routing privacy gate (`local_provider_ids()` in `tauri-app`), like LM Studio, so `Local-Only` requests may route to it and it is grouped under Local.
- **P8.5 Frontend surfacing.** Add `"ollama"` to the TypeScript `ProviderKind` union so it mirrors the Rust enum; confirm the zero-priced Ollama models fall under the Local group in the `ProviderModelPicker` (via the existing zero-price heuristic) with no grouping change required.

### Deliverable

A configured Ollama provider that lists its installed models via `/api/tags`, runs streaming and non-streaming chat over the shared native path, is treated as local by the privacy gate and zero-priced by the cost signal, and appears under Local in the model picker across all routing modes.

### Parallelization

- P8.1 first (the kind is the shared dependency). P8.2, P8.3, and P8.4 then proceed in parallel once the kind exists. P8.5 (frontend) depends on P8.1 landing the `"ollama"` serialization.

### Verification / acceptance

- `cargo test` in `providers` covers the adapter with mocked HTTP: `list_models` hits `GET /api/tags` and yields the installed model ids, and chat POSTs to `/chat/completions` with no `Authorization` header (no live network required in CI).
- A `providers` inline unit test asserts `build_ollama` selects `AuthStrategy::None` when `api_key_ref` is unset; an `#[ignore]`d manual test exercises a real Ollama server on `http://127.0.0.1:11434`, excluded from CI.
- `cargo test` in `domain` asserts `ProviderKind::Ollama` serializes to `"ollama"`; the `providers` registry test asserts an Ollama factory is registered; the `tauri-app` test asserts Ollama is classified local.
- A `vitest` test asserts a zero-priced Ollama-style model appears under the Local group in the model picker.
- Whole-workspace per-crate `cargo build`, `cargo test`, `cargo clippy` (with `-D warnings`), and `vitest` are green, and CI passes on all matrix targets.

---

## Phase 9: Embedded local inference engine (Strategy B)

Goal: build the zero-install embedded local inference engine so the app can run local GGUF models in-process via the llama.cpp family, with no separate Ollama or LM Studio install required. The engine implements the same `ChatProvider` contract as every HTTP adapter (emitting OpenAI-shaped `ChatDelta`s), is registered as a first-class local provider (`ProviderKind::Embedded`), routes through the existing Auto / Prefer Local / Prefer Quality / Manual modes, and surfaces imported `.gguf` models under Local in the model picker with a settings surface to import/select/load a local model (Section 4.3).

Dependencies: Phase 2 (the `ChatProvider` contract and registry) and Phase 4 (routing consumes the local classification and zero price). Frontend surfacing depends on Phase 5's model selector and settings surfaces. The heavy native llama.cpp binding is isolated behind a default-off cargo feature so the routine build never needs a C++ toolchain or network.

### Tasks

- **P9.1 Engine crate (`crates/engine`).** Scaffold a new `[workspace] exclude`d crate hosting `EmbeddedEngine`, which implements `ChatProvider` (id `"embedded"`; capabilities streaming-only: tools/vision/json_mode off; a conservative `max_context`). The native llama.cpp binding (`llama-cpp-2`) is an OPTIONAL dependency gated behind a cargo feature `llama` that is DEFAULT OFF. Under default features the crate is a dependency-light stub whose `chat`/`chat_stream` return a clear "not compiled with llama" error; the real llama.cpp-backed path (GGUF load, greedy decode, token-to-`ChatDelta` mapping, terminal finish reason, one model resident at a time) lives entirely behind `#[cfg(feature = "llama")]`. A local `.gguf` model registry (import/select/list) is unit-testable without the native lib; downloading/catalog is deferred.
- **P9.2 Provider kind.** Add `ProviderKind::Embedded` to `domain` (serializes to `"embedded"` under the existing `rename_all = "camelCase"`), guarded by the camelCase serde test.
- **P9.3 Factory + registry + pricing wiring.** Add `EmbeddedFactory` (`providers/src/adapters/embedded.rs`) building `engine::EmbeddedEngine` from a `ProviderConfig` (no key needed), register it in `builtin_registry()`, re-export it from the crate root, and seed `ProviderKind::Embedded` at `TokenPrice::ZERO` in `PricingTable::bundled_defaults()`. `providers` gains a path dep on `engine` WITHOUT enabling `llama` by default, plus a `llama` passthrough feature.
- **P9.4 Local classification.** Classify `ProviderKind::Embedded` as provably-local in the routing privacy gate (`local_provider_ids()` in `tauri-app`) — the engine runs in-process, so it is always local — so `Local-Only` requests may route to it and it is grouped under Local.
- **P9.5 Lifecycle commands.** Add `#[tauri::command]` handlers for the embedded model lifecycle (list/import/select/load/unload/status of a local `.gguf`), returning display-safe camelCase DTOs (never secret material), registered in `generate_handler!`. Add TS mirrors in `ipc/commands.ts` and DTO types in `types/index.ts`.
- **P9.6 Frontend surfacing.** Add `"embedded"` to the TypeScript `ProviderKind` union so it mirrors the Rust enum; confirm zero-priced embedded models fall under the Local group in the `ProviderModelPicker` (via the existing zero-price heuristic, with explicit test coverage). Add a first-class embedded-engine surface to `LocalRuntimesSection` (a `.gguf` path input, Import / Load / Unload controls, a per-model Load action in the imported-model list, and a status line showing which model is loaded), wired to the FEAT-002 lifecycle commands (`list`/`import`/`select`/`load`/`unload`/`status`) in `ipc/commands.ts`, keeping the LM Studio + generic OpenAI form intact. Reconcile the stale "coming soon" empty-state: Ollama (shipped in Phase 8) and the embedded engine are presented as available Local providers, and only genuinely-future runtimes (Hugging Face) remain in the informational note, preserving its `role`/`aria-label` and stable test id. The LM Studio + generic OpenAI form's base URL is now wired too: its `set_local_runtime` save persists a `ProviderConfig` (upserted by a stable per-kind id), so those endpoints enumerate under Local and route; the form rehydrates the configured runtime(s) from `list_local_runtimes` on mount and can edit or clear them via `clear_local_runtime`.
- **P9.7 CI.** Extend the routine per-crate `build` job to fmt/build/clippy/test `crates/engine` (default features) before `providers`. Add a NEW gated `engine-native` job (gated like `tauri-bundle`: `workflow_dispatch` or `v*` tags) on ubuntu + windows that installs the C/C++ toolchain the llama.cpp binding needs (cmake + C++ compiler + libclang) and runs `cargo build --manifest-path crates/engine/Cargo.toml --features llama`. This is the ONLY native-compile site.

### Deliverable

A zero-install embedded engine that imports/selects a local `.gguf`, runs streaming and non-streaming chat in-process via llama.cpp, implements the `ChatProvider` contract uniformly, is treated as local by the privacy gate and zero-priced by the cost signal, appears under Local in the model picker across all routing modes, and has a settings surface to manage the local model. The native path is isolated behind the default-off `llama` feature and proven by a dedicated gated CI job.

### Parallelization

- P9.1 and P9.2 first (the crate and the shared kind). P9.3, P9.4, and P9.5 then proceed once the crate + kind exist. P9.6 (frontend) depends on P9.2 landing the `"embedded"` serialization and P9.5 landing the commands. P9.7 (CI) lands with the backend wiring; the workflow change is routed through a PR.

### Verification / acceptance

- `cargo test` in `engine` (default features) covers the stub `chat`/`chat_stream` "not compiled" error, the model registry (register/select/list, relative/absolute path resolution), and `capabilities()`/`id()`; an `#[ignore]`d manual test under `--features llama` exercises a real `.gguf`, excluded from CI.
- `cargo test` in `domain` asserts `ProviderKind::Embedded` serializes to `"embedded"`; the `providers` registry test asserts an Embedded factory is registered (nine kinds total); the `tauri-app` test asserts Embedded is classified local and the embedded-model lifecycle helpers register/select/unload correctly.
- The gated `engine-native` CI job compiles `cargo build --manifest-path crates/engine/Cargo.toml --features llama` on ubuntu + windows; this is the only place the native llama.cpp path is exercised.
- A `vitest` test asserts a zero-priced embedded-style model appears under the Local group in the model picker and the settings surface drives the import/select/load commands.
- Whole-workspace per-crate `cargo build`, `cargo test`, `cargo clippy` (with `-D warnings`), and `vitest` are green, and CI passes on all matrix targets.

---

## Phase 10: Provider visibility diagnostics, cloud-key registration, and Ollama hardening

Goal: close the "I added a key (or started Ollama) but nothing shows up in the picker" gap by making provider enumeration observable and by making the Providers & Keys and local-provider write paths actually persist enumerable `ProviderConfig` rows. Three concrete threads: surface per-provider enumeration errors instead of swallowing them, register cloud provider keys as real `ProviderConfig` rows so their models enumerate under Cloud, and harden Ollama's `GET /api/tags` discovery so a running local Ollama surfaces its installed models. This phase is diagnostics and wiring only; the routing engine, the provider contract, and the embedded engine are untouched.

Dependencies: Phase 2 (the `ChatProvider` contract, registry, and `list_available_models`), Phase 5 (the model selector and settings surfaces that consume the results), and Phase 8 (the Ollama adapter and its `/api/tags` discovery). Section 9.1/9.2 secret-hygiene invariants constrain every new command and error path.

### Tasks

- **P10.1 Structured enumeration result surfaced to the UI.** Change `providers::list_available_models` to return a structured `AvailableModelsResult { models, errors }` instead of swallowing per-provider `list_models()` failures to stderr. Each failing provider is captured as a display-safe `ProviderEnumerationError { providerId, message }` while healthy providers still populate the list, so graceful degradation is preserved. Thread the new shape through the `list_available_models` Tauri command; `send_message` routing candidates consume only `result.models` so routing behavior is unchanged. Add the frontend `AvailableModelsResult` / `ProviderEnumerationError` types, update the IPC wrapper and the providers store (`errors: []` default), and render the errors near the model picker and under Providers & Keys via a `ProviderEnumerationErrors` component without blanking the picker. Verification: `cargo test --manifest-path hybrid-orchestrator/crates/providers/Cargo.toml` covers a mixed run where one adapter errors and others still return models; `cargo test --manifest-path hybrid-orchestrator/crates/tauri-app/Cargo.toml` covers the command returning both models and errors; `vitest` in `frontend/` asserts models and an enumeration error render together.
- **P10.2 Cloud provider key registration (`set_cloud_provider` family).** Add a `set_cloud_provider` / `list_cloud_providers` / `clear_cloud_provider` command family that persists a real `ProviderConfig` row (correct `ProviderKind`, stable per-kind id, `api_key_ref` = the stored `SecretRef`, optional `base_url`), mirroring the `set_local_runtime` pattern (get then update or insert, store the optional key via the `SecretStore` keeping only the opaque `SecretRef`, validate input, return a display-safe `_inner`-testable view). Register the commands in `generate_handler!` and add frontend IPC wrappers. Rework `ProviderKeysSection` to pick a provider KIND from a dropdown and rehydrate configured providers from the backend on mount instead of the dead-end `set_provider_secret` plus a `localStorage` marker, so cloud providers (OpenAI, Anthropic, Gemini, Bedrock, Azure, and Kiro) are enumerated and their models surface under Cloud. Verification: `cargo test --manifest-path hybrid-orchestrator/crates/tauri-app/Cargo.toml` drives the `_inner` fns to assert an inserted row carries the right kind and a `SecretRef` (never raw key material) and that `list_`/`clear_` round-trip; `vitest` in `frontend/` asserts selecting a kind and saving a key calls `set_cloud_provider` and rehydrates from `list_cloud_providers`.
- **P10.3 Ollama `/api/tags` hardening.** Harden the Ollama discovery override so a running local Ollama actually surfaces its models: default the base URL to the IPv4 loopback literal `http://127.0.0.1:11434/v1` (see the IPv4-vs-IPv6 tradeoff in Section 4.3), send an explicit `Accept: application/json` header on the `GET /api/tags` request, and prove the decode with a test using the exact rich `/api/tags` payload (models with `details`/`capabilities`) so serde leniency over `TagsResponse` (no `deny_unknown_fields`) is guaranteed to keep tolerating it. Verification: `cargo test --manifest-path hybrid-orchestrator/crates/providers/Cargo.toml` covers the wiremock `/api/tags` contract test asserting the `Accept` header and the decode of the exact payload into the installed model ids, plus the inline `DEFAULT_BASE_URL` / `tags_root` unit tests; the real-server exercise stays an `#[ignore]`d manual test excluded from CI.

### Deliverable

Provider enumeration is observable end to end: `list_available_models` returns healthy models plus display-safe per-provider enumeration errors that the picker and Providers & Keys surface without blanking the list; cloud provider keys entered under Providers & Keys persist a real `ProviderConfig` so their models enumerate under Cloud; and a running local Ollama surfaces its installed models via a hardened `GET /api/tags` discovery (IPv4 loopback default, explicit `Accept` header, decode proven against the exact payload).

### Parallelization

- P10.1, P10.2, and P10.3 touch mostly distinct seams (the enumeration return shape, the cloud-provider command family, and the Ollama adapter) and can proceed in parallel. P10.1 lands the `AvailableModelsResult` shape that the picker consumes, so coordinate the frontend rendering of P10.1's errors with P10.2's Providers & Keys rework since both render in that settings surface.

### Verification / acceptance

- `cargo test --manifest-path hybrid-orchestrator/crates/providers/Cargo.toml` covers the mixed healthy/errored enumeration run and the Ollama `/api/tags` contract and decode tests.
- `cargo test --manifest-path hybrid-orchestrator/crates/tauri-app/Cargo.toml` covers the `list_available_models` command returning `{ models, errors }` and the `set_cloud_provider` / `list_cloud_providers` / `clear_cloud_provider` `_inner` round-trip with no raw key material in any returned view.
- `vitest` in `frontend/` asserts models plus an enumeration error render together, and that Providers & Keys persists via `set_cloud_provider` and rehydrates from `list_cloud_providers`.
- Per-crate `cargo build`, `cargo test`, `cargo clippy` (with `-D warnings`), and `vitest` are green, and CI passes on all matrix targets.

### Future work / considered alternatives

These arose while closing this phase and are recorded as deferred and explicitly out of scope here so the intent is not lost:

- **OAuth "connect / login" for cloud providers.** An alternative to pasting an API key: let a user connect or log in to a provider and have the app obtain a token, rather than entering a raw key under Providers & Keys. Deferred. The tradeoff is a substantially larger effort: per-provider OAuth clients, token storage plus refresh, and Tauri redirect handling, whereas the current `SecretStore` plus `api_key_ref` contract (Section 9.1) is key-based and satisfies the "no secret material outside the keychain" invariant as-is. Not implemented in this phase, which only makes the existing key-based path persist a real `ProviderConfig`.
- **Folder-scan browse mode for the embedded engine.** A usability improvement over selecting a single `.gguf` file: let the embedded-engine "Browse" control select a FOLDER and have Agent Harbor scan it directly for local `.gguf` models. Deferred and out of scope here; the embedded engine (Phase 9) is untouched by this phase, and this concerns the Local Runtimes browse UX rather than provider visibility.
- **LAN host mode.** A future networking feature where one Agent Harbor instance runs an agent as a host and another device on the same local network connects to it and utilizes its language models. Deferred and out of scope here as a substantial future feature: it needs host and discovery, authentication, and transport design of its own, none of which this diagnostics-and-wiring phase introduces.

---

## Phase 11: Model picker startup population and refresh-on-change

Goal: close the remaining "I filled in a model and API key and it persists, but nothing reflects in the conversation and the chat picker still says 'No models available yet ...'" gap. Phase 10 made enumeration observable and made cloud keys persist real `ProviderConfig` rows, but two wiring gaps kept the CHAT picker empty: (bug A) the providers store's `load()` was only ever called from the Providers & Keys settings surface, so the chat picker never fetched models at startup or on its own mount; and (bug B) no backend command emitted `CoreEvent::ProvidersChanged`, so even saving a key or starting a runtime never triggered a refetch. This phase loads the store once at startup, emits `ProvidersChanged` after every successful provider-config mutation, and adds an always-available "Refresh models" affordance so failures are never silent. This is frontend wiring plus a fire-and-forget backend emit only; the routing engine, the provider contract, the enumeration shape, the Ollama adapter, and the embedded engine are all untouched. Explicitly OUT OF SCOPE (recorded here so the intent is not lost): embedding or bundling Ollama in-process (Ollama is already integrated and a reachable Ollama showed nothing for the SAME frontend reason, so bundling it would not fix the picker), the embedded-engine folder-scan / gguf-selector rework, any embedded feature-flag or release-bundle change, and the previously-considered HTML5 rewrite (the app stays on Tauri).

Dependencies: Phase 5 (the shell's single `onCoreEvent` fan-out effect and the model-selector / settings surfaces), Phase 10 (the `AvailableModelsResult { models, errors }` shape, the `ProviderEnumerationErrors` component, and the `set_cloud_provider` command family). Section 9.1/9.2 secret-hygiene invariants constrain the diagnostics: only display-safe `models`/`errors` are rendered, never a resolved secret.

### Tasks

- **P11.1 Startup load of the providers store (bug A).** In the shell's single root `useEffect` (empty deps, the one `onCoreEvent` subscription), also trigger `useProvidersStore.getState().load()` once at startup, so the chat model picker is populated on launch without the user first opening Settings. Keep EXACTLY ONE `onCoreEvent` subscription (opened once, torn down once) and the fan-out to every store's `applyCoreEvent` unchanged; do not add effect deps that re-run it. Verification: `vitest` in `frontend/` asserts mounting `<App/>` calls the providers store `load()` once at startup and that the single-subscription invariant still holds (listen called once; a `providersChanged` event drives a second load).
- **P11.2 Emit `CoreEvent::ProvidersChanged` after successful provider-config mutations (bug B).** In `crates/tauri-app/src/commands.rs`, after each SUCCESSFUL mutation, emit `CoreEvent::ProvidersChanged` through `AppState.core_events` (fire-and-forget, mirroring the existing `McpStateChanged` sends; a dropped UI event must not fail the command). Wire it for `set_cloud_provider`, `clear_cloud_provider`, `set_local_runtime`, `clear_local_runtime`, `import_embedded_model`, `select_embedded_model`, `load_embedded_model`, and `unload_embedded_model`. Emit in the thin `#[tauri::command]` wrapper AFTER the `_inner` body returns `Ok`, so a validation/persist error emits nothing and the existing `_inner` unit tests are unchanged. Remove the now-obsolete `#[allow(dead_code)]` and "no emitter wired yet" comment on `CoreEvent::ProvidersChanged` in `crates/orchestrator-core/src/events.rs`, keeping the other still-unemitted variants' allows and the exhaustive secret-hygiene test compiling. Verification: `cargo test --manifest-path hybrid-orchestrator/crates/tauri-app/Cargo.toml` drains the core-event receiver (`AppState::new` returns `(Self, rx)`) and asserts each mutation emits `ProvidersChanged`, and that a rejected mutation emits nothing; `cargo test --manifest-path hybrid-orchestrator/crates/orchestrator-core/Cargo.toml` keeps the `no_core_event_variant_carries_secret_material` test green.
- **P11.3 Visible errors near the chat picker plus a Refresh affordance.** With the startup `load()` now running in the chat context, the already-placed `ProviderEnumerationErrors` (inside `PerMessageOverrideControl` in the composer) surfaces "Couldn't load models from &lt;providerId&gt;: &lt;reason&gt;" when a provider fails, without a duplicate stacked error list in the same pane. Add a lightweight, always-available, `aria-label`led "Refresh models" button near the chat picker that calls the providers store `load()` for on-demand re-enumeration. Diagnostics are built ONLY from the display-safe `models`/`errors`; no resolved secret is ever rendered. Verification: `vitest` in `frontend/` asserts the Refresh control calls `load()` / re-runs `list_available_models`, and that `store.errors` render near the chat picker.

### Deliverable

The chat model picker is populated on launch without opening Settings; saving or clearing a cloud key or local runtime (and the embedded-model import/select/load/unload) emits `CoreEvent::ProvidersChanged` so the picker refetches; a failing provider surfaces a display-safe "Couldn't load models from ..." line near the chat picker instead of a silently empty picker; and an always-available "Refresh models" affordance lets the user re-enumerate on demand.

### Parallelization

- P11.1 (frontend startup load) and P11.2 (backend emit) touch disjoint seams (`app.tsx` versus `commands.rs`/`events.rs`) and can proceed in parallel. P11.3 (visible errors plus Refresh affordance) builds on P11.1's startup load running in the chat context, so land P11.1 first or together with it.

### Verification / acceptance

- `cargo fmt --manifest-path hybrid-orchestrator/crates/tauri-app/Cargo.toml --check` and `... crates/orchestrator-core/Cargo.toml --check` pass (offline).
- `cargo test --manifest-path hybrid-orchestrator/crates/tauri-app/Cargo.toml` covers the `ProvidersChanged` emit on each mutation (draining the receiver) and no emit on a rejected mutation.
- `cargo test --manifest-path hybrid-orchestrator/crates/orchestrator-core/Cargo.toml` keeps the secret-hygiene exhaustiveness test green after removing the `ProvidersChanged` `#[allow(dead_code)]`.
- `vitest` in `frontend/` asserts the startup `load()`, the preserved single-subscription invariant, the Refresh affordance calling `load()`, and errors rendering near the chat picker.
- Per-crate `cargo build`, `cargo test`, `cargo clippy` (with `-D warnings`), `vitest`, eslint, prettier, and `tauri build` are green, and CI passes on all matrix targets.

### Future work / considered alternatives

- **Bundling Ollama in-process.** Considered and explicitly rejected for this phase: Ollama is already integrated (`ensure_ollama_provider_config` seeds it and enumeration goes through `GET /api/tags`), and a reachable Ollama showed nothing for the SAME frontend reason this phase fixes (the picker store was never loaded), so bundling Ollama would not have fixed the picker. Out of scope.
- **HTML5 rewrite.** Considered and rejected: the app stays on Tauri (the desktop shell, keychain-backed `SecretStore`, and native embedded engine are core constraints). No action taken.

---

## Phase 12: Model-picker self-reporting diagnostics

Goal: close the last "nothing shows up and nothing tells me why" gap after Phases 10 and 11. A user still reported the chat picker showing "No models available yet ..." with no visible reason, so this phase makes the built app SELF-REPORT the load-path outcome instead of failing silently. Three remaining clean-empty causes are made visible: (A) the providers store `load()` swallowed an IPC rejection, leaving `models:[]`/`errors:[]` with no signal; (B) `providers::list_available_models` silently skipped a configured provider row whose instance was never built (the invisible skip); and (C) `list_available_models_inner` used a fail-fast `build_registry(...)?` that aborted the whole enumeration on the first un-buildable row. This phase is diagnostics and wiring only; the routing engine, the provider contract, the enumeration shape, the Ollama adapter, and the embedded engine are untouched.

Dependencies: Phase 10 (the `AvailableModelsResult { models, errors }` shape and `ProviderEnumerationErrors` component), Phase 11 (the startup `load()` and the chat-context enumeration). Section 9.1/9.2 secret-hygiene invariants constrain every new command and readout: only display-safe fields (`baseUrl`, counts, `ProviderError` `Display` strings) are surfaced, never a resolved secret.

### Tasks

- **P12.1 Backend self-report plus the invisible-skip fix.** In `crates/providers/src/builtins.rs`, change `list_available_models`'s `let Some(instance) = registry.get(&cfg.id) else { continue };` None branch to push a display-safe `ProviderEnumerationError { providerId, message }` (static message: the provider is configured but no instance was built and cannot be enumerated) instead of silently continuing. In `crates/tauri-app/src/commands.rs`, replace the fail-fast `providers::build_registry(...)?` in `list_available_models_inner` with a resilient per-row build (start from `builtin_registry()`, `build_from_config` per row, skip a failed row rather than aborting) so one un-buildable row surfaces via the enumeration path instead of a thrown `CommandError`. Add a display-safe `provider_diagnostics` `#[tauri::command]` with an `_inner` body that mirrors `list_available_models_inner`'s seeding, registry build, and shared embedded-instance setup exactly, returning `ProviderDiagnosticsReport { configuredCount, totalModelCount, providerCountWithModels, providers: [ ProviderDiagnostic { id, kind, baseUrl, instanceBuilt, modelCount, error } ] }` (`#[serde(rename_all = "camelCase")]`, never any `api_key_ref`/secret). Register it in `generate_handler!`. Verification: `cargo test --manifest-path hybrid-orchestrator/crates/providers/Cargo.toml` proves a config row with no built instance yields exactly one display-safe camelCase error and models stays empty; `cargo test --manifest-path hybrid-orchestrator/crates/tauri-app/Cargo.toml` proves `provider_diagnostics_inner` reports the seeded ollama-local row with `instanceBuilt` true and a non-empty error (no live Ollama) and that the serialized report is camelCase and secret-free.
- **P12.2 Frontend capture of the load outcome.** Extend the providers store (`frontend/src/state/providers.ts`) with `loadState` (`idle`/`loading`/`loaded`/`failed`) and a display-safe `lastError`, and rewrite `load()` to capture a rejected `list_available_models` into `loadState:"failed"`/`lastError` instead of swallowing it (models left as-is on failure). In `PerMessageOverrideControl`, render a copyable status line (`data-testid="model-load-status"`) reading `loading…` / `loaded N models from M providers` / `failed: <error>` above the existing `ProviderEnumerationErrors`. Add the hand-mirrored `ProviderDiagnostic` / `ProviderDiagnosticsReport` TS types and a `providerDiagnostics()` IPC wrapper. Verification: `vitest` in `frontend/` asserts a rejected `listAvailableModels()` sets `loadState:"failed"`/`lastError` without throwing, the status line renders the loaded/failed text, and `providerDiagnostics()` invokes `provider_diagnostics`.
- **P12.3 Always-reachable Diagnostics settings section.** Add `frontend/src/features/settings/DiagnosticsSection.tsx` (mount-load pattern from `LocalRuntimesSection`/`ProviderKeysSection`) that runs `provider_diagnostics` on mount and renders a plain-text, copyable per-provider readout (id, kind, `baseUrl` or "default", `instanceBuilt` yes/no, `modelCount`, and any `error`) plus a summary line and a "Run diagnostics" / Refresh button (`data-testid="diagnostics-refresh"`), catching an IPC throw so the panel is never blank. Wire it into `Settings.tsx`'s `SECTIONS` and section switch. Verification: `vitest` in `frontend/` navigates to the Diagnostics section, asserts the region and summary render with a well-formed mock, that Refresh re-invokes `provider_diagnostics`, and that a rejected call shows `failed: <error>` rather than a blank panel; every `<App/>`/`<Settings/>`/model-selector test has a `provider_diagnostics`-safe mock so no new IPC call resolves `undefined`.

### Deliverable

The built app self-reports the model-picker load outcome: the chat area shows a copyable status line (loading / loaded N from M / failed: reason), a configured provider row with no built instance is reported as a per-provider enumeration error instead of vanishing, one un-buildable row no longer aborts the whole enumeration, and an always-reachable Settings "Diagnostics" section runs `provider_diagnostics` to show a display-safe per-provider readout the user can copy into a bug report.

### Parallelization

- P12.1 (backend command plus fixes) lands the `provider_diagnostics` shape that P12.2 and P12.3 consume, so land it first or together. P12.2 (store plus chat status line) and P12.3 (Diagnostics settings section) touch mostly disjoint frontend seams and can proceed in parallel once the types and IPC wrapper exist.

### Verification / acceptance

- `cargo fmt --manifest-path hybrid-orchestrator/crates/providers/Cargo.toml --check` and `... crates/tauri-app/Cargo.toml --check` pass (offline).
- `cargo test --manifest-path hybrid-orchestrator/crates/providers/Cargo.toml` covers the invisible-skip regression, and `... crates/tauri-app/Cargo.toml` covers `provider_diagnostics_inner`.
- `vitest` in `frontend/` covers the store throw path, the chat status line, the `providerDiagnostics()` wrapper, and the Diagnostics section (including the IPC-throw path).
- Per-crate `cargo build`, `cargo test`, `cargo clippy` (with `-D warnings`), `vitest`, eslint, prettier, and `tauri build` are green, and CI passes on all matrix targets.

---

## Phase 13: Surface the real provider build error and base_url UX

Goal: finish the "nothing tells me why" story from Phase 12. Phase 12 made a configured-but-unbuilt row visible, but it always showed the STATIC "no instance was built" text because both commands discarded the build error via `let _ = registry.build_from_config(...)`. This phase captures that real `ProviderError` and surfaces it in both the picker and diagnostics, confirms the Gemini-by-key build path works at default, and adds non-blocking base_url UX so users stop pasting web/session or Gemini model URLs into a generic OpenAI-compatible runtime. Diagnostics stays DERIVED from the shared enumeration so it cannot drift from the picker; secret hygiene (Section 9.1/9.2) is preserved throughout.

Dependencies: Phase 12 (the `AvailableModelsResult { models, errors }` shape, the invisible-skip branch, and `provider_diagnostics`). Section 9.1/9.2 constrain every surfaced string to `ProviderError` `Display` (failure class only, never key material).

### Tasks

- **P13.1 Capture and surface the real build error.** In `crates/providers/src/builtins.rs`, add `list_available_models_with_build_errors(registry, configs, pricing, build_errors: &BTreeMap<String, String>)`; keep the three-argument `list_available_models` as a delegate that passes an empty map so external callers are unchanged. In the `registry.get(&cfg.id)` None branch, prefer a captured `build_errors` entry keyed by `cfg.id` over the static generic message. In `crates/tauri-app/src/commands.rs`, stop discarding the per-row `build_from_config` `Err` in `list_available_models_inner` and `provider_diagnostics_inner`: capture each failing row's `ProviderError` `Display` into a `BTreeMap<String, String>` keyed by `cfg.id` and pass it to the new seam, so the picker's enumeration errors and the Diagnostics `error` field both show the real cause and cannot drift. Verification: `cargo test` per changed crate proves an unbuilt row surfaces its captured build error (falling back to the static text only when none was recorded) and that the auto-seeded `ollama-local` row still builds and enumerates.
- **P13.2 Confirm Gemini-by-key at default.** Confirm (no code change needed) that `set_cloud_provider` stores the key under config id `gemini-cloud` and `build_gemini` resolves that same handle and defaults `base_url` to `https://generativelanguage.googleapis.com`, so a plain Gemini key at default builds and enumerates; the reported failure was a keyring resolve failure, now surfaced via P13.1 as `ProviderError::Auth`.
- **P13.3 Advisory base_url UX (non-blocking).** In `crates/tauri-app/src/commands.rs`, add `advise_generic_openai_base_url` plus `merge_warnings`, wired into `set_cloud_provider_inner` and `set_local_runtime_inner`, that warns (in the returned view's `warning` field) when a `GenericOpenAI` base URL looks like a web/session URL (`/session/`) or a model endpoint (`:generateContent`) rather than an API root, WITHOUT blocking the save; link-local/metadata IPs remain the ONLY blocked case. Update `ProviderKeysSection.tsx` and `LocalRuntimesSection.tsx` with clearer placeholders/inline help, matching non-blocking client advisories, and a hint to configure Gemini via the Gemini kind rather than the generic OpenAI-compatible runtime. Verification: `cargo test` per changed crate covers the advisory (non-blocking warning, save still persists) and that link-local/metadata still blocks; `vitest` covers the client advisories.

### Deliverable

The picker and the Diagnostics panel show the REAL per-row provider build error (for example a Gemini keyring resolve failure or a generic-OpenAI missing/invalid base_url) instead of the generic fallback, a plain Gemini key at default builds and enumerates, and the settings surfaces steer users toward an API base URL (like `https://host/v1`) and the Gemini kind with non-blocking advisories, all while preserving the v0.7.3 non-fatal behavior (one failing provider never blanks the list) and secret hygiene.

### Parallelization

- P13.1 (backend capture plus the new seam) is the foundation and lands first. P13.2 is a confirmation that rides along with P13.1's error surfacing. P13.3 (advisory plus frontend UX) touches disjoint seams and can proceed in parallel once P13.1 is in.

### Verification / acceptance

- `cargo fmt --manifest-path hybrid-orchestrator/crates/providers/Cargo.toml --check` and `... crates/tauri-app/Cargo.toml --check` pass (offline).
- `cargo test --manifest-path hybrid-orchestrator/crates/providers/Cargo.toml` covers the build-error preference over the static fallback, and `... crates/tauri-app/Cargo.toml` covers the captured-error wiring and the advisory base_url check.
- `vitest` in `frontend/` covers the non-blocking client advisories and the Gemini-kind hint.
- Per-crate `cargo build`, `cargo test`, `cargo clippy` (with `-D warnings`), `vitest`, eslint, prettier, and `tauri build` are green, and CI passes on all matrix targets.

---

## Cross-cutting: testing strategy and deferred items

### Testing strategy

- **Unit tests (per crate).** Each Rust crate carries `cargo test` coverage for its own logic: persistence CRUD and migrations, secret store round-trips, provider request/response translation, MCP handshake and tool mapping, and routing scoring and precedence. Frontend components carry `vitest` plus React Testing Library unit tests with mocked `invoke` and mocked event streams.
- **Integration tests (`orchestrator-core`).** The pipeline is tested end to end with mock providers and a fixture MCP server: route selection, provider call, tool loop with permission gating, persistence of results and route metadata. These require no live network so they run in CI.
- **Adapter contract tests.** Each provider adapter is tested against recorded or mocked HTTP/SSE responses so translation and stream normalization are verified without live credentials. Live checks (LM Studio on loopback, real cloud keys) are optional, gated, and manual, never required in CI.
- **UI and app-level checks.** `vitest` for component behavior; `tauri build` in CI to catch bundling, capability, and COPY-path issues that unit tests miss; a documented manual end-to-end script exercised at the end of Phase 5 and Phase 6.
- **Static analysis and formatting.** `cargo clippy` and `rustfmt` for Rust, `eslint` and `prettier` for the frontend, enforced in CI.
- **Security regression tests.** Assertions that secrets never cross IPC, base-URL validation behaves, and MCP permission gating holds (Phase 6), kept as permanent regression tests.
- **CI gate.** Every phase's acceptance is `cargo build`, `cargo test`, `cargo clippy`, `vitest`, and `tauri build` green on the OS matrix. A phase is not complete until its listed checks pass.

### Deferred / stretch items

These are explicitly out of the initial build (aligned with the non-goals in architecture Section 1.3) and are candidates for later iterations:

- **Additional provider adapters** beyond the six named targets, contributed through the stable `ChatProvider` seam.
- **Additional routing policies** (for example latency-aware or reliability-aware policies) through the `RoutingPolicy` seam.
- **MCP resources and prompts** beyond tools, if and when specific servers need them.
- **Conversation summarization and long-context truncation strategies** that build on `max_context` capability data (Section 4.4).
- **Optional encrypted export/import and local backup tooling** (state stays local first; no hosted sync backend, per Section 1.3).
- **Cross-platform packaging polish, auto-update, and code-signing automation** beyond the baseline installer.
- **A pure-Rust GUI or alternate shell** exploration, allowed only with the documented justification and migration note from Section 3.1, since the orchestration crates are already Tauri-independent.
- **Browser extension or mobile client**, out of scope for this version but kept feasible by the portable core.
