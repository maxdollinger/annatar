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

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use libsql::{Connection, params};
use regex::Regex;

use crate::history::{self, Commit};
use crate::store::Store;
use crate::symbols::{JavaParser, Symbol};
use crate::walk;

/// A summary of one index run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexStats {
    /// Files that produced at least one symbol.
    pub files: usize,
    /// Symbol rows written to `symbols`.
    pub symbols: usize,
    /// Commit rows written to `symbol_commits`.
    pub commits: usize,
    /// Ticket-key rows written to `symbol_tickets`.
    pub tickets: usize,
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
/// When `repo` is not a git work tree the run degrades to a structure-only
/// index: one warning, no history tables, no error. When it is a repo, a
/// per-symbol history lookup that fails (an untracked file, say) warns with the
/// file and is skipped, so one bad file cannot abort the whole index.
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
    let build = store.begin_index().await?;
    let mut stats = IndexStats {
        files: 0,
        symbols: 0,
        commits: 0,
        tickets: 0,
        empty: 0,
        parse_errors: 0,
        unreadable: 0,
    };
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
        for symbol in &parsed.symbols {
            let Some(symbol_id) =
                write_symbol(&transaction, relative, &source, symbol, &mut ids).await?
            else {
                continue;
            };
            stats.symbols += 1;
            if !is_repo {
                continue;
            }
            match history::history_for_span(repo, relative, symbol.start_line, symbol.end_line) {
                Ok(commits) => {
                    attach_history(&transaction, symbol_id, &commits, ticket_regex, &mut stats)
                        .await?;
                }
                Err(err) => {
                    tracing::warn!(
                        path = %relative.display(),
                        error = %err,
                        "skipping history for symbol"
                    );
                }
            }
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
        empty = stats.empty,
        parse_errors = stats.parse_errors,
        unreadable = stats.unreadable,
        "indexed repository"
    );
    Ok(stats)
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
    source: &str,
    symbol: &Symbol,
    ids: &mut HashMap<String, i64>,
) -> Result<Option<i64>> {
    let parent_id = symbol.parent.as_ref().and_then(|parent| ids.get(parent));
    let annotations = serde_json::to_string(&symbol.annotations)
        .context("serializing symbol annotations as JSON")?;
    let content_hash =
        content_hash(symbol, source).with_context(|| format!("hashing symbol {}", symbol.fqn))?;

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
    use libsql::params;
    use std::process::Command;

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
        assert_eq!(
            stats,
            IndexStats {
                files: 0,
                symbols: 0,
                commits: 0,
                tickets: 0,
                empty: 0,
                parse_errors: 0,
                unreadable: 0,
            }
        );

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

    fn git(repo: &Path) -> Command {
        let mut command = Command::new("git");
        command
            .current_dir(repo)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_AUTHOR_NAME", "Annatar Test")
            .env("GIT_AUTHOR_EMAIL", "annatar@test.invalid")
            .env("GIT_COMMITTER_NAME", "Annatar Test")
            .env("GIT_COMMITTER_EMAIL", "annatar@test.invalid");
        command
    }

    fn git_ok(repo: &Path, args: &[&str]) {
        let output = git(repo).args(args).output().expect("git should run");
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn init_repo(repo: &Path) {
        git_ok(repo, &["init", "-q"]);
        git_ok(repo, &["config", "user.email", "annatar@test.invalid"]);
        git_ok(repo, &["config", "user.name", "Annatar Test"]);
        git_ok(repo, &["config", "commit.gpgsign", "false"]);
    }

    fn commit(repo: &Path, message: &str, date: &str) {
        git_ok(repo, &["add", "-A"]);
        let output = git(repo)
            .args(["commit", "-q", "-m", message])
            .env("GIT_AUTHOR_DATE", date)
            .env("GIT_COMMITTER_DATE", date)
            .output()
            .expect("git should run");
        assert!(
            output.status.success(),
            "git commit failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// A Java file whose method body changes with `value`, so each rewrite
    /// touches the same lines and `git log -L` sees every commit.
    fn java(value: i64) -> String {
        format!(
            "package com.acme;\n\npublic class Service {{\n    public int value() {{\n        return {value};\n    }}\n}}\n"
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
    }
}
