use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use libsql::{Builder, Connection, Database};
use tempfile::NamedTempFile;

use crate::schema;

/// Rebuilt from scratch by every index run.
pub const INDEX_DB: &str = "index.db";
/// Persists across runs and holds only expensive, content-keyed results.
pub const CACHE_DB: &str = "cache.db";

/// Opens the two database files and manages the index rebuild lifecycle.
///
/// `<data_dir>/cache.db` is opened once and shared. `<data_dir>/index.db` is
/// never written in place: [`Store::begin_index`] hands out an owned
/// [`IndexBuild`] that writes to a temporary file next to the real index, and
/// [`IndexBuild::commit`] atomically renames it over `index.db`. A build that
/// is dropped instead of committed deletes its temporary file, leaving the
/// previous index untouched.
pub struct Store {
    data_dir: PathBuf,
    cache: Connection,
    // Anchors the cache connection; never read directly.
    _cache_db: Database,
}

/// An owned, in-progress index build. Hold it for the whole run, write tables
/// through [`IndexBuild::connection`], then call [`IndexBuild::commit`] once at
/// the end. Dropping it aborts the build.
pub struct IndexBuild {
    conn: Connection,
    db: Database,
    temp: NamedTempFile,
    final_path: PathBuf,
}

impl Store {
    /// Open the data directory, creating it if needed, and the persistent
    /// cache database.
    pub async fn open(data_dir: impl AsRef<Path>) -> Result<Self> {
        let data_dir = data_dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&data_dir)
            .with_context(|| format!("creating data directory {}", data_dir.display()))?;
        let cache_path = data_dir.join(CACHE_DB);
        let cache_db = Builder::new_local(&cache_path)
            .build()
            .await
            .with_context(|| format!("opening cache database {}", cache_path.display()))?;
        let cache = cache_db.connect().context("connecting to cache database")?;
        schema::create_cache(&cache)
            .await
            .context("creating cache schema")?;
        Ok(Self {
            data_dir,
            cache,
            _cache_db: cache_db,
        })
    }

    /// The persistent cache connection. Later phases create their own tables
    /// here with `CREATE TABLE IF NOT EXISTS`.
    pub fn cache(&self) -> &Connection {
        &self.cache
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn index_path(&self) -> PathBuf {
        self.data_dir.join(INDEX_DB)
    }

    pub fn cache_path(&self) -> PathBuf {
        self.data_dir.join(CACHE_DB)
    }

    /// Open a read-only connection to the committed `index.db`.
    ///
    /// This never creates the file or its schema: reading before a run has
    /// committed an index is an error, pointing the caller at `annatar index`.
    pub async fn open_index(&self) -> Result<Connection> {
        let path = self.index_path();
        if !path.exists() {
            anyhow::bail!(
                "index database {} does not exist; run `annatar index` first",
                path.display()
            );
        }
        let db = Builder::new_local(&path)
            .build()
            .await
            .with_context(|| format!("opening index database {}", path.display()))?;
        let conn = db
            .connect()
            .with_context(|| format!("connecting to index database {}", path.display()))?;
        enable_foreign_keys(&conn).await?;
        Ok(conn)
    }

    /// Start a fresh index build in a temporary file. The returned
    /// [`IndexBuild`] is owned by the caller and must be committed to become
    /// the new `index.db`. Later phases create their index tables on the
    /// build's connection.
    pub async fn begin_index(&self) -> Result<IndexBuild> {
        let final_path = self.index_path();
        let temp = tempfile::Builder::new()
            .prefix(".index-")
            .suffix(".db")
            .tempfile_in(&self.data_dir)
            .context("creating temporary index file")?;
        let db = Builder::new_local(temp.path())
            .build()
            .await
            .context("opening temporary index database")?;
        let conn = db.connect().context("connecting to temporary index")?;
        enable_foreign_keys(&conn).await?;
        schema::create_index(&conn)
            .await
            .context("creating index schema")?;
        Ok(IndexBuild {
            conn,
            db,
            temp,
            final_path,
        })
    }
}

/// Turn on foreign-key enforcement for one connection.
///
/// SQLite/libSQL enforce foreign keys per connection, and `PRAGMA foreign_keys`
/// is a no-op inside a transaction, so this must run immediately after
/// `connect`, before any write. It does not create or alter any table; the
/// declared `REFERENCES` clauses are the schema.
async fn enable_foreign_keys(conn: &Connection) -> Result<()> {
    conn.execute("PRAGMA foreign_keys = ON", ())
        .await
        .context("enabling foreign key enforcement")?;
    Ok(())
}

