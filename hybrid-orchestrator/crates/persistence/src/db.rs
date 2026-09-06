//! Database pool and migrations (architecture.md Section 7.2).
//!
//! [`Db`] owns a `sqlx::SqlitePool` and runs the versioned SQL migrations in
//! `migrations/` at startup via `sqlx::migrate!`. Migrations are recorded by
//! sqlx in its `_sqlx_migrations` table, so re-opening an existing database is
//! idempotent (already-applied migrations are skipped).

use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteSynchronous,
};

/// How long a busy SQLite connection waits for a competing writer to release
/// the write lock before returning `SQLITE_BUSY` (architecture.md Section 7.5).
///
/// The file-backed pool opens multiple connections, so writes to DIFFERENT
/// conversations can land on DIFFERENT connections and contend at the SQLite
/// layer (the `SessionManager`'s per-conversation async lock only serializes
/// writes to the SAME conversation). Combined with WAL journaling below, a
/// short busy-timeout lets a briefly-blocked writer wait for the lock instead
/// of immediately failing.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// Errors surfaced by the persistence layer.
#[derive(Debug, thiserror::Error)]
pub enum PersistenceError {
    /// A `sqlx` query / connection error.
    #[error("database error: {0}")]
    Sqlx(#[from] sqlx::Error),
    /// A migration failed to apply.
    #[error("migration error: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
    /// A row's JSON column failed to (de)serialize into a domain model.
    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),
    /// A requested row was not found.
    #[error("not found: {0}")]
    NotFound(String),
}

/// The embedded set of versioned migrations (the `migrations/` directory).
static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// A handle to the SQLite-backed store.
#[derive(Debug, Clone)]
pub struct Db {
    pool: SqlitePool,
}

impl Db {
    /// Open (creating if missing) the SQLite database at `path` and run all
    /// pending migrations.
    pub async fn open(path: impl AsRef<Path>) -> Result<Self, PersistenceError> {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            // Enforce FK constraints (messages -> conversations ON DELETE CASCADE).
            .foreign_keys(true)
            // WAL journaling lets readers proceed concurrently with a writer and
            // is the recommended mode for a multi-connection pool (Section 7.5).
            .journal_mode(SqliteJournalMode::Wal)
            // NORMAL is the safe, standard synchronous level to pair with WAL.
            .synchronous(SqliteSynchronous::Normal)
            // Wait (rather than immediately erroring with SQLITE_BUSY) when a
            // competing writer on another pool connection holds the write lock.
            // Cross-conversation writes land on different connections, so
            // without this they could collide; the SessionManager's
            // per-conversation lock only serializes SAME-conversation writes.
            .busy_timeout(BUSY_TIMEOUT);
        Self::connect(options).await
    }

    /// Open an in-memory database (per-connection scratch store) and run
    /// migrations. Handy for tests; the data is discarded when the pool drops.
    ///
    /// A single connection is used so all statements share the one in-memory
    /// database (each SQLite `:memory:` connection is otherwise distinct).
    pub async fn open_in_memory() -> Result<Self, PersistenceError> {
        let options = SqliteConnectOptions::from_str("sqlite::memory:")
            .expect("static in-memory sqlite url is valid")
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await?;
        let db = Db { pool };
        db.run_migrations().await?;
        Ok(db)
    }

    async fn connect(options: SqliteConnectOptions) -> Result<Self, PersistenceError> {
        let pool = SqlitePoolOptions::new().connect_with(options).await?;
        let db = Db { pool };
        db.run_migrations().await?;
        Ok(db)
    }

    /// Run all pending migrations. Idempotent.
    pub async fn run_migrations(&self) -> Result<(), PersistenceError> {
        MIGRATOR.run(&self.pool).await?;
        Ok(())
    }

    /// Borrow the underlying pool for the repositories.
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }
}
