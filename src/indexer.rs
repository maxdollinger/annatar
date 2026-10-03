//! Build `index.db` from a repository's Java sources.
//!
//! [`build_index`] walks the production files, parses each one with a single
//! reusable [`crate::symbols::JavaParser`] and writes every symbol to the
//! `symbols` table of a fresh index build. The whole run happens on
//! [`crate::store::IndexBuild`]'s temporary file and only becomes `index.db`
//! when it commits; an error drops the build and leaves the previous index
//! untouched.
//!
//! Symbols are keyed by their fully qualified name, which is unique and stable
//! across runs. The parser returns symbols in pre-order, so a symbol is always
//! written before its members and nested types; an in-memory map from fqn to row
//! id supplies each child's `parent_id`. A file that cannot be read or parsed is
//! counted in its own bucket, never fatal.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use libsql::{Connection, params};
use regex::Regex;

use crate::history::{self, Commit, HistoryCache};
use crate::store::Store;
use crate::symbols::{JavaParser, Symbol};
use crate::walk;

/// A summary of one index run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IndexStats {
    /// Files that produced at least one symbol.
    pub files: usize,
    /// Symbol rows written to `symbols`.
    pub symbols: usize,
    /// Commit rows written to `symbol_commits`.
    pub commits: usize,
    /// Ticket-key rows written to `symbol_tickets`.
    pub tickets: usize,
    /// Symbols whose history came from the history cache without a git call.
    pub history_hits: usize,
    /// Symbols whose history needed a `git log -L` (no usable cache row).
    pub history_misses: usize,
    /// Symbols in a git repository indexed without history: their file has no
    /// commit (untracked) or its lookup failed. On a git repository,
    /// `history_hits + history_misses + history_skipped == symbols`.
    pub history_skipped: usize,
    /// Files that parsed cleanly but produced no symbols (`package-info.java`).
    pub empty: usize,
    /// Files tree-sitter could not parse, or for which it produced no tree.
    pub parse_errors: usize,
    /// Files whose contents could not be read.
    pub unreadable: usize,
}

/// Index every production Java file under `repo` into a fresh `index.db`.
///
/// `path_prefix`, when given, limits the run to a part of the repository.
/// `ticket_regex` extracts Jira keys from commit subjects and bodies; it is
/// compiled by the caller so a bad pattern fails before the run starts.
///
/// History is cached in `cache.db` keyed by fqn plus the symbol's content hash
/// and its file's last commit sha, so a repeat run over unchanged sources
/// reuses stored commits without a `git log -L`. A cache write goes to the
/// persistent cache connection, not the build transaction, so an aborted index
/// cannot roll it back; the temporary `index.db` still holds the only copy of
/// the symbol rows.
///
/// When `repo` is not a git work tree the run degrades to a structure-only
/// index: one warning, no history tables, no error. When it is a repo, a file
/// with no commit (untracked) or whose last-commit lookup fails gets one
/// warning naming it and is indexed without history; a cache fault is treated
/// as a miss. Neither can abort the whole index. A file with uncommitted
/// changes still gets history, which may be mis-attributed because `-L`
/// resolves its span against `HEAD`, but that history is never cached.
pub async fn build_index(
    store: &Store,
    repo: &Path,
    path_prefix: Option<&Path>,
    ticket_regex: &Regex,
) -> Result<IndexStats> {
    anyhow::ensure!(
        repo.is_dir(),
        "repository path {} is not a directory",
        repo.display()
    );
    let is_repo = history::is_repository(repo);
    if !is_repo {
        tracing::warn!(
            repo = %repo.display(),
            "not a git work tree; indexing structure only (no history)"
        );
    }
    let files = walk::java_files(repo, path_prefix)?;
    let dirty = if is_repo {
        dirty_files(repo, &files)
    } else {
        HashSet::new()
    };
    let build = store.begin_index().await?;
    let cache = HistoryCache::new(store.cache());
    let mut stats = IndexStats::default();
    let mut parser = JavaParser::new()?;
    let mut ids: HashMap<String, i64> = HashMap::new();

    let transaction = build
        .connection()
        .transaction()
        .await
        .context("starting index transaction")?;

    for relative in &files {
        let source = match std::fs::read_to_string(repo.join(relative)) {
            Ok(source) => source,
            Err(err) => {
                tracing::warn!(path = %relative.display(), error = %err, "skipping unreadable file");
                stats.unreadable += 1;
                continue;
            }
        };
        let parsed = parser.parse(relative, &source)?;
        if parsed.parse_error {
            // The parser already logged the path; the file drops out of the
            // run, and the bucket keeps it distinct from a clean empty file.
            stats.parse_errors += 1;
            continue;
        }
        if parsed.symbols.is_empty() {
            // A valid file with no symbols (`package-info.java`) is normal, so
            // this stays at debug to keep a clean run quiet.
            tracing::debug!(path = %relative.display(), "no symbols parsed; skipping file");
            stats.empty += 1;
            continue;
        }
        stats.files += 1;

        // One `git log -1` per file keys the whole file's cache entries. A file
        // no commit touches has no history to look up, so it is skipped once
        // here rather than failing one `git log -L` per symbol.
        let file_last_commit = if is_repo {
            match history::file_last_commit(repo, relative) {
                Ok(Some(sha)) => Some(sha),
                Ok(None) => {
                    tracing::warn!(
                        path = %relative.display(),
                        "no commit touches this file; indexing it without history"
                    );
                    None
                }
                Err(err) => {
                    tracing::warn!(
                        path = %relative.display(),
                        error = %err,
                        "skipping history for file"
                    );
                    None
                }
            }
        } else {
            None
        };

        let cacheable = !dirty.contains(relative);
        for symbol in &parsed.symbols {
            let content_hash = content_hash(symbol, &source)
                .with_context(|| format!("hashing symbol {}", symbol.fqn))?;
            let Some(symbol_id) =
                write_symbol(&transaction, relative, symbol, &content_hash, &mut ids).await?
            else {
                continue;
            };
            stats.symbols += 1;
            if !is_repo {
                continue;
            }
            let Some(last_commit) = file_last_commit.as_deref() else {
                stats.history_skipped += 1;
                continue;
            };
            let commits = match symbol_history(
                &cache,
                repo,
                relative,
                symbol,
                &content_hash,
                last_commit,
                cacheable,
            )
            .await
            {
                HistoryOutcome::Hit(commits) => {
                    stats.history_hits += 1;
                    commits
                }
                HistoryOutcome::Miss(commits) => {
                    stats.history_misses += 1;
                    commits
                }
                HistoryOutcome::Failed => {
                    stats.history_skipped += 1;
                    continue;
                }
            };
            attach_history(&transaction, symbol_id, &commits, ticket_regex, &mut stats).await?;
        }
    }

    transaction
        .commit()
        .await
        .context("committing index transaction")?;
    build.commit()?;
    tracing::info!(
        files = stats.files,
        symbols = stats.symbols,
        commits = stats.commits,
        tickets = stats.tickets,
        history_hits = stats.history_hits,
        history_misses = stats.history_misses,
        history_skipped = stats.history_skipped,
        empty = stats.empty,
        parse_errors = stats.parse_errors,
        unreadable = stats.unreadable,
        "indexed repository"
    );
    Ok(stats)
}

