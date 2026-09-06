# Agent Harbor

Agent Harbor is a cross-platform desktop application that orchestrates AI chat
providers, MCP tool servers, and routing policies behind a single Tauri (Rust
core) + React/TypeScript (Vite) frontend.

The authoritative design lives in the specs:

- [Hybrid Orchestrator architecture](.kiro/specs/hybrid-orchestrator/architecture.md)
- [Phased task plan (Phase 0 onward)](.kiro/specs/hybrid-orchestrator/tasks.md)

This repository currently contains the **Phase 0 scaffold**: the full crate
layout, a launching Tauri shell, a rendering React frontend, and CI. Feature
logic (Phases 1-6) is not implemented yet; the library crates are empty
placeholders.

## Repository layout

```
agent-harbor/
├── .github/workflows/ci.yml        # CI pipeline (build/test/clippy, vitest, tauri build)
├── .kiro/specs/hybrid-orchestrator # Architecture + task specifications
└── hybrid-orchestrator/            # Cargo workspace + frontend
    ├── Cargo.toml                  # Virtual workspace manifest
    ├── Cargo.lock                  # Committed lockfile (see policy below)
    ├── rustfmt.toml                # Rust formatting config
    ├── crates/
    │   ├── orchestrator-core/      # Session/pipeline/events core (depends on the libs)
    │   ├── providers/              # Provider adapter contracts + adapters
    │   ├── mcp-client/             # MCP transport/session/tools client
    │   ├── routing/                # Routing policies + signals
    │   ├── persistence/            # DB, repositories, config
    │   ├── secrets/                # Secret storage
    │   └── tauri-app/              # Tauri v2 desktop shell (binary)
    └── frontend/                   # React + TypeScript (Vite) UI
        ├── package.json
        ├── package-lock.json       # Committed lockfile (see policy below)
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
cargo build                                   # build the library crates
cargo test                                    # run the crate smoke tests
cargo clippy --all-targets -- -D warnings     # lint (warnings are errors)
cargo fmt --all --check                       # check formatting
```

To build/test the Tauri shell crate explicitly (see the offline note below for
why it is not part of the default workspace commands):

```sh
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

Phase 0 was scaffolded in an **offline (INTEGRATIONS_ONLY) sandbox** where
crates.io and registry.npmjs.org are unreachable. This shapes the layout:

- The **six library crates** (`orchestrator-core`, `providers`, `mcp-client`,
  `routing`, `persistence`, `secrets`) are dependency-free and build, test, and
  lint **fully offline** with the bare `cargo` commands above.
- **`tauri-app`** depends on the external `tauri`/`serde`/`serde_json` crates and
  the frontend depends on the npm registry. Neither can be resolved offline, so
  they are **validated in CI**, where network access exists.

### Workspace `exclude` arrangement for `tauri-app`

Cargo resolves the entire `members` dependency graph to build `Cargo.lock`, even
for crates excluded from `default-members`. Because `tauri` is unreachable
offline, listing `tauri-app` under `members` would make **every** bare cargo
command fail. To keep the offline baseline green, `tauri-app` is placed under
`[workspace] exclude` in `hybrid-orchestrator/Cargo.toml` (not `members`).

A consequence: `cargo build --workspace` and `cargo build -p tauri-app` do **not**
see the excluded crate. CI therefore builds it explicitly via
`cargo build --manifest-path crates/tauri-app/Cargo.toml`, and the `tauri build`
step compiles it as part of bundling.

**Known tradeoff (tracked, not permanent).** Because the crate is `exclude`d
rather than a non-default `members` entry, its Section 3.3 dependency edges are
validated only in CI and its dependencies never enter
`hybrid-orchestrator/Cargo.lock`, so a breaking change to a leaf crate's public
API that affects `tauri-app` will not surface in the offline default build.

**Exit path (once network access exists).** Move `crates/tauri-app` from
`exclude` into `members` in `hybrid-orchestrator/Cargo.toml` (keeping it out of
`default-members` so the bare offline `cargo build` still skips it), run a
networked `cargo build -p tauri-app` to populate `Cargo.lock` with tauri's
dependency graph, and commit the updated `Cargo.lock` in the same change. This
reconnects the crate to the workspace lockfile without breaking the offline
default build.

## Continuous integration

[`.github/workflows/ci.yml`](.github/workflows/ci.yml) runs on every push and
pull request across an OS matrix (`ubuntu-latest`, `macos-latest`,
`windows-latest`). It runs `cargo fmt --check`, `cargo build/test/clippy` over
the workspace **and** the excluded `tauri-app` crate, the frontend `vitest`
suite (plus ESLint and Prettier checks), and finally `tauri build` to produce an
installer. CI is the authoritative place the full build is exercised.

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
