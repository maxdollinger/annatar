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
    // 1.5 `symbols`. 4.3 adds the chat model's one-line `what` and `why`
    // for methods and constructors: NULL when not described this run, and
    // `why` also NULL when the history gives no reason. 4.4 fills them for
    // types.
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
        content_hash TEXT NOT NULL,
        what TEXT,
        why TEXT
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
    // 3.2 `tickets`: the readers' copy of `ticket_cache` for every key in
    // `symbol_tickets` that the cache knows (content or unavailable), so `show`
    // and MCP never open `cache.db`. Content columns are NULL when
    // `unavailable = 1`. `summary` is the Jira title; 4.2 adds the chat
    // model's English `llm_summary` and `llm_purpose` (NULL when the ticket
    // is unavailable or was not summarised this run; `llm_purpose` is also
    // NULL when the ticket gives no reason).
    "CREATE TABLE tickets (
        key TEXT PRIMARY KEY,
        unavailable INTEGER NOT NULL,
        issue_type TEXT,
        summary TEXT,
        description TEXT,
        parent_key TEXT,
        llm_summary TEXT,
        llm_purpose TEXT
    )",
    // 5.1 adds `symbol_vec`
    // (`F32_BLOB` + `libsql_vector_idx`).
];

/// `CREATE TABLE IF NOT EXISTS` statements for the persistent `cache.db`.
pub const CACHE_TABLES: &[&str] = &[
    // 2.3 `history_cache`, dropped by R3: its rows were computed from a span
    // that started at the declaration, not the Javadoc, under the same key.
    "DROP TABLE IF EXISTS history_cache",
    // R3 `history_cache_v2`: one current row per fqn, keyed by fqn plus the
    // symbol's content hash and the file's last commit sha; `commits` is a JSON
    // array of [`crate::history::Commit`] for the Javadoc-inclusive span.
    "CREATE TABLE IF NOT EXISTS history_cache_v2 (
        fqn TEXT PRIMARY KEY,
        content_hash TEXT NOT NULL,
        file_last_commit TEXT NOT NULL,
        commits TEXT NOT NULL
    )",
    // 3.2 `ticket_cache`: one row per *requested* ticket key (Jira may answer
    // a moved issue with its new key). `unavailable = 1` (with the HTTP
    // `status`) for a 403/404, content columns otherwise. `fetched_at` is UTC;
    // rows never expire, dropping the table refreshes them.
    "CREATE TABLE IF NOT EXISTS ticket_cache (
        key TEXT PRIMARY KEY,
        unavailable INTEGER NOT NULL,
        status INTEGER,
        issue_type TEXT,
        summary TEXT,
        description TEXT,
        parent_key TEXT,
        fetched_at TEXT NOT NULL
    )",
    // 4.1 `llm_cache`: one validated chat reply (`output`, JSON text) per key,
    // a blake3 hash of chat model + reasoning effort + temperature (4.2) +
    // the response type's JSON schema + the full prompt. Failed completions
    // are never stored.
    "CREATE TABLE IF NOT EXISTS llm_cache (
        key TEXT PRIMARY KEY,
        model TEXT NOT NULL,
        output TEXT NOT NULL,
        created_at TEXT NOT NULL
    )",
    // 4.1 `embedding_cache`: one vector per key, a blake3 hash of embedding
    // model + text, stored as `dim` little-endian f32s (libSQL's `F32_BLOB`
    // layout).
    "CREATE TABLE IF NOT EXISTS embedding_cache (
        key TEXT PRIMARY KEY,
        model TEXT NOT NULL,
        dim INTEGER NOT NULL,
        vector BLOB NOT NULL
    )",
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