/// The indexed files with uncommitted changes, logged once per run.
///
/// If the check itself fails, every file is treated as dirty: the run still
/// gets history, it just neither reads nor writes the cache.
fn dirty_files(repo: &Path, files: &[PathBuf]) -> HashSet<PathBuf> {
    let changed = match history::dirty_files(repo) {
        Ok(changed) => changed,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "cannot detect uncommitted changes; history cache disabled for this run"
            );
            return files.iter().cloned().collect();
        }
    };
    let dirty: HashSet<PathBuf> = files
        .iter()
        .filter(|file| changed.contains(*file))
        .cloned()
        .collect();
    if !dirty.is_empty() {
        tracing::warn!(
            files = dirty.len(),
            "uncommitted changes: their history may be mis-attributed and is not cached"
        );
        for file in &dirty {
            tracing::debug!(path = %file.display(), "uncommitted changes");
        }
    }
    dirty
}

/// Where one symbol's history came from.
enum HistoryOutcome {
    /// Reused from the history cache without a git call.
    Hit(Vec<Commit>),
    /// Recomputed with `git log -L`: no row, an outdated row or an unreadable one.
    Miss(Vec<Commit>),
    /// `git log -L` failed; the symbol is indexed without history.
    Failed,
}

/// The commits for one symbol: from the cache when its key still matches,
/// otherwise from `git log -L`, which then refills the cache. A symbol in a
/// dirty file (`cacheable == false`) bypasses the cache both ways.
///
/// The cache is disposable, so a read or decode fault is a miss and a failed
/// write only loses the reuse; both warn and never abort the run. A failed
/// `git log -L` warns and yields [`HistoryOutcome::Failed`].
async fn symbol_history(
    cache: &HistoryCache<'_>,
    repo: &Path,
    file: &Path,
    symbol: &Symbol,
    content_hash: &str,
    last_commit: &str,
    cacheable: bool,
) -> HistoryOutcome {
    if cacheable {
        match cache.get(&symbol.fqn, content_hash, last_commit).await {
            Ok(Some(commits)) => return HistoryOutcome::Hit(commits),
            Ok(None) => {}
            Err(err) => {
                tracing::warn!(
                    fqn = %symbol.fqn,
                    error = format!("{err:#}"),
                    "unreadable history cache row; treating it as a miss"
                );
            }
        }
    }
    let commits =
        match history::history_for_span(repo, file, symbol.history_start_line, symbol.end_line) {
            Ok(commits) => commits,
            Err(err) => {
                tracing::warn!(
                    path = %file.display(),
                    fqn = %symbol.fqn,
                    error = %err,
                    "skipping history for symbol"
                );
                return HistoryOutcome::Failed;
            }
        };
    if !cacheable {
        return HistoryOutcome::Miss(commits);
    }
    if let Err(err) = cache
        .put(&symbol.fqn, content_hash, last_commit, &commits)
        .await
    {
        tracing::warn!(
            fqn = %symbol.fqn,
            error = format!("{err:#}"),
            "could not write history cache row"
        );
    }
    HistoryOutcome::Miss(commits)
}

/// Write one symbol, returning its new row id, or `None` on a duplicate fqn.
///
/// `parent_id` is looked up from `ids`, which is populated as the pre-order
/// walk visits each parent. A duplicate fqn is ignored with a warning: the
/// first definition wins, so a stray copy of a class cannot corrupt the links
/// of the one already indexed.
async fn write_symbol(
    conn: &Connection,
    file: &Path,
    symbol: &Symbol,
    content_hash: &str,
    ids: &mut HashMap<String, i64>,
) -> Result<Option<i64>> {
    let parent_id = symbol.parent.as_ref().and_then(|parent| ids.get(parent));
    let annotations = serde_json::to_string(&symbol.annotations)
        .context("serializing symbol annotations as JSON")?;

    let inserted = conn
        .execute(
            "INSERT OR IGNORE INTO symbols
                (parent_id, kind, role, fqn, file, start_line, end_line, signature, javadoc, annotations, content_hash)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                parent_id.copied(),
                symbol.kind.as_str(),
                symbol.role.as_ref().map(|role| role.as_str()),
                symbol.fqn.as_str(),
                relative_path(file),
                symbol.start_line as i64,
                symbol.end_line as i64,
                symbol.signature.as_str(),
                symbol.javadoc.as_deref(),
                annotations,
                content_hash,
            ],
        )
        .await
        .with_context(|| format!("writing symbol {}", symbol.fqn))?;

    if inserted == 0 {
        tracing::warn!(fqn = %symbol.fqn, "duplicate fqn; keeping the first definition");
        return Ok(None);
    }
    let id = conn.last_insert_rowid();
    ids.insert(symbol.fqn.clone(), id);
    Ok(Some(id))
}

