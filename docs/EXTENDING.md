# Extending Agent Harbor

Agent Harbor is built around traits and registries so the three most common
extensions add code (or, for MCP servers, no code at all) without touching the
core message pipeline. This guide documents each extension seam, cites the real
code locations, and points at the Phase 6 demo tests that prove the seam works
with **zero edits to `orchestrator-core` or the routing engine**.

This guide corresponds to Section 10 of the
[architecture specification](../.kiro/specs/hybrid-orchestrator/architecture.md).

All crate paths below are relative to `hybrid-orchestrator/`.

## Contents

- [Adding a provider](#adding-a-provider)
- [Adding a routing policy](#adding-a-routing-policy)
- [Adding an MCP server](#adding-an-mcp-server)
- [Versioned config schema and forward compatibility](#versioned-config-schema-and-forward-compatibility)

## Adding a provider

A provider is a chat backend the orchestrator can route a turn to. The seam is
the `ChatProvider` trait plus a `ProviderFactory` that builds it from a
`ProviderConfig`.

**The common case needs no new code.** Any OpenAI-compatible endpoint is added
at **runtime** as a `GenericOpenAI` `ProviderConfig` (a `base_url` plus an
optional secret-key reference). No new adapter, no rebuild. Only reach for a new
adapter when a backend speaks a genuinely different wire protocol.

To add a new adapter for a non-OpenAI-compatible backend:

1. **Implement `ChatProvider`** for the backend in `crates/providers/src/adapters/`.
   This is where request/response translation, streaming, and capability
   reporting live. Existing adapters such as
   `crates/providers/src/adapters/lmstudio.rs` and
   `crates/providers/src/adapters/generic_openai.rs` are the reference
   implementations; several share the OpenAI-compatible core in
   `crates/providers/src/adapters/native.rs`.
2. **Implement a `ProviderFactory`** that constructs your provider from a
   `ProviderConfig`.
3. **Register the factory** with the `ProviderRegistry` via
   `ProviderRegistry::register_factory` (`crates/providers/src/registry.rs`). The
   built-in factories are all registered in one place,
   `builtin_registry()` in `crates/providers/src/builtins.rs`; add yours
   alongside them (or register it at startup on the registry that
   `build_registry()` returns).
4. The provider is now configurable (base URL, key reference, extras) and
   immediately usable by routing and the UI. It surfaces through
   `providers::list_available_models` as `AvailableModel` rows, so the model
   selector picks it up automatically.

**Invariant:** no change to `orchestrator-core` or `routing` is required. The
routing engine consumes `AvailableModel` candidates and the pipeline drives the
`ChatProvider` trait object, neither of which knows about concrete adapters.

`ProviderKind` is a fixed enum in the `domain` crate, so a demo/test provider
registers against an existing kind (for example `GenericOpenAI`) rather than
adding a new enum variant. That is exactly what the worked example does.

**Worked example (Phase 6, P6.5):**
[`crates/providers/tests/demo_provider_extension.rs`](../hybrid-orchestrator/crates/providers/tests/demo_provider_extension.rs)
defines a demo `EchoProvider` + `EchoFactory`, registers the factory against
`ProviderKind::GenericOpenAI` via `ProviderRegistry::register_factory`, builds an
instance from a `ProviderConfig`, and drives it through
`providers::list_available_models` to confirm it surfaces as selectable
`AvailableModel` rows. The test lives entirely in the `providers` crate and
requires no edit to `orchestrator-core` or `routing`.

## Adding a routing policy

A routing policy decides which model an automatic (non-pinned) turn should use.
The seam is the `RoutingPolicy` trait and the `PolicyRegistry`.

1. **Implement `RoutingPolicy::decide`** for your policy in
   `crates/routing/src/policies/` (`RoutingPolicy` is defined in
   `crates/routing/src/policy.rs`). `decide` receives the routing request
   (complexity, privacy, and cost signals plus the candidate models) and returns
   a `RoutingDecision`.
2. **Register it** in the `PolicyRegistry` via `PolicyRegistry::register`
   (`crates/routing/src/policies/registry.rs`).
3. **Select it as the active automatic policy** via `PolicyRegistry::set_active`.

**You only implement the automatic decision.** Manual-override precedence
(Section 6.3) is applied by the engine wrapper `ManualOverrideResolver`
(`crates/routing/src/policies/manual_override.rs`), which _wraps_ whatever
automatic policy is active. A manual pin (or conversation pin) short-circuits to
the pinned route before your policy is consulted; when there is no override, the
wrapper delegates to your `decide`. Your custom policy never has to handle the
override case, and the message pipeline is untouched.

**Worked example (Phase 6, P6.6):**
[`crates/routing/tests/demo_policy_extension.rs`](../hybrid-orchestrator/crates/routing/tests/demo_policy_extension.rs)
defines a demo `AlwaysFirstPolicy`, registers it on a `PolicyRegistry`, calls
`set_active(...)`, and asserts that (a) with no manual override the resolver
delegates to the demo policy's decision, and (b) with a manual override the
`ManualOverrideResolver` still short-circuits to the pinned route. This proves
the precedence wrapper stays intact with a custom active policy, with zero edits
to `orchestrator-core` or the routing engine logic.

## Adding an MCP server

Adding an MCP tool server is a **runtime, no-code** operation. Through the
Tool/MCP Server Manager UI you specify a transport and the client does the rest:

- **stdio transport:** a command (and arguments/env) the client launches as a
  child process, or
- **HTTP/SSE transport:** a base URL (with configured headers).

The MCP client then performs the JSON-RPC handshake, discovers the server's
tools, and exposes them to the model automatically. No adapter code and no
rebuild are needed; MCP servers are pluggable by design.

**Worked example (Phase 6, P6.7):**
[`crates/mcp-client/tests/stdio_roundtrip.rs`](../hybrid-orchestrator/crates/mcp-client/tests/stdio_roundtrip.rs)
exercises the runtime add path against the bundled mock MCP server: it builds an
`McpServerConfig` for a stdio transport and drives the same server-handle
lifecycle the Tool/MCP Server Manager uses, asserting that handshake, tool
discovery, and namespaced tool exposure all happen automatically from
configuration alone.

### Sandboxing and permission posture (Phase 6, P6.4)

Servers you add run under the Section 9.4 security posture, which Phase 6
hardened and pinned with regression tests:

- **Controlled child environment.** stdio servers are spawned from a _cleared_
  environment (`env_clear`) with only a minimal, documented allowlist re-added
  (`PATH`, and on Windows the child-startup essentials such as `SystemRoot`),
  then the caller-configured `env` is layered on top. A child therefore does
  **not** inherit the parent process's environment or any of its secrets. See
  `crates/mcp-client/src/transport/stdio.rs`. Note that allowlisting `PATH` is
  **env-secret isolation, not full process/filesystem sandboxing**: a child can
  still resolve and exec other host binaries on `PATH`. Run only servers you
  trust.
- **Ask-mode prompts block.** In `Ask` permission mode a tool invocation emits a
  `PermissionRequested` core event and **blocks until the user resolves it**; a
  Deny becomes a structured tool-error (the turn does not crash) and an Allow
  proceeds. See `crates/orchestrator-core/src/permission.rs`.
- **Least exposure to models.** Only tools from servers that pass **both** the
  conversation gate and the **active persona** gate are exposed to the model, so
  a not-allowed server's tools never reach it. Each gate restricts only when its
  allow-list is non-empty: an empty conversation `enabled_tool_servers` or an
  empty persona `allowed_tool_servers` imposes no restriction (matching how
  personas are created, where the allow-list defaults to empty), while a
  persona that restricts to server A exposes only A **even when the conversation
  never opted into gating**. See `effective_tool_servers` in
  `crates/orchestrator-core/src/tools_bridge.rs`.
- **Lifecycle.** Servers are terminated on disable/removal/exit; stdio children
  are killed when the transport shuts down.
- **Base-URL validation (staged seam, P6.2).** A loopback-preferred base-URL
  validator (accept loopback, accept remote TLS, warn on remote plaintext, block
  link-local/metadata and hostless input) is implemented and unit-tested in
  `crates/tauri-app/src/commands.rs` (`validate_base_url` /
  `check_provider_base_url`). It is a **staged enforcement seam, not an active
  runtime guarantee**: no IPC command currently accepts a raw provider `base_url`
  (providers are seeded through `persistence::ProviderRepo`), so the validator is
  guarded with `#[cfg_attr(not(test), allow(dead_code))]` and ready to be called
  before `ProviderRepo::insert` / `update` once a provider-config write command
  lands.

## Versioned config schema and forward compatibility

The extension seams above are stable on purpose. Section 10.4 of the
architecture describes the compatibility guarantees that keep older
configurations and adapters working as the app evolves:

- **Schema version.** `app_config` and persisted config records carry a
  `schema_version` (see `crates/persistence/src/config.rs`). On startup,
  migrations upgrade older versions forward; unknown newer fields are preserved
  where possible rather than dropped, so a config written by a newer build is
  not silently damaged by an older one.
- **Additive-change bias.** New capabilities are added as **optional fields with
  sensible defaults** so older configs keep working and newer configs degrade
  gracefully on older builds. Prefer adding an optional field over repurposing
  or removing an existing one.
- **Stable trait contracts.** `ChatProvider`, `RoutingPolicy`, and the MCP
  transport enums are the versioned extension seams. Changes to them are
  versioned and documented, and adapters/policies target a stable trait surface,
  so an extension written against today's traits keeps compiling.
- **Core independence from the shell.** The orchestration crates
  (`domain`, `orchestrator-core`, `providers`, `mcp-client`, `routing`,
  `persistence`, `secrets`) do **not** depend on Tauri. The desktop shell can be
  upgraded or replaced without breaking providers, routing, MCP, or persistence.

This forward-compatibility posture is why the three seams above can be extended
without editing the core: the pipeline depends only on the stable traits and on
versioned, additive config.
