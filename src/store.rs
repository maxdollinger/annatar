use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use libsql::{Builder, Connection, Database};
use tempfile::NamedTempFile;

/// Rebuilt from scratch by every index run.
pub const INDEX_DB: &str = "index.db";
/// Persists across runs and holds only expensive, content-keyed results.
pub const CACHE_DB: &str = "cache.db";

/// Opens the two database files and manages the index rebuild lifecycle.
///
/// `<data_dir>/cache.db` is opened once and shared. `<data_dir>/index.db` is
/// never written in place: a run builds a temporary file next to it and
/// atomically renames it over the old index when [`Store::finish_index`] is
/// called. An aborted run drops the temporary file and leaves the previous
/// index untouched.
pub struct Store {
    data_dir: PathBuf,
    cache: Connection,
    // Anchors the cache connection; never read directly.
    _cache_db: Database,
    index: Option<IndexBuild>,
}

struct IndexBuild {
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
        Ok(Self {
            data_dir,
            cache,
            _cache_db: cache_db,
            index: None,
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

    /// Start a fresh index build in a temporary file. Any previous unfinished
    /// build is discarded. Later phases create their index tables on the
    /// returned connection.
    pub async fn begin_index(&mut self) -> Result<&Connection> {
        self.index = None;
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
        self.index = Some(IndexBuild {
            conn,
            db,
            temp,
            final_path,
        });
        Ok(&self.index.as_ref().expect("just set").conn)
    }

    /// The in-progress index connection, if a build has been started.
    pub fn index(&self) -> Option<&Connection> {
        self.index.as_ref().map(|build| &build.conn)
    }

    /// Close the temporary index and atomically rename it over `index.db`.
    pub fn finish_index(&mut self) -> Result<()> {
        let Some(build) = self.index.take() else {
            bail!("no index build in progress");
        };
        let IndexBuild {
            db,
            conn,
            temp,
            final_path,
        } = build;
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

    async fn write_index(store: &mut Store, table: &str, value: &str) {
        let conn = store.begin_index().await.unwrap();
        conn.execute(&format!("CREATE TABLE {table} (v TEXT)"), ())
            .await
            .unwrap();
        conn.execute(&format!("INSERT INTO {table} VALUES ('{value}')"), ())
            .await
            .unwrap();
        store.finish_index().unwrap();
    }

    #[tokio::test]
    async fn finished_run_replaces_index_and_aborted_run_keeps_the_old_one() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).await.unwrap();
        let index_path = store.index_path();
        drop(store);

        let mut store = Store::open(dir.path()).await.unwrap();
        write_index(&mut store, "t", "first").await;
        assert!(index_path.exists(), "finished run should create index.db");
        drop(store);

        // A second run that never finishes must not touch the committed index.
        let mut store = Store::open(dir.path()).await.unwrap();
        let conn = store.begin_index().await.unwrap();
        conn.execute("CREATE TABLE t2 (v TEXT)", ()).await.unwrap();
        conn.execute("INSERT INTO t2 VALUES ('second')", ())
            .await
            .unwrap();
        drop(store);

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
    async fn cache_survives_rebuilds_and_aborts() {
        let dir = tempfile::tempdir().unwrap();

        let mut store = Store::open(dir.path()).await.unwrap();
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
        write_index(&mut store, "t", "run1").await;
        drop(store);

        // Another run aborts while a rebuild is in progress.
        let mut store = Store::open(dir.path()).await.unwrap();
        let conn = store.begin_index().await.unwrap();
        conn.execute("CREATE TABLE t (v TEXT)", ()).await.unwrap();
        drop(store);

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
    async fn begin_index_discards_a_previous_unfinished_build() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path()).await.unwrap();

        store.begin_index().await.unwrap();
        let conn = store.begin_index().await.unwrap();
        conn.execute("CREATE TABLE fresh (v TEXT)", ())
            .await
            .unwrap();

        let leftover: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().to_string())
            .filter(|name| name.starts_with(".index-"))
            .collect();
        assert!(
            leftover.len() <= 1,
            "only the current temporary index should exist, found {leftover:?}"
        );
    }

    #[tokio::test]
    async fn finish_without_begin_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path()).await.unwrap();
        assert!(store.finish_index().is_err());
    }
}
