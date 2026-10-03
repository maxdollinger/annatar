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
    // 1.5 `symbols`; 2.2 `symbol_commits` + `symbol_tickets`; 4.3/4.4 add the
    // what/why columns to `symbols`; 5.1 `symbol_vec` (`F32_BLOB` +
    // `libsql_vector_idx`).
];

/// `CREATE TABLE IF NOT EXISTS` statements for the persistent `cache.db`.
pub const CACHE_TABLES: &[&str] = &[
    // 2.3 history cache; 3.2 `tickets`; 4.1 LLM cache and embedding cache.
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
