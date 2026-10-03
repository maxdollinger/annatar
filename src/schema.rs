//! Database schemas for the two files [`crate::store`] manages.
//!
//! `index.db` is rebuilt from scratch on every run, so its schema is a plain
//! list of `CREATE TABLE` statements applied when a build begins: no
//! migrations, no versioning, no `IF NOT EXISTS`. `cache.db` persists, so its
//! tables use `CREATE TABLE IF NOT EXISTS`; when a cache table's shape
//! changes, drop that table and let it refill (ground rules).
//!
//! Every phase adds its own tables here, keeping all DDL in one reviewable
//! place instead of spread across phases 1–5.

use anyhow::{Context, Result};
use libsql::Connection;

/// `CREATE TABLE` statements for the disposable `index.db`, in dependency
/// order.
pub const INDEX_TABLES: &[&str] = &[
    // 1.5 `symbols`.
    "CREATE TABLE symbols (
        id INTEGER PRIMARY KEY,
        parent_id INTEGER REFERENCES symbols(id),
        kind TEXT NOT NULL,
        role TEXT,
        fqn TEXT NOT NULL UNIQUE,
        file TEXT NOT NULL,
        start_line INTEGER NOT NULL,
        end_line INTEGER NOT NULL,
        signature TEXT NOT NULL,
        javadoc TEXT,
        annotations TEXT NOT NULL DEFAULT '[]',
        content_hash TEXT NOT NULL
    )",
    "CREATE INDEX symbols_parent_id ON symbols(parent_id)",
    // 2.2 `symbol_commits`.
    "CREATE TABLE symbol_commits (
        id INTEGER PRIMARY KEY,
        symbol_id INTEGER NOT NULL REFERENCES symbols(id),
        sha TEXT NOT NULL,
        date TEXT NOT NULL,
        subject TEXT NOT NULL,
        UNIQUE(symbol_id, sha)
    )",
    "CREATE INDEX symbol_commits_symbol_id ON symbol_commits(symbol_id)",
    // 2.2 `symbol_tickets`.
    "CREATE TABLE symbol_tickets (
        id INTEGER PRIMARY KEY,
        symbol_id INTEGER NOT NULL REFERENCES symbols(id),
        ticket_key TEXT NOT NULL,
        first_date TEXT NOT NULL,
        last_date TEXT NOT NULL,
        UNIQUE(symbol_id, ticket_key)
    )",
    "CREATE INDEX symbol_tickets_symbol_id ON symbol_tickets(symbol_id)",
    // 4.3/4.4 add the what/why columns to `symbols`; 5.1 adds `symbol_vec`
    // (`F32_BLOB` + `libsql_vector_idx`).
];

/// `CREATE TABLE IF NOT EXISTS` statements for the persistent `cache.db`.
pub const CACHE_TABLES: &[&str] = &[
    // 2.3 `history_cache`: one current row per fqn, keyed by fqn plus the
    // symbol's content hash and the file's last commit sha; `commits` is a JSON
    // array of [`crate::history::Commit`].
    "CREATE TABLE IF NOT EXISTS history_cache (
        fqn TEXT PRIMARY KEY,
        content_hash TEXT NOT NULL,
        file_last_commit TEXT NOT NULL,
        commits TEXT NOT NULL
    )",
    // 3.2 `tickets`; 4.1 LLM cache and embedding cache.
];

/// Create every index table on a fresh build connection. Called by
/// [`crate::store::Store::begin_index`].
pub async fn create_index(conn: &Connection) -> Result<()> {
    for statement in INDEX_TABLES {
        conn.execute(statement, ())
            .await
            .with_context(|| format!("creating index table: {statement}"))?;
    }
    Ok(())
}

/// Create any missing cache tables. Called by
/// [`crate::store::Store::open`].
pub async fn create_cache(conn: &Connection) -> Result<()> {
    for statement in CACHE_TABLES {
        conn.execute(statement, ())
            .await
            .with_context(|| format!("creating cache table: {statement}"))?;
    }
    Ok(())
}