/// Write the commits that touched one symbol and the ticket keys they mention.
///
/// One `symbol_commits` row per commit (subject kept as the fallback for a
/// commit with no ticket) and one `symbol_tickets` row per distinct key drawn
/// from both the subject and the body. A key's dates come from the commit
/// order, not from comparing the ISO strings: [`Commit`]s arrive newest first
/// (`git log` order), so the first commit that mentions a key is its most
/// recent (`last_date`) and the last commit is its oldest (`first_date`), which
/// is robust to the timezone offsets `%aI` carries.
async fn attach_history(
    conn: &Connection,
    symbol_id: i64,
    commits: &[Commit],
    ticket_regex: &Regex,
    stats: &mut IndexStats,
) -> Result<()> {
    for commit in commits {
        let inserted = conn
            .execute(
                "INSERT OR IGNORE INTO symbol_commits (symbol_id, sha, date, subject)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    symbol_id,
                    commit.sha.as_str(),
                    commit.date.as_str(),
                    commit.subject.as_str(),
                ],
            )
            .await
            .with_context(|| format!("writing commit {} for symbol {symbol_id}", commit.sha))?;
        stats.commits += inserted as usize;
    }

    for (key, first_date, last_date) in ticket_span(commits, ticket_regex) {
        let inserted = conn
            .execute(
                "INSERT OR IGNORE INTO symbol_tickets
                    (symbol_id, ticket_key, first_date, last_date)
                 VALUES (?1, ?2, ?3, ?4)",
                params![symbol_id, key.as_str(), first_date, last_date],
            )
            .await
            .with_context(|| format!("writing ticket {key} for symbol {symbol_id}"))?;
        stats.tickets += inserted as usize;
    }
    Ok(())
}

/// The distinct ticket keys mentioned across `commits` (newest first), each
/// with its oldest (`first_date`) and most recent (`last_date`) date.
fn ticket_span(commits: &[Commit], ticket_regex: &Regex) -> Vec<(String, String, String)> {
    let mut order: Vec<String> = Vec::new();
    let mut dates: HashMap<String, (String, String)> = HashMap::new();
    for commit in commits {
        let mut text = commit.subject.clone();
        text.push('\n');
        text.push_str(&commit.body);
        for key in history::ticket_keys(ticket_regex, &text) {
            match dates.get_mut(&key) {
                // Seen before, so this commit is older: move the first date back.
                Some((first, _last)) => *first = commit.date.clone(),
                // First sighting is the newest commit, so it sets both ends.
                None => {
                    order.push(key.clone());
                    dates.insert(key, (commit.date.clone(), commit.date.clone()));
                }
            }
        }
    }
    order
        .into_iter()
        .map(|key| {
            let (first, last) = dates
                .remove(&key)
                .expect("key was recorded on first sighting");
            (key, first, last)
        })
        .collect()
}

/// A content hash over the Javadoc and the symbol's source. Including the
/// Javadoc means a documentation-only edit changes the hash, so later
/// summaries keyed by it are invalidated.
fn content_hash(symbol: &Symbol, source: &str) -> Result<String> {
    let slice = source
        .as_bytes()
        .get(symbol.start_byte..symbol.end_byte)
        .with_context(|| {
            format!(
                "source span {}..{} is out of range for symbol {} (source length {})",
                symbol.start_byte,
                symbol.end_byte,
                symbol.fqn,
                source.len()
            )
        })?;
    let mut hasher = blake3::Hasher::new();
    hasher.update(symbol.javadoc.as_deref().unwrap_or_default().as_bytes());
    hasher.update(b"\n");
    hasher.update(slice);
    Ok(hasher.finalize().to_hex().to_string())
}

/// The repo-relative path with `/` separators, e.g.
/// `src/main/java/com/acme/User.java`.
fn relative_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DEFAULT_TICKET_REGEX;
    use crate::store::IndexReader;
    use crate::test_support::{commit, init_repo};
    use libsql::params;

    fn ticket_regex() -> Regex {
        Regex::new(DEFAULT_TICKET_REGEX).unwrap()
    }

    async fn build(repo: &Path, data_dir: &Path) -> Result<IndexStats> {
        let store = Store::open(data_dir).await.unwrap();
        build_index(&store, repo, None, &ticket_regex()).await
    }

    async fn id_of(conn: &Connection, fqn: &str) -> i64 {
        let mut rows = conn
            .query("SELECT id FROM symbols WHERE fqn = ?1", params![fqn])
            .await
            .unwrap();
        rows.next()
            .await
            .unwrap()
            .unwrap_or_else(|| panic!("no symbol {fqn}"))
            .get::<i64>(0)
            .unwrap()
    }

    async fn text_of(conn: &Connection, fqn: &str, column: &str) -> Option<String> {
        let mut rows = conn
            .query(
                &format!("SELECT {column} FROM symbols WHERE fqn = ?1"),
                params![fqn],
            )
            .await
            .unwrap();
        rows.next()
            .await
            .unwrap()?
            .get::<Option<String>>(0)
            .unwrap()
    }

    async fn parent_of(conn: &Connection, fqn: &str) -> Option<i64> {
        let mut rows = conn
            .query("SELECT parent_id FROM symbols WHERE fqn = ?1", params![fqn])
            .await
            .unwrap();
        rows.next()
            .await
            .unwrap()
            .unwrap()
            .get::<Option<i64>>(0)
            .unwrap()
    }

    async fn count(conn: &Connection) -> i64 {
        let mut rows = conn
            .query("SELECT COUNT(*) FROM symbols", ())
            .await
            .unwrap();
        rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap()
    }

    fn write(repo: &Path, relative: &str, source: &str) {
        let path = repo.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, source).unwrap();
    }

    const SAMPLE: &str = r#"
