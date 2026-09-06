//! `persistence` crate: SQLite storage and versioned config schema
//! (architecture.md Section 7.2).
//!
//! [`Db`] wraps a `sqlx` SQLite connection pool and runs versioned migrations
//! at startup (`migrations/`). Typed repositories in [`repositories`] provide
//! CRUD over the Section 7.1 domain models, mapping structured fields to/from
//! JSON TEXT columns. [`config`] persists [`config::AppConfig`] with a
//! `schema_version` (Section 10.4).
//!
//! Secrets are NEVER stored here (Section 7.2 / 9.1): the providers table holds
//! only the `SecretRef` handle string.
//!
//! We use the runtime `sqlx::query(...)` APIs (NOT the compile-time-checked
//! `query!` macros) so no live `DATABASE_URL` is needed at build time.

pub mod config;
pub mod db;
pub mod repositories;

pub use config::AppConfig;
pub use db::{Db, PersistenceError};
pub use repositories::{ConversationRepo, McpServerRepo, MessageRepo, PersonaRepo, ProviderRepo};
