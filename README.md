# Agent Harbor

Agent Harbor is a cross-platform desktop application that orchestrates AI chat
providers, MCP tool servers, and routing policies behind a single Tauri (Rust
core) + React/TypeScript (Vite) frontend.

The authoritative design lives in the specs:

- [Hybrid Orchestrator architecture](.kiro/specs/hybrid-orchestrator/architecture.md)
- [Phased task plan (Phase 0 onward)](.kiro/specs/hybrid-orchestrator/tasks.md)

## Implementation status

Phases 0 through 3 are implemented and merged; Phases 4 through 6 are pending.

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
- **Phase 4 - Routing policy engine + full message pipeline**: PENDING.
- **Phase 5 - UI surfaces**: PENDING.
- **Phase 6 - Security hardening + extensibility validation**: PENDING.

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
    │   ├── routing/                # Routing policies + signals (Phase 4, offline member)
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

```sh
cd hybrid-orchestrator
cargo build                                   # build the offline member crate (routing)
cargo test                                    # run the crate smoke tests
cargo clippy --all-targets -- -D warnings     # lint (warnings are errors)
cargo fmt --all --check                       # check formatting
```

The crates that carry crates.io dependencies are under `[workspace] exclude`
(see the offline note below), so the bare workspace commands above do not touch
them. Build/test them explicitly via their manifests, for example:

```sh
cargo build --manifest-path crates/orchestrator-core/Cargo.toml
cargo build --manifest-path crates/tauri-app/Cargo.toml
```

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
cargo tauri dev --config crates/tauri-app/tauri.conf.json     # dev shell
cargo tauri build --config crates/tauri-app/tauri.conf.json   # signed installer
```

## Sandbox / offline constraint

The project was originally scaffolded in an **offline (INTEGRATIONS_ONLY)
sandbox** where crates.io and registry.npmjs.org are unreachable. This still
shapes the workspace layout as feature crates gained real dependencies through
Phases 1 through 3:

- **`routing`** is the sole crate that remains dependency-free, so it stays a
  `[workspace] members` entry and builds, tests, and lints **fully offline**
  with the bare `cargo` commands above.
- The crates that now carry crates.io dependencies (`domain`,
  `orchestrator-core`, `persistence`, `secrets`, `providers`, `mcp-client`, and
  `tauri-app`) are unreachable offline, so they are placed under
  `[workspace] exclude` and **validated in CI**, where network access exists.
- The frontend depends on the npm registry and is likewise validated in CI.

### Workspace `exclude` arrangement for the networked crates

Cargo resolves the entire `members` dependency graph to build `Cargo.lock`, even
for crates excluded from `default-members`. Because the crates listed above pull
in crates.io dependencies (for example `tauri`/`tauri-build`, `sqlx`, `keyring`,
`reqwest`, `tokio`, `aws-sigv4`) that are unreachable offline, listing them under
`members` would make **every** bare cargo command fail. To keep the offline
baseline green, they are placed under `[workspace] exclude` in
`hybrid-orchestrator/Cargo.toml` (not `members`); only the dependency-free
`routing` crate remains a `members` entry.

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
