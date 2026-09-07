# Agent Harbor

Agent Harbor is a cross-platform desktop application that orchestrates AI chat
providers, MCP tool servers, and routing policies behind a single Tauri (Rust
core) + React/TypeScript (Vite) frontend.

**The app is feature-complete per the specification.** All phases (0 through 6)
are implemented and merged: the full `message -> route -> provider -> tool
loop -> persist` pipeline, the five wired React surfaces, security hardening,
and validated extension seams. See [Implementation status](#implementation-status)
for the per-phase breakdown, [Build, test, and run](#build-test-and-run) to build
and launch it, and [Extending the app](#extending-the-app) plus
[docs/EXTENDING.md](docs/EXTENDING.md) to add a provider, routing policy, or MCP
server.

The authoritative design lives in the specs:

- [Hybrid Orchestrator architecture](.kiro/specs/hybrid-orchestrator/architecture.md)
- [Phased task plan (Phase 0 onward)](.kiro/specs/hybrid-orchestrator/tasks.md)

## Implementation status

All phases (0 through 6) are implemented and merged; the app is feature-complete
per the specification.

- **Phase 0 - Scaffold**: DONE. Full crate layout, a launching Tauri shell, a
  rendering React frontend, and CI.
- **Phase 1 - Core foundations**: DONE. Domain models (Section 7.1 DTOs in the
  leaf `domain` crate), SQLite persistence, the `secrets` keystore, the session
  manager, and the Tauri command/event skeleton.
- **Phase 2 - Provider layer**: DONE. The OpenAI-compatible `ChatProvider`
  contract and `ProviderRegistry`, adapters for OpenAI, LM Studio, a generic
  OpenAI-compatible endpoint, Azure OpenAI, Anthropic, Gemini, and Bedrock, plus
  `list_available_models`.
- **Phase 3 - MCP client**: DONE. stdio + HTTP/SSE transports, server lifecycle,
  tool discovery, the function-calling bridge, and Ask/Allow/Deny tool
  permissions.
- **Phase 4 - Routing policy engine + full message pipeline**: DONE. The
  pluggable `RoutingPolicy` trait with complexity/privacy/cost signals, the
  automatic default policy and manual-override precedence, and the
  `PolicyRegistry`, plus the end-to-end
  `message -> route -> provider -> tool loop -> persist` pipeline
  (`orchestrator_core::run_turn`) and the `send_message` command that drives it
  and streams over the core-event bridge.
- **Phase 5 - UI surfaces**: DONE. The five wired React surfaces - chat
  (streaming message list, per-message route badges, Ask-mode permission
  prompt), model selector (Auto vs manual pin, Local/Cloud provider-model
  picker with capability/cost hints), tool/MCP manager (transport-aware server
  form, live connection health, tool inspector, permission modes), agent editor
  (personas with system prompt, model parameters, default route, allowed tools,
  routing hint), and conversation history (search, rename/tag/duplicate/export/
  delete, session resume) - assembled into an app shell with a single top-level
  core-event subscription that fans events into the Zustand stores, backed by
  the new Tauri commands (get_messages, set_conversation_route, assign_persona,
  get_route_explanation, MCP server CRUD + set_mcp_enabled + refresh_mcp_tools +
  set_tool_permission, export_conversation, open_conversation, stop_generation)
  and a runtime MCP handle manager.
- **Phase 6 - Security hardening + extensibility validation**: DONE. Audited
  secret handling and least-privilege IPC (permanent regression tests assert no
  command return or core event carries secret material), a loopback-preferred
  base-URL validator (accept loopback, warn + recommend TLS off-host, block
  link-local/metadata targets) implemented and unit-tested as an enforcement
  seam staged for the provider-config write path - not yet an active runtime
  guarantee, since no IPC command currently accepts a raw provider `base_url`
  (see [docs/EXTENDING.md](docs/EXTENDING.md)), a hardened MCP child-process
  environment
  (`env_clear` + minimal allowlist, no inherited secrets) with enforced Ask-mode
  permission prompts and persona/conversation tool gating, and validated +
  documented provider/policy/MCP extension seams (zero-core-edit demos plus
  [docs/EXTENDING.md](docs/EXTENDING.md)).

## User interface

The desktop shell is a navigable layout with a collapsible **left sidebar** that
hosts the brand/logo and the primary destinations **Chat**, **History**, and
**Settings**:

- **Chat** is the default surface (streaming message list, composer, and
  permission prompt) with a **routing-mode segmented control** placed near the
  Chat composer to pick the per-conversation routing behavior (Auto / Prefer
  Local / Prefer Quality / Manual).
- **History** is the conversation history and session-management surface.
- **Settings** groups the heavy configuration behind its own **sub-navigation**
  (providers & keys, local runtimes, MCP / tools, agents, routing, and
  appearance). The **Appearance** section holds a **dark/light theme toggle**;
  the theme follows the OS by default until an explicit choice is stored.

The brand mark is a **hand-crafted SVG logo** (a harbor anchor fused with an
agent-node/network glyph in the teal/blue accent, legible on both light and dark
backgrounds). It lives in two places:

- `hybrid-orchestrator/frontend/src/assets/logo.svg` - the in-app logo rendered
  in the sidebar brand area.
- `hybrid-orchestrator/crates/tauri-app/icons/agent-harbor.svg` - the canonical,
  square, app-icon source that shares the motif.

### Regenerating the app icons

The raster app-icon set referenced by `bundle.icon` in `tauri.conf.json`
(`32x32.png`, `128x128.png`, `128x128@2x.png`, `icon.ico`, `icon.icns`) is
regenerated from a high-resolution raster export of `agent-harbor.svg` via the
Tauri CLI:

```sh
cargo tauri icon path/to/agent-harbor.png
```

The current rasters are still the earlier Phase 0 placeholders because the Tauri
CLI is unavailable in the offline sandbox; regenerating them from the SVG source
is a documented follow-up (see
[`hybrid-orchestrator/crates/tauri-app/icons/README.md`](hybrid-orchestrator/crates/tauri-app/icons/README.md)).

## Repository layout

```
agent-harbor/
├── .github/workflows/ci.yml        # CI: fast routine build/test/clippy/vitest + gated tauri bundle
├── .kiro/specs/hybrid-orchestrator # Architecture + task specifications
└── hybrid-orchestrator/            # Cargo workspace + frontend
    ├── Cargo.toml                  # Virtual workspace manifest
    ├── Cargo.lock                  # Committed lockfile (see policy below)
    ├── rustfmt.toml                # Rust formatting config
    ├── crates/
    │   ├── domain/                 # Section 7.1 DTOs (leaf crate)
    │   ├── orchestrator-core/      # Session manager + command/event skeleton
    │   ├── providers/              # ChatProvider contract, registry, adapters
    │   ├── mcp-client/             # MCP transports, lifecycle, tools, permissions
    │   ├── routing/                # Routing policies + signals (Phase 4, CI-verified)
    │   ├── persistence/            # SQLite DB, repositories, config
    │   ├── secrets/                # Secret keystore (keyring-backed)
    │   └── tauri-app/              # Tauri v2 desktop shell (binary)
    └── frontend/                   # React + TypeScript (Vite) UI
        ├── package.json            # package-lock.json is committed once CI generates it (see policy)
        ├── eslint.config.js        # ESLint flat config (React + TypeScript)
        ├── .prettierrc.json        # Prettier config
        └── src/                    # App, IPC stubs, feature folders
```

## Prerequisites

- **Rust** (stable toolchain) with the `clippy` and `rustfmt` components:
  `rustup component add clippy rustfmt`.
- **Node.js 20 or 22** and npm.
- **Tauri CLI v2**: `cargo install tauri-cli --version '^2'`
  (or `npm i -g @tauri-apps/cli`).
- **Platform dependencies for Tauri v2**:
  - **Linux**: `libwebkit2gtk-4.1-dev`, `build-essential`, `curl`, `wget`,
    `file`, `libxdo-dev`, `libssl-dev`, `libayatana-appindicator3-dev`,
    `librsvg2-dev`, `patchelf`.
  - **macOS**: Xcode command line tools (ships WKWebView).
  - **Windows**: WebView2 runtime (usually preinstalled on Windows 10/11) and
    the MSVC build tools.

## Build, test, and run

### Rust core (from `hybrid-orchestrator/`)

Every crate is under `[workspace] exclude` (the `members` array is empty - see
the offline note below), so the bare workspace commands (`cargo build`,
`cargo test`, `cargo clippy --all-targets`, `cargo fmt --all`) act on **nothing**
and there is no workspace-level cargo step. Build, test, lint, and format each
crate explicitly through its manifest, exactly as CI does:

```sh
cd hybrid-orchestrator

# Per crate (repeat for domain, secrets, persistence, providers, mcp-client,
# routing, orchestrator-core, tauri-app):
cargo fmt   --manifest-path crates/orchestrator-core/Cargo.toml --check
cargo build --manifest-path crates/orchestrator-core/Cargo.toml
cargo test  --manifest-path crates/orchestrator-core/Cargo.toml
cargo clippy --manifest-path crates/orchestrator-core/Cargo.toml --all-targets -- -D warnings
```

CI runs this fmt/build/test/clippy set for all eight crates on `ubuntu-latest`
and `windows-latest` (see [Continuous integration](#continuous-integration)).

### Frontend (from `hybrid-orchestrator/frontend/`)

```sh
cd hybrid-orchestrator/frontend
npm install          # install dependencies
npm run dev          # start the Vite dev server (http://localhost:5173)
npm run test         # run the vitest suite
npm run lint         # ESLint
npm run format       # Prettier (write); use format:check in CI
```

### Full desktop app (from `hybrid-orchestrator/`)

```sh
cd hybrid-orchestrator
cargo tauri dev --config crates/tauri-app/tauri.conf.json     # run the app (dev shell)
cargo tauri build --config crates/tauri-app/tauri.conf.json   # produce an installer bundle
```

You can also produce an installer without a local Tauri CLI by triggering the
gated `tauri-bundle` CI job (manual `workflow_dispatch` or a `v*` version tag) -
see [Continuous integration](#continuous-integration).

## Sandbox / offline constraint

The project was originally scaffolded in an **offline (INTEGRATIONS_ONLY)
sandbox** where crates.io and registry.npmjs.org are unreachable. This still
shapes the workspace layout as feature crates gained real dependencies through
Phases 1 through 3:

- In Phase 4 the `routing` crate gained path deps on `providers` and `domain`,
  which pull in the crates.io HTTP/DB/keyring stack transitively, so it is no
  longer dependency-free. It **joined the excluded set** and is **validated in
  CI** like every other crate; its own unit tests still use no live network.
- Every crate now carries crates.io dependencies (directly or transitively):
  `domain`, `orchestrator-core`, `persistence`, `secrets`, `providers`,
  `mcp-client`, `routing`, and `tauri-app` are all under `[workspace] exclude`
  and **validated in CI**, where network access exists. No crate remains an
  offline-buildable `members` entry, so the `members` array is empty and the
  bare `cargo` commands above build nothing offline.
- The frontend depends on the npm registry and is likewise validated in CI.

### Workspace `exclude` arrangement for the networked crates

Cargo resolves the entire `members` dependency graph to build `Cargo.lock`, even
for crates excluded from `default-members`. Because the crates listed above pull
in crates.io dependencies (for example `tauri`/`tauri-build`, `sqlx`, `keyring`,
`reqwest`, `tokio`, `aws-sigv4`) that are unreachable offline, listing them under
`members` would make **every** bare cargo command fail. To keep the offline
baseline green, they are placed under `[workspace] exclude` in
`hybrid-orchestrator/Cargo.toml` (not `members`). As of Phase 4 that includes
`routing` (it gained crates.io deps transitively through its `providers`/`domain`
path deps), so no crate remains a `members` entry and the `members` array is
empty.

A consequence: `cargo build --workspace` and `cargo build -p <crate>` do **not**
see the excluded crates. CI therefore builds/tests/clippies each one explicitly
via `cargo build --manifest-path crates/<crate>/Cargo.toml`, and the gated
`tauri build` step compiles `tauri-app` as part of bundling the installer.

**Known tradeoff (tracked, not permanent).** Because these crates are `exclude`d
rather than non-default `members` entries, their Section 3.3 dependency edges are
validated only in CI and their dependencies never enter
`hybrid-orchestrator/Cargo.lock`, so a breaking change to a leaf crate's public
API that affects an excluded crate will not surface in the offline default build.

**Exit path (once network access exists).** Move each excluded crate from
`exclude` into `members` in `hybrid-orchestrator/Cargo.toml` (keeping them out of
`default-members` so the bare offline `cargo build` still skips them), run a
networked `cargo build -p domain -p orchestrator-core -p persistence -p secrets
-p providers -p mcp-client -p tauri-app` to populate `Cargo.lock` with the full
dependency graph, and commit the updated `Cargo.lock` in the same change. This
reconnects the crates to the workspace lockfile without breaking the offline
default build.

## Continuous integration

[`.github/workflows/ci.yml`](.github/workflows/ci.yml) is organized as two jobs
across an OS matrix (`ubuntu-latest`, `windows-latest`; macOS is intentionally
omitted):

- **`build` (routine gate)** runs on **every push and pull request**. It runs
  `cargo fmt --check`, `cargo build/test/clippy` over the workspace **and** each
  excluded crate (via `--manifest-path`), plus the frontend `vitest` suite (with
  ESLint and Prettier checks). It does **not** bundle an installer, so it
  finishes in a few minutes and is the authoritative correctness gate.
- **`tauri-bundle` (gated release check)** installs a prebuilt Tauri v2 CLI and
  runs `tauri build` to produce an installer on the same matrix. It is gated to
  run **only** on manual dispatch (`workflow_dispatch`) and on version-tag pushes
  (`v*`), so the slow bundle step does not run on every commit.

## Auto-updates

From **v0.4.0** onward the desktop app can update itself in place via Tauri's
updater plugin. Updates are **cryptographically signed** (minisign) and verified
on-device against the public key embedded in the app, so a downloaded update is
installed only if its signature matches; unsigned or tampered payloads are
rejected.

- The updater polls the project's GitHub Releases for a `latest.json` manifest
  (resolved from `releases/latest/download/latest.json`, which always points at
  the newest published release) and compares its version to the running app.
- Signing happens **in CI**: the gated `tauri-bundle` job signs each updater
  bundle with the repository's pre-configured `TAURI_SIGNING_PRIVATE_KEY` /
  `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` secrets, emitting a `.sig` file per
  installer, and the `publish-updater-manifest` job composes `latest.json` (with
  each platform's signature + release download URL) and attaches it to the
  release. The private signing key is never stored in the repository.
- `latest.json` is composed **per platform**: it includes only the platforms
  that actually produced a signed updater payload for a given release, and the
  job fails only when *no* platform did. Today that means **Windows self-update
  is live** (`windows-x86_64`), while **Linux self-update is still pending**: the
  pinned Tauri v2 does not currently emit the Linux `*.AppImage.tar.gz` updater
  payload, so `linux-x86_64` is omitted from the manifest until that lands
  (tracked as a follow-up). Linux users should update by re-downloading the
  release installer for now.
- There is an in-app **Check for updates** control in **Settings > About /
  Updates** that shows the current version, checks for a newer release, and
  offers **Install and restart** when one is available.

> **Note - Windows SmartScreen.** This signed auto-updater is a **separate
> concern** from Windows executable code-signing. Until the app ships with a
> code-signing certificate (a paid task deferred to a later phase), Windows may
> still show a SmartScreen "unknown publisher" warning on first launch of the
> installer. The updater's minisign signing does not remove that warning.

## Extending the app

The architecture is built around traits + registries so the three most common
extensions add code (or, for MCP servers, no code at all) without modifying the
core pipeline. See **[docs/EXTENDING.md](docs/EXTENDING.md)** for step-by-step
guides, real file references, and the Phase 6 demo tests that prove each seam
works with zero edits to `orchestrator-core` or `routing`:

- **Add a provider** - implement `ChatProvider` + `ProviderFactory` and register
  it in `builtin_registry()`; any OpenAI-compatible endpoint needs no new adapter
  at all (add it at runtime as a `GenericOpenAI` provider config).
- **Add a routing policy** - implement `RoutingPolicy::decide`, register it in
  the `PolicyRegistry`, and select it active; the manual-override precedence
  wrapper applies automatically.
- **Add an MCP server** - a runtime, no-code path via the Tool/MCP Server
  Manager: specify a stdio command or HTTP/SSE URL and the client handshakes,
  discovers, and exposes tools automatically.

## Lockfile policy

`hybrid-orchestrator/Cargo.lock` and `hybrid-orchestrator/frontend/package-lock.json`
are **committed** to guarantee reproducible builds. When you change dependencies,
commit the updated lockfile in the **same commit** as the dependency change.
Never add these lockfiles to `.gitignore`.

`frontend/package-lock.json` does not exist yet: it cannot be generated in the
offline Phase 0 sandbox (`npm install` requires the npm registry). The first
networked CI run installs via `npm ci || npm install`, which takes the
`npm install` path and produces the lockfile. Commit that generated
`package-lock.json`, then switch CI to a plain `npm ci` and re-enable the
setup-node npm cache (see the note in `.github/workflows/ci.yml`).

## License

MIT
