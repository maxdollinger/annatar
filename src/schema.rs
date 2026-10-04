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
    // 1.5 `symbols`. 4.6 (product-owner redesign) adds the chat model's
    // `description` of every method, constructor and type, NULL when not
    // described this run (it replaced 4.3/4.4's `what` and `why`).
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
        description TEXT
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
    // 5.1 `symbol_vectors` is not here: its `F32_BLOB(<dim>)` column needs
    // the embedding dimension, known only from the first embedding response,
    // so the embeddings stage creates it with [`vector_tables`].
    // 5.1 review `index_meta`: settings the index was built with, one row per
    // key (see [`META_EMBEDDING_MODEL`] and its siblings), so a reader can
    // tell whether its query vectors match the stored ones.
    "CREATE TABLE index_meta (
        key TEXT PRIMARY KEY,
        value TEXT NOT NULL
    )",
];

/// `index_meta` key: the embedding model that produced `symbol_vectors`.
/// Written by the embeddings stage with the other `META_EMBEDDING_*` keys
/// only when the index has vectors.
pub const META_EMBEDDING_MODEL: &str = "embedding_model";
/// `index_meta` key: the length of every vector in `symbol_vectors`.
pub const META_EMBEDDING_DIM: &str = "embedding_dim";
/// `index_meta` key: `true` or `false`, the `[embedding] parent_description`
/// setting the embedded texts were built with.
pub const META_EMBEDDING_PARENT_DESCRIPTION: &str = "embedding_parent_description";

/// `max_neighbors` of the vector index. libSQL's default for 1024
/// dimensions is 96, about 105 KB of graph per symbol with float8
/// neighbours; 32 keeps recall@10 at 1.0 on `argus` and on a 6× larger
/// synthetic set (16 drops to 0.998 there) at about 42 KB per symbol.
pub const VECTOR_MAX_NEIGHBORS: usize = 32;

/// Name of the vector index over `symbol_vectors.embedding`, the first
/// argument of `vector_top_k`.
pub const VECTOR_INDEX: &str = "symbol_vectors_embedding";

/// 5.1 `symbol_vectors`: one embedding of `dim` 32-bit floats per symbol
/// that has a description (its `symbol_id` is the rowid `vector_top_k`
/// returns), with a cosine `libsql_vector_idx` index (float8 neighbours,
/// [`VECTOR_MAX_NEIGHBORS`]). Part of `index.db`, but created by the
/// embeddings stage once the dimension is known; an index without any vector
/// has no such table. `vector_top_k` returns at most about 200 rows
/// whatever its `k` (libSQL's default search list), so a filtered query
/// cannot over-fetch the whole table through it.
pub fn vector_tables(dim: usize) -> [String; 2] {
    [
        format!(
            "CREATE TABLE symbol_vectors (
        symbol_id INTEGER PRIMARY KEY REFERENCES symbols(id),
        embedding F32_BLOB({dim}) NOT NULL
    )"
        ),
        format!(
            "CREATE INDEX {VECTOR_INDEX} ON symbol_vectors(libsql_vector_idx(embedding, 'metric=cosine', 'compress_neighbors=float8', 'max_neighbors={VECTOR_MAX_NEIGHBORS}'))"
        ),
    ]
}

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

/// Page cache of a build connection that writes vectors, in KiB (a cap, not
/// an allocation). The vector index rewrites graph nodes of several KiB on
/// every insert; with SQLite's 2 MB default the open transaction spills
/// pages to the temporary file on each one, which made 654 inserts of 1024
/// dimensions take 13 s on a slow (container-mounted) disk instead of 1.4 s
/// (default neighbours), and still 1.6 s instead of 0.6 s with
/// [`VECTOR_MAX_NEIGHBORS`].
pub const VECTOR_CACHE_KIB: i64 = 256 * 1024;

/// Create `symbol_vectors` and its vector index for `dim`-dimensional
/// embeddings on a build connection, and raise its page cache to
/// [`VECTOR_CACHE_KIB`]; see [`vector_tables`].
pub async fn create_vectors(conn: &Connection, dim: usize) -> Result<()> {
    anyhow::ensure!(dim > 0, "an embedding needs at least one dimension");
    conn.execute(&format!("PRAGMA cache_size = -{VECTOR_CACHE_KIB}"), ())
        .await
        .context("raising the page cache for the vector index")?;
    for statement in vector_tables(dim) {
        conn.execute(&statement, ())
            .await
            .with_context(|| format!("creating vector table: {statement}"))?;
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

#[cfg(test)]
mod tests {
    use libsql::params;

    use super::*;
    use crate::store::Store;

    #[tokio::test]
    async fn vector_table_takes_f32_blobs_and_answers_top_k_queries() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).await.unwrap();
        let build = store.begin_index().await.unwrap();
        let conn = build.connection();
        create_vectors(conn, 3).await.unwrap();
        for (id, fqn, vector) in [
            (1, "com.acme.A", [1.0f32, 0.0, 0.0]),
            (2, "com.acme.B", [0.0, 1.0, 0.0]),
            (3, "com.acme.C", [0.9, 0.1, 0.0]),
        ] {
            conn.execute(
                "INSERT INTO symbols (id, kind, fqn, file, start_line, end_line, signature, content_hash)
                 VALUES (?1, 'class', ?2, 'A.java', 1, 1, '', '')",
                params![id, fqn],
            )
            .await
            .unwrap();
            let blob: Vec<u8> = vector.iter().flat_map(|v| v.to_le_bytes()).collect();
            conn.execute(
                "INSERT INTO symbol_vectors (symbol_id, embedding) VALUES (?1, vector32(?2))",
                params![id, blob],
            )
            .await
            .unwrap();
        }

        let mut rows = conn
            .query(
                &format!(
                    "SELECT s.fqn FROM vector_top_k('{VECTOR_INDEX}', vector32('[1, 0, 0]'), 2) AS v
                     JOIN symbols s ON s.id = v.id"
                ),
                (),
            )
            .await
            .unwrap();
        let mut fqns = Vec::new();
        while let Some(row) = rows.next().await.unwrap() {
            fqns.push(row.get::<String>(0).unwrap());
        }
        assert_eq!(fqns, vec!["com.acme.A", "com.acme.C"]);

        let err = conn
            .execute(
                "UPDATE symbol_vectors SET embedding = vector32('[1, 0]') WHERE symbol_id = 1",
                (),
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("dimension"), "{err}");
        assert!(create_vectors(conn, 0).await.is_err());

        let mut rows = conn
            .query(
                "SELECT sql FROM sqlite_master WHERE name = ?1",
                params![VECTOR_INDEX],
            )
            .await
            .unwrap();
        let sql = rows
            .next()
            .await
            .unwrap()
            .unwrap()
            .get::<String>(0)
            .unwrap();
        assert!(
            sql.contains(&format!("'max_neighbors={VECTOR_MAX_NEIGHBORS}'")),
            "{sql}"
        );
    }
}
