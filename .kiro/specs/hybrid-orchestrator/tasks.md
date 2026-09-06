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
- **P6.3 LM Studio localhost handling.** Enforce loopback-preferred base-URL validation: default to `http://localhost:1234/v1`, warn and recommend TLS for non-loopback hosts, block obviously dangerous internal targets where feasible, and send no implicit API key on the local path (Section 9.3). Add tests for the URL validator (loopback vs remote vs blocked).
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