impl IndexBuild {
    /// The connection to the in-progress index. Create and fill tables here.
    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    /// Close the temporary index and atomically rename it over `index.db`.
    pub fn commit(self) -> Result<()> {
        let IndexBuild {
            conn,
            db,
            temp,
            final_path,
        } = self;
        drop(conn);
        drop(db);
        temp.persist(&final_path)
            .map_err(|err| err.error)
            .with_context(|| format!("replacing {}", final_path.display()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn first_string(conn: &Connection, sql: &str) -> Option<String> {
        let mut rows = conn.query(sql, ()).await.unwrap();
        let row = rows.next().await.unwrap()?;
        Some(row.get::<String>(0).unwrap())
    }

    async fn write_index(store: &Store, table: &str, value: &str) {
        let build = store.begin_index().await.unwrap();
        build
            .connection()
            .execute(&format!("CREATE TABLE {table} (v TEXT)"), ())
            .await
            .unwrap();
        build
            .connection()
            .execute(&format!("INSERT INTO {table} VALUES ('{value}')"), ())
            .await
            .unwrap();
        build.commit().unwrap();
    }

    fn temp_files(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().to_string())
            .filter(|name| name.starts_with(".index-"))
            .collect()
    }

    async fn symbol_count(build: &IndexBuild) -> i64 {
        let mut rows = build
            .connection()
            .query("SELECT COUNT(*) FROM symbols", ())
            .await
            .unwrap();
        rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap()
    }

    #[tokio::test]
    async fn build_connection_rejects_a_dangling_parent() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).await.unwrap();
        let build = store.begin_index().await.unwrap();

        let err = build
            .connection()
            .execute(
                "INSERT OR IGNORE INTO symbols
                    (parent_id, kind, fqn, file, start_line, end_line, signature, content_hash)
                 VALUES (999, 'class', 'com.acme.Ghost', 'Ghost.java', 1, 1, 'class Ghost', 'hash')",
                (),
            )
            .await
            .expect_err("FK enforcement should reject a parent_id that names no row");
        assert!(
            format!("{err:#}").contains("FOREIGN KEY"),
            "error should be an FK violation, got: {err:#}"
        );
        assert_eq!(
            symbol_count(&build).await,
            0,
            "a dangling parent must never be stored"
        );
    }

    #[tokio::test]
    async fn build_connection_accepts_a_self_referential_parent() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).await.unwrap();
        let build = store.begin_index().await.unwrap();

        build
            .connection()
            .execute(
                "INSERT INTO symbols
                    (id, parent_id, kind, fqn, file, start_line, end_line, signature, content_hash)
                 VALUES (1, NULL, 'class', 'com.acme.Parent', 'Parent.java', 1, 1, 'class Parent', 'hash')",
                (),
            )
            .await
            .expect("a top-level symbol has no parent to reference");

        build
            .connection()
            .execute(
                "INSERT INTO symbols
                    (id, parent_id, kind, fqn, file, start_line, end_line, signature, content_hash)
                 VALUES (2, 1, 'method', 'com.acme.Parent#child()', 'Parent.java', 2, 2, 'void child()', 'hash')",
                (),
            )
            .await
            .expect("a child may reference the parent row already present");

        assert_eq!(symbol_count(&build).await, 2, "both rows should be stored");
    }

    #[tokio::test]
    async fn committed_build_replaces_index_and_aborted_build_keeps_the_old_one() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).await.unwrap();
        let index_path = store.index_path();
        drop(store);

        let store = Store::open(dir.path()).await.unwrap();
        write_index(&store, "t", "first").await;
        assert!(
            index_path.exists(),
            "committed build should create index.db"
        );
        drop(store);

        // A second run that never commits must not touch the committed index.
        let store = Store::open(dir.path()).await.unwrap();
        let build = store.begin_index().await.unwrap();
        build
            .connection()
            .execute("CREATE TABLE t2 (v TEXT)", ())
            .await
            .unwrap();
        build
            .connection()
            .execute("INSERT INTO t2 VALUES ('second')", ())
            .await
            .unwrap();
        drop(build);

        let db = Builder::new_local(&index_path).build().await.unwrap();
        let conn = db.connect().unwrap();
        assert_eq!(
            first_string(&conn, "SELECT v FROM t").await.as_deref(),
            Some("first"),
            "aborted run should leave the previous index intact"
        );
        assert!(
            first_string(
                &conn,
                "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 't2'"
            )
            .await
            .is_none(),
            "aborted run should not leak its data into index.db"
        );
    }

    #[tokio::test]
    async fn open_index_errors_before_the_first_build_and_reads_after_one() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).await.unwrap();

        let err = store
            .open_index()
            .await
            .expect_err("there is no index before the first build");
        assert!(
            format!("{err:#}").contains("does not exist"),
            "error should say the index is missing, got: {err:#}"
        );

        write_index(&store, "t", "one").await;
        let conn = store.open_index().await.unwrap();
        assert_eq!(
            first_string(&conn, "SELECT v FROM t").await.as_deref(),
            Some("one"),
            "the read connection should see the committed index"
        );
    }

    #[tokio::test]
    async fn cache_survives_rebuilds_and_aborts() {
        let dir = tempfile::tempdir().unwrap();

        let store = Store::open(dir.path()).await.unwrap();
        store
            .cache()
            .execute("CREATE TABLE c (v TEXT)", ())
            .await
            .unwrap();
        store
            .cache()
            .execute("INSERT INTO c VALUES ('keep')", ())
            .await
            .unwrap();
        write_index(&store, "t", "run1").await;
        drop(store);

        // Another run aborts while a rebuild is in progress.
        let store = Store::open(dir.path()).await.unwrap();
        let build = store.begin_index().await.unwrap();
        build
            .connection()
            .execute("CREATE TABLE t (v TEXT)", ())
            .await
            .unwrap();
        drop(build);

        let store = Store::open(dir.path()).await.unwrap();
        assert_eq!(
            first_string(store.cache(), "SELECT v FROM c")
                .await
                .as_deref(),
            Some("keep"),
            "cache.db should survive both rebuilds and aborts"
        );
    }

    #[tokio::test]
    async fn aborted_build_leaves_no_temp_file_or_index() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).await.unwrap();

        let build = store.begin_index().await.unwrap();
        build
            .connection()
            .execute("CREATE TABLE fresh (v TEXT)", ())
            .await
            .unwrap();
        assert_eq!(
            temp_files(dir.path()).len(),
            1,
            "an in-progress build should own exactly one temporary file"
        );
        drop(build);

        assert!(
            temp_files(dir.path()).is_empty(),
            "aborting a build should delete its temporary file"
        );
        assert!(
            !store.index_path().exists(),
            "aborting a build should never create index.db"
        );
    }
}