package com.acme.sample;

import java.util.List;

/** Handles users. */
@Service
public class UserService {
    /** Finds a user. */
    public User find(Long id) { return null; }

    public User find(String name) { return null; }

    public class Nested {
        public void ping() {}
    }
}
"#;

    fn symbol_with_span(fqn: &str, start_byte: usize, end_byte: usize) -> Symbol {
        Symbol {
            package: String::new(),
            name: "Broken".to_string(),
            fqn: fqn.to_string(),
            kind: crate::symbols::SymbolKind::Class,
            start_line: 1,
            end_line: 1,
            history_start_line: 1,
            start_byte,
            end_byte,
            parent: None,
            signature: "class Broken".to_string(),
            javadoc: None,
            annotations: Vec::new(),
            role: None,
        }
    }

    #[test]
    fn content_hash_errors_on_out_of_range_span() {
        let symbol = symbol_with_span("com.acme.Broken", 0, 999);
        let err = content_hash(&symbol, "class Broken {}")
            .expect_err("a span past the end of the source should fail, not panic");
        let message = format!("{err:#}");
        assert!(
            message.contains("com.acme.Broken"),
            "error should name the fqn, got: {message}"
        );
        assert!(
            message.contains("0..999"),
            "error should name the span, got: {message}"
        );
    }

    #[tokio::test]
    async fn indexes_symbols_with_parent_links_and_content_hashes() {
        let repo = tempfile::tempdir().unwrap();
        write(
            repo.path(),
            "src/main/java/com/acme/sample/UserService.java",
            SAMPLE,
        );
        let data = tempfile::tempdir().unwrap();

        let stats = build(repo.path(), data.path()).await.unwrap();
        assert_eq!(stats.files, 1);
        assert_eq!(stats.empty, 0);
        assert_eq!(stats.parse_errors, 0);
        assert_eq!(stats.unreadable, 0);
        assert_eq!(stats.symbols, 5, "class, two methods, nested type, ping");

        let reader = IndexReader::open(data.path()).await.unwrap();
        let conn = reader.connection();
        assert_eq!(count(conn).await, 5);

        let class_id = id_of(conn, "com.acme.sample.UserService").await;
        let nested_id = id_of(conn, "com.acme.sample.UserService.Nested").await;

        assert_eq!(
            parent_of(conn, "com.acme.sample.UserService").await,
            None,
            "a top-level type has no parent"
        );
        assert_eq!(
            parent_of(conn, "com.acme.sample.UserService#find(Long)").await,
            Some(class_id),
            "members link to their type"
        );
        assert_eq!(
            parent_of(conn, "com.acme.sample.UserService.Nested").await,
            Some(class_id),
            "nested types link to their enclosing type"
        );
        assert_eq!(
            parent_of(conn, "com.acme.sample.UserService.Nested#ping()").await,
            Some(nested_id),
            "members of a nested type link to that nested type"
        );

        assert_eq!(
            text_of(conn, "com.acme.sample.UserService", "kind")
                .await
                .as_deref(),
            Some("class")
        );
        assert_eq!(
            text_of(conn, "com.acme.sample.UserService", "role")
                .await
                .as_deref(),
            Some("service")
        );
        assert_eq!(
            text_of(conn, "com.acme.sample.UserService#find(Long)", "kind")
                .await
                .as_deref(),
            Some("method")
        );
        assert_eq!(
            text_of(conn, "com.acme.sample.UserService#find(Long)", "file")
                .await
                .as_deref(),
            Some("src/main/java/com/acme/sample/UserService.java")
        );
        assert_eq!(
            text_of(conn, "com.acme.sample.UserService", "annotations")
                .await
                .as_deref(),
            Some(r#"["@Service"]"#)
        );
        assert_eq!(
            text_of(conn, "com.acme.sample.UserService", "javadoc")
                .await
                .as_deref(),
            Some("Handles users.")
        );

        let hashes: Vec<String> = {
            let mut rows = conn
                .query("SELECT content_hash FROM symbols", ())
                .await
                .unwrap();
            let mut out = Vec::new();
            while let Some(row) = rows.next().await.unwrap() {
                out.push(row.get::<String>(0).unwrap());
            }
            out
        };
        assert_eq!(hashes.len(), 5);
        assert!(hashes.iter().all(|hash| !hash.is_empty()));
        let unique: std::collections::HashSet<&String> = hashes.iter().collect();
        assert_eq!(
            unique.len(),
            hashes.len(),
            "content hashes should be unique"
        );
    }

    #[tokio::test]
    async fn duplicate_fqn_keeps_the_first_definition() {
        let repo = tempfile::tempdir().unwrap();
        write(
            repo.path(),
            "src/main/java/com/acme/Dup.java",
            "package com.acme;\nclass Dup {}\n",
        );
        write(
            repo.path(),
            "src/main/java/com/acme/Other.java",
            "package com.acme;\nclass Dup {}\n",
        );
        let data = tempfile::tempdir().unwrap();

        let stats = build(repo.path(), data.path()).await.unwrap();
        assert_eq!(stats.symbols, 1, "the duplicate is not counted");

        let reader = IndexReader::open(data.path()).await.unwrap();
        let conn = reader.connection();
        assert_eq!(count(conn).await, 1);
    }

    #[tokio::test]
    async fn empty_repo_yields_zero_symbols_without_error() {
        let repo = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();

        let stats = build(repo.path(), data.path()).await.unwrap();
        assert_eq!(stats, IndexStats::default());

        let reader = IndexReader::open(data.path()).await.unwrap();
        let conn = reader.connection();
        assert_eq!(count(conn).await, 0);
    }

    #[tokio::test]
    async fn missing_repo_is_an_error_not_an_empty_index() {
        let data = tempfile::tempdir().unwrap();
        let store = Store::open(data.path()).await.unwrap();

        let err = build_index(&store, Path::new("/does/not/exist"), None, &ticket_regex())
            .await
            .expect_err("a missing repo path should fail");

        assert!(
            format!("{err:#}").contains("not a directory"),
            "error should name the bad repo path, got: {err:#}"
        );
    }

    #[tokio::test]
    async fn broken_file_is_skipped_and_the_rest_is_indexed() {
        let repo = tempfile::tempdir().unwrap();
        write(
            repo.path(),
            "src/main/java/com/acme/Good.java",
            "package com.acme;\nclass Good {}\n",
        );
        write(
            repo.path(),
            "src/main/java/com/acme/Broken.java",
            "package com.acme;\nclass Broken {\n",
        );
        let data = tempfile::tempdir().unwrap();

        let stats = build(repo.path(), data.path()).await.unwrap();
        assert_eq!(stats.files, 1);
        assert_eq!(stats.parse_errors, 1);
        assert_eq!(stats.empty, 0);
        assert_eq!(stats.unreadable, 0);
        assert_eq!(stats.symbols, 1);

        let reader = IndexReader::open(data.path()).await.unwrap();
        let conn = reader.connection();
        assert_eq!(count(conn).await, 1);
    }

    #[tokio::test]
    async fn good_empty_and_broken_files_land_in_distinct_buckets() {
        let repo = tempfile::tempdir().unwrap();
        write(
            repo.path(),
            "src/main/java/com/acme/Good.java",
            "package com.acme;\nclass Good {}\n",
        );
        write(
            repo.path(),
            "src/main/java/com/acme/package-info.java",
            "package com.acme;\n",
        );
        write(
            repo.path(),
            "src/main/java/com/acme/Broken.java",
            "package com.acme;\nclass Broken {\n",
        );
        let data = tempfile::tempdir().unwrap();

        let stats = build(repo.path(), data.path()).await.unwrap();
        assert_eq!(stats.files, 1, "only Good.java produced a symbol");
        assert_eq!(
            stats.empty, 1,
            "package-info.java parsed cleanly, no symbols"
        );
        assert_eq!(stats.parse_errors, 1, "Broken.java did not parse");
        assert_eq!(stats.unreadable, 0);
        assert_eq!(stats.symbols, 1);

        let reader = IndexReader::open(data.path()).await.unwrap();
        let conn = reader.connection();
        assert_eq!(count(conn).await, 1);
    }

    /// A Java file whose method body changes with `value`, so each rewrite
    /// touches the same lines and `git log -L` sees every commit.
    fn java(value: i64) -> String {
        java_class("Service", value)
    }

    /// The same shape as [`java`] under a chosen class name, so a test can
    /// prove one file's change does not invalidate another file's cache.
    fn java_class(name: &str, value: i64) -> String {
        format!(
            "package com.acme;\n\npublic class {name} {{\n    public int value() {{\n        return {value};\n    }}\n}}\n"
        )
    }

    async fn index_git_repo(repo: &Path, data: &Path) -> IndexStats {
        let store = Store::open(data).await.unwrap();
        build_index(&store, repo, None, &ticket_regex())
            .await
            .unwrap()
    }

    async fn row_count(conn: &Connection, table: &str, symbol_id: i64) -> i64 {
        let mut rows = conn
            .query(
                &format!("SELECT COUNT(*) FROM {table} WHERE symbol_id = ?1"),
                params![symbol_id],
            )
            .await
            .unwrap();
        rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap()
    }

    async fn ticket_dates(conn: &Connection, symbol_id: i64, key: &str) -> (String, String) {
        let mut rows = conn
            .query(
                "SELECT first_date, last_date FROM symbol_tickets
                 WHERE symbol_id = ?1 AND ticket_key = ?2",
                params![symbol_id, key],
            )
            .await
            .unwrap();
        let row = rows
            .next()
            .await
            .unwrap()
            .unwrap_or_else(|| panic!("no ticket {key} for symbol {symbol_id}"));
        (row.get::<String>(0).unwrap(), row.get::<String>(1).unwrap())
    }

    #[tokio::test]
    async fn git_history_stores_multiple_ticket_keys_with_their_span() {
        let repo = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        write(repo.path(), "src/main/java/com/acme/Service.java", &java(1));
        commit(
            repo.path(),
            "GRLD-1 add service",
            "2024-01-01T00:00:00+01:00",
        );
        write(repo.path(), "src/main/java/com/acme/Service.java", &java(2));
        commit(
            repo.path(),
            "GRLD-2 refine service\n\nstill GRLD-1",
            "2024-02-02T00:00:00+02:00",
        );
        let data = tempfile::tempdir().unwrap();

        let stats = index_git_repo(repo.path(), data.path()).await;
        assert!(stats.commits >= 2, "both commits are recorded");
        assert_eq!(
            stats.tickets, 4,
            "the class and the method each get both keys"
        );

        let reader = IndexReader::open(data.path()).await.unwrap();
        let conn = reader.connection();
        let class_id = id_of(conn, "com.acme.Service").await;
        assert_eq!(row_count(conn, "symbol_commits", class_id).await, 2);
        assert_eq!(
            ticket_dates(conn, class_id, "GRLD-1").await,
            (
                "2024-01-01T00:00:00+01:00".to_string(),
                "2024-02-02T00:00:00+02:00".to_string()
            ),
            "GRLD-1 spans from its first to its last mention"
        );
        assert_eq!(
            ticket_dates(conn, class_id, "GRLD-2").await,
            (
                "2024-02-02T00:00:00+02:00".to_string(),
                "2024-02-02T00:00:00+02:00".to_string()
            ),
            "a key mentioned once has equal first and last dates"
        );
    }

    #[tokio::test]
    async fn duplicate_key_across_commits_yields_one_row() {
        let repo = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        write(repo.path(), "src/main/java/com/acme/Service.java", &java(1));
        commit(repo.path(), "GRLD-7 add", "2024-01-01T00:00:00+01:00");
        write(repo.path(), "src/main/java/com/acme/Service.java", &java(2));
        commit(repo.path(), "GRLD-7 fix", "2024-03-03T00:00:00-05:00");
        let data = tempfile::tempdir().unwrap();

        index_git_repo(repo.path(), data.path()).await;

        let reader = IndexReader::open(data.path()).await.unwrap();
        let conn = reader.connection();
        let class_id = id_of(conn, "com.acme.Service").await;
        assert_eq!(
            row_count(conn, "symbol_tickets", class_id).await,
            1,
            "the repeated key is stored once"
        );
        assert_eq!(
            ticket_dates(conn, class_id, "GRLD-7").await,
            (
                "2024-01-01T00:00:00+01:00".to_string(),
                "2024-03-03T00:00:00-05:00".to_string()
            ),
            "first date is the older commit, last date the newer"
        );
    }

    #[tokio::test]
    async fn commit_without_a_ticket_keeps_the_commit_but_no_ticket_row() {
        let repo = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        write(repo.path(), "src/main/java/com/acme/Service.java", &java(1));
        commit(
            repo.path(),
            "no ticket in this message",
            "2024-01-01T00:00:00+00:00",
        );
        let data = tempfile::tempdir().unwrap();

        index_git_repo(repo.path(), data.path()).await;

        let reader = IndexReader::open(data.path()).await.unwrap();
        let conn = reader.connection();
        let class_id = id_of(conn, "com.acme.Service").await;
        assert_eq!(
            row_count(conn, "symbol_commits", class_id).await,
            1,
            "the commit is still stored as the fallback"
        );
        assert_eq!(
            row_count(conn, "symbol_tickets", class_id).await,
            0,
            "no key means no ticket row"
        );
    }

    async fn cache_count(data: &Path, table: &str) -> i64 {
        let store = Store::open(data).await.unwrap();
        let mut rows = store
            .cache()
            .query(&format!("SELECT COUNT(*) FROM {table}"), ())
            .await
            .unwrap();
        rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap()
    }

    async fn commit_rows(data: &Path) -> Vec<(String, String, String, String)> {
        let reader = IndexReader::open(data).await.unwrap();
        let mut rows = reader
            .connection()
            .query(
                "SELECT s.fqn, c.sha, c.date, c.subject
                 FROM symbol_commits c JOIN symbols s ON s.id = c.symbol_id
                 ORDER BY s.fqn, c.sha",
                (),
            )
            .await
            .unwrap();
        let mut out = Vec::new();
        while let Some(row) = rows.next().await.unwrap() {
            out.push((
                row.get::<String>(0).unwrap(),
                row.get::<String>(1).unwrap(),
                row.get::<String>(2).unwrap(),
                row.get::<String>(3).unwrap(),
            ));
        }
        out
    }

    async fn ticket_rows(data: &Path) -> Vec<(String, String, String, String)> {
        let reader = IndexReader::open(data).await.unwrap();
        let mut rows = reader
            .connection()
            .query(
                "SELECT s.fqn, t.ticket_key, t.first_date, t.last_date
                 FROM symbol_tickets t JOIN symbols s ON s.id = t.symbol_id
                 ORDER BY s.fqn, t.ticket_key",
                (),
            )
            .await
            .unwrap();
        let mut out = Vec::new();
        while let Some(row) = rows.next().await.unwrap() {
            out.push((
                row.get::<String>(0).unwrap(),
                row.get::<String>(1).unwrap(),
                row.get::<String>(2).unwrap(),
                row.get::<String>(3).unwrap(),
            ));
        }
        out
    }

    #[tokio::test]
    async fn non_git_repo_indexes_structure_without_history() {
        let repo = tempfile::tempdir().unwrap();
        write(repo.path(), "src/main/java/com/acme/Service.java", &java(1));
        let data = tempfile::tempdir().unwrap();

        let stats = build(repo.path(), data.path()).await.unwrap();

        assert_eq!(
            stats.symbols, 2,
            "the class and its method are still indexed"
        );
        assert_eq!(stats.commits, 0);
        assert_eq!(stats.tickets, 0);
        assert_eq!(
            stats.history_hits, 0,
            "a non-git run never consults the cache"
        );
        assert_eq!(stats.history_misses, 0);

        let reader = IndexReader::open(data.path()).await.unwrap();
        let conn = reader.connection();
        assert_eq!(
            row_count(
                conn,
                "symbol_commits",
                id_of(conn, "com.acme.Service").await
            )
            .await,
            0
        );
        assert_eq!(
            cache_count(data.path(), "history_cache_v2").await,
            0,
            "the history cache stays untouched"
        );
    }

    #[tokio::test]
    async fn second_index_hits_the_history_cache_with_identical_rows() {
        let repo = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        write(repo.path(), "src/main/java/com/acme/Service.java", &java(1));
        commit(
            repo.path(),
            "GRLD-1 add service",
            "2024-01-01T00:00:00+01:00",
        );
        let data = tempfile::tempdir().unwrap();

        let cold = index_git_repo(repo.path(), data.path()).await;
        assert_eq!(cold.history_hits, 0, "a fresh cache cannot hit");
        assert_eq!(cold.history_misses, 2, "both symbols are looked up");

        let commits_before = commit_rows(data.path()).await;
        let tickets_before = ticket_rows(data.path()).await;

        let warm = index_git_repo(repo.path(), data.path()).await;
        assert_eq!(
            warm.history_hits, 2,
            "every symbol is served from the cache"
        );
        assert_eq!(warm.history_misses, 0);

        assert_eq!(
            commit_rows(data.path()).await,
            commits_before,
            "a cached run stores the same commit rows"
        );
        assert_eq!(
            ticket_rows(data.path()).await,
            tickets_before,
            "a cached run stores the same ticket rows"
        );
    }

    #[tokio::test]
    async fn untracked_file_is_skipped_once_and_counted() {
        let repo = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        write(
            repo.path(),
            "src/main/java/com/acme/Service.java",
            &java_class("Service", 1),
        );
        commit(
            repo.path(),
            "GRLD-1 add service",
            "2024-01-01T00:00:00+01:00",
        );
        write(
            repo.path(),
            "src/main/java/com/acme/Draft.java",
            &java_class("Draft", 1),
        );
        let data = tempfile::tempdir().unwrap();

        let stats = index_git_repo(repo.path(), data.path()).await;

        assert_eq!(stats.symbols, 4, "the untracked file is still indexed");
        assert_eq!(
            stats.history_misses, 2,
            "only the committed file is looked up"
        );
        assert_eq!(stats.history_skipped, 2, "the untracked file's symbols");
        assert_eq!(
            stats.history_hits + stats.history_misses + stats.history_skipped,
            stats.symbols,
            "every symbol lands in exactly one history bucket"
        );
        let reader = IndexReader::open(data.path()).await.unwrap();
        let conn = reader.connection();
        assert_eq!(
            row_count(conn, "symbol_commits", id_of(conn, "com.acme.Draft").await).await,
            0
        );
        assert_eq!(
            row_count(
                conn,
                "symbol_commits",
                id_of(conn, "com.acme.Service").await
            )
            .await,
            1
        );
        assert_eq!(
            cache_count(data.path(), "history_cache_v2").await,
            2,
            "an untracked file is never cached"
        );
    }

    #[tokio::test]
    async fn dirty_file_gets_history_but_is_never_cached() {
        let repo = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        write(
            repo.path(),
            "src/main/java/com/acme/Service.java",
            &java_class("Service", 1),
        );
        write(
            repo.path(),
            "src/main/java/com/acme/Clean.java",
            &java_class("Clean", 1),
        );
        commit(
            repo.path(),
            "GRLD-1 add services",
            "2024-01-01T00:00:00+01:00",
        );
        // An uncommitted edit that shifts every line of `Service` down.
        write(
            repo.path(),
            "src/main/java/com/acme/Service.java",
            &format!("// moved\n// down\n{}", java_class("Service", 1)),
        );
        let data = tempfile::tempdir().unwrap();

        let dirty = index_git_repo(repo.path(), data.path()).await;
        assert_eq!(dirty.history_misses, 4, "both files still get history");
        assert_eq!(
            cache_count(data.path(), "history_cache_v2").await,
            2,
            "only the clean file is cached"
        );
        let again = index_git_repo(repo.path(), data.path()).await;
        assert_eq!(
            (again.history_hits, again.history_misses),
            (2, 2),
            "the dirty file never reads the cache either"
        );

        commit(
            repo.path(),
            "GRLD-2 move service",
            "2024-01-02T00:00:00+01:00",
        );
        index_git_repo(repo.path(), data.path()).await;
        assert_eq!(
            cache_count(data.path(), "history_cache_v2").await,
            4,
            "once committed, the file is cached"
        );
    }

    #[tokio::test]
    async fn javadoc_only_commit_is_part_of_the_member_history() {
        let source = |doc: &str| {
            format!(
                "package com.acme;\n\npublic class Service {{\n    /** {doc} */\n    public int value() {{\n        return 1;\n    }}\n}}\n"
            )
        };
        let repo = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        write(
            repo.path(),
            "src/main/java/com/acme/Service.java",
            &source("Returns one."),
        );
        commit(repo.path(), "GRLD-1 add value", "2024-01-01T00:00:00+01:00");
        write(
            repo.path(),
            "src/main/java/com/acme/Service.java",
            &source("Returns the constant one."),
        );
        commit(
            repo.path(),
            "GRLD-9 document value",
            "2024-01-02T00:00:00+01:00",
        );
        let data = tempfile::tempdir().unwrap();

        index_git_repo(repo.path(), data.path()).await;

        let reader = IndexReader::open(data.path()).await.unwrap();
        let conn = reader.connection();
        let method = id_of(conn, "com.acme.Service#value()").await;
        assert_eq!(row_count(conn, "symbol_commits", method).await, 2);
        assert_eq!(
            ticket_dates(conn, method, "GRLD-9").await,
            (
                "2024-01-02T00:00:00+01:00".to_string(),
                "2024-01-02T00:00:00+01:00".to_string()
            ),
            "the doc-only commit's ticket is attributed to the method"
        );
    }

    #[tokio::test]
    async fn legacy_history_cache_table_is_dropped() {
        let data = tempfile::tempdir().unwrap();
        {
            let store = Store::open(data.path()).await.unwrap();
            store
                .cache()
                .execute("CREATE TABLE history_cache (fqn TEXT PRIMARY KEY)", ())
                .await
                .unwrap();
        }
        let store = Store::open(data.path()).await.unwrap();
        let mut rows = store
            .cache()
            .query(
                "SELECT COUNT(*) FROM sqlite_master WHERE name = 'history_cache'",
                (),
            )
            .await
            .unwrap();
        let count = rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap();
        assert_eq!(count, 0, "rows computed from the old span are discarded");
    }

    #[tokio::test]
    async fn corrupt_cache_row_is_a_miss_and_is_rewritten() {
        let repo = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        write(repo.path(), "src/main/java/com/acme/Service.java", &java(1));
        commit(
            repo.path(),
            "GRLD-1 add service",
            "2024-01-01T00:00:00+01:00",
        );
        let data = tempfile::tempdir().unwrap();
        index_git_repo(repo.path(), data.path()).await;
        let commits_before = commit_rows(data.path()).await;
        {
            let store = Store::open(data.path()).await.unwrap();
            store
                .cache()
                .execute("UPDATE history_cache_v2 SET commits = '{'", ())
                .await
                .unwrap();
        }

        let corrupt = index_git_repo(repo.path(), data.path()).await;
        assert_eq!(
            (corrupt.history_hits, corrupt.history_misses),
            (0, 2),
            "an undecodable row is a miss, not an error"
        );
        assert_eq!(
            commit_rows(data.path()).await,
            commits_before,
            "history is recomputed and attached"
        );

        let healed = index_git_repo(repo.path(), data.path()).await;
        assert_eq!(
            (healed.history_hits, healed.history_misses),
            (2, 0),
            "the miss rewrote the row with valid JSON"
        );
    }

    #[tokio::test]
    async fn content_change_invalidates_only_the_changed_file() {
        let repo = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        write(
            repo.path(),
            "src/main/java/com/acme/A.java",
            &java_class("ServiceA", 1),
        );
        write(
            repo.path(),
            "src/main/java/com/acme/B.java",
            &java_class("ServiceB", 1),
        );
        commit(repo.path(), "GRLD-1 add both", "2024-01-01T00:00:00+00:00");
        let data = tempfile::tempdir().unwrap();

        index_git_repo(repo.path(), data.path()).await;

        write(
            repo.path(),
            "src/main/java/com/acme/B.java",
            &java_class("ServiceB", 2),
        );
        commit(repo.path(), "GRLD-2 change B", "2024-02-02T00:00:00+00:00");

        let warm = index_git_repo(repo.path(), data.path()).await;
        assert_eq!(warm.history_hits, 2, "A's class and method are unchanged");
        assert_eq!(warm.history_misses, 2, "B's class and method changed");
    }

    #[tokio::test]
    async fn new_commit_invalidates_by_file_last_commit_even_when_content_reverts() {
        let repo = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        write(
            repo.path(),
            "src/main/java/com/acme/A.java",
            &java_class("ServiceA", 1),
        );
        write(
            repo.path(),
            "src/main/java/com/acme/B.java",
            &java_class("ServiceB", 1),
        );
        commit(repo.path(), "GRLD-1 add both", "2024-01-01T00:00:00+00:00");
        let data = tempfile::tempdir().unwrap();

        index_git_repo(repo.path(), data.path()).await;

        write(
            repo.path(),
            "src/main/java/com/acme/B.java",
            &java_class("ServiceB", 2),
        );
        commit(repo.path(), "GRLD-2 change B", "2024-02-02T00:00:00+00:00");
        write(
            repo.path(),
            "src/main/java/com/acme/B.java",
            &java_class("ServiceB", 1),
        );
        commit(repo.path(), "GRLD-3 revert B", "2024-03-03T00:00:00+00:00");

        let warm = index_git_repo(repo.path(), data.path()).await;
        assert_eq!(warm.history_hits, 2, "A is untouched");
        assert_eq!(
            warm.history_misses, 2,
            "B's content hash matches but its last commit moved, so it misses"
        );
    }

    #[tokio::test]
    async fn history_cache_lives_in_cache_db_and_survives_a_rebuild() {
        let repo = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        write(repo.path(), "src/main/java/com/acme/Service.java", &java(1));
        commit(
            repo.path(),
            "GRLD-1 add service",
            "2024-01-01T00:00:00+00:00",
        );
        let data = tempfile::tempdir().unwrap();

        index_git_repo(repo.path(), data.path()).await;
        assert_eq!(
            cache_count(data.path(), "history_cache_v2").await,
            2,
            "both symbols are cached in cache.db"
        );

        std::fs::remove_file(data.path().join(crate::store::INDEX_DB)).unwrap();
        index_git_repo(repo.path(), data.path()).await;
        assert_eq!(
            cache_count(data.path(), "history_cache_v2").await,
            2,
            "the cache is not rebuilt from scratch"
        );
    }
}
