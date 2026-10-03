//! Build `index.db` from a repository's Java sources.
//!
//! [`build_index`] is the one orchestrator of an index run. It owns the
//! [`crate::store::IndexBuild`] and the single write transaction on it from
//! start to finish, and runs the stages in order on that transaction:
//!
//! 1. **structure** (`index_structure`) walks the production files, parses
//!    each one with a single reusable [`crate::symbols::JavaParser`] and writes
//!    every symbol to `symbols`;
//! 2. **history** (`index_history`) attaches each written symbol's commits
//!    and ticket keys (`symbol_commits`, `symbol_tickets`); skipped when the
//!    repository is not a git work tree;
//! 3. **tickets** (`index_tickets`) makes sure every distinct key in
//!    `symbol_tickets` is in the `cache.db` ticket cache, fetching only the
//!    missing ones from Jira, and copies the cached fields into the index
//!    `tickets` table; runs after history, on git work trees only.
//!
//! Later stages (summaries) slot in after tickets the same way. A
//! stage never commits: the transaction commits and the temporary file is
//! atomically renamed over `index.db` exactly once, after the last stage. Any
//! stage error drops the transaction and the build, leaving the previous index
//! untouched.
//!
//! Symbols are keyed by their fully qualified name, which is unique and stable
//! across runs. The parser returns symbols in pre-order, so a symbol is always
//! written before its members and nested types; an in-memory map from fqn to row
//! id supplies each child's `parent_id`. A file that cannot be read or parsed is
//! counted in its own bucket, never fatal.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Context, Result};
use libsql::{Connection, Transaction, params};
use regex::Regex;

use crate::history::{self, Commit, HistoryCache};
use crate::jira::{FetchError, Ticket};
use crate::store::Store;
use crate::symbols::{JavaParser, Symbol};
use crate::tickets::{CachedTicket, Fetched, TicketCache, TicketFetch, fetch_with_retry};
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
    /// Distinct ticket keys in `symbol_tickets`. When the ticket stage runs,
    /// `ticket_hits + tickets_fetched + tickets_unavailable + tickets_failed +
    /// tickets_not_fetched == ticket_keys`.
    pub ticket_keys: usize,
    /// Keys served from the ticket cache (content or unavailable), no Jira call.
    pub ticket_hits: usize,
    /// Keys fetched from Jira this run and cached with content.
    pub tickets_fetched: usize,
    /// Keys Jira answered 403/404 for this run, cached as unavailable.
    pub tickets_unavailable: usize,
    /// Keys whose fetch failed this run (rate limit after retries, transport,
    /// other status, unparseable response); not cached, retried next run.
    pub tickets_failed: usize,
    /// Keys missing from the cache that were not fetched because Jira is not
    /// configured, has no token, the run is `--offline`, or the circuit
    /// breaker stopped fetching after a transport failure or exhausted 429.
    pub tickets_not_fetched: usize,
    /// Jira requests sent, retries and fetches aborted by the circuit breaker
    /// included. Zero on a fully cached run.
    pub jira_requests: usize,
    /// Files that parsed cleanly but produced no symbols (`package-info.java`).
    pub empty: usize,
    /// Files tree-sitter could not parse, or for which it produced no tree.
    pub parse_errors: usize,
    /// Files whose contents could not be read.
    pub unreadable: usize,
}

/// Index every production Java file under `repo` into a fresh `index.db`.
///
/// `path_prefix`, when given, limits the run to a part of the repository. The
/// new index then holds only that part and still replaces the whole
/// `index.db` (one warning says so); `--path` is for fast prompt iteration, not
/// for refreshing a slice of a full index. `ticket_regex` extracts Jira keys
/// from commit subjects and bodies; it is compiled by the caller so a bad
/// pattern fails before the run starts.
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
///
/// `jira` is how missing tickets are fetched; `None` (offline, or Jira not
/// configured) still copies already-cached tickets into the index but makes no
/// request. Bad Jira credentials abort the run; see `index_tickets` for the
/// other outcomes.
pub async fn build_index(
    store: &Store,
    repo: &Path,
    path_prefix: Option<&Path>,
    ticket_regex: &Regex,
    jira: Option<&TicketFetch>,
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
    if let Some(prefix) = path_prefix {
        tracing::warn!(
            prefix = %prefix.display(),
            files = files.len(),
            index = %store.index_path().display(),
            "--path run: the index will be replaced by one holding only this prefix"
        );
    }

    let build = store.begin_index().await?;
    let transaction = build
        .connection()
        .transaction()
        .await
        .context("starting index transaction")?;
    let mut stats = IndexStats::default();

    let indexed = index_structure(&transaction, repo, &files, &mut stats).await?;
    if is_repo {
        index_history(
            &transaction,
            store.cache(),
            repo,
            &indexed,
            ticket_regex,
            &mut stats,
        )
        .await?;
        index_tickets(&transaction, store.cache(), jira, &mut stats).await?;
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
        ticket_keys = stats.ticket_keys,
        ticket_hits = stats.ticket_hits,
        tickets_fetched = stats.tickets_fetched,
        tickets_unavailable = stats.tickets_unavailable,
        tickets_failed = stats.tickets_failed,
        tickets_not_fetched = stats.tickets_not_fetched,
        jira_requests = stats.jira_requests,
        empty = stats.empty,
        parse_errors = stats.parse_errors,
        unreadable = stats.unreadable,
        "indexed repository"
    );
    Ok(stats)
}

/// One file the structure stage parsed, with the rows it wrote (none if every
/// fqn was a duplicate).
struct IndexedFile {
    path: PathBuf,
    symbols: Vec<IndexedSymbol>,
}

/// One symbol row the structure stage wrote, with what later stages key on.
struct IndexedSymbol {
    id: i64,
    symbol: Symbol,
    content_hash: String,
}

/// Structure stage: parse `files` and write every symbol to `symbols`.
///
/// Returns each file that produced symbols, with the rows it wrote (a
/// duplicate fqn writes no row and is left out), in walk order. Fills
/// `files`, `symbols`, `empty`, `parse_errors` and `unreadable`.
async fn index_structure(
    transaction: &Transaction,
    repo: &Path,
    files: &[PathBuf],
    stats: &mut IndexStats,
) -> Result<Vec<IndexedFile>> {
    let mut parser = JavaParser::new()?;
    let mut ids: HashMap<String, i64> = HashMap::new();
    let mut indexed = Vec::new();

    for relative in files {
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

        let mut symbols = Vec::with_capacity(parsed.symbols.len());
        for symbol in parsed.symbols {
            let content_hash = content_hash(&symbol, &source)
                .with_context(|| format!("hashing symbol {}", symbol.fqn))?;
            let Some(id) =
                write_symbol(transaction, relative, &symbol, &content_hash, &mut ids).await?
            else {
                continue;
            };
            stats.symbols += 1;
            symbols.push(IndexedSymbol {
                id,
                symbol,
                content_hash,
            });
        }
        indexed.push(IndexedFile {
            path: relative.clone(),
            symbols,
        });
    }
    Ok(indexed)
}

/// History stage: attach commits and ticket keys to every symbol the
/// structure stage wrote. Only called on a git work tree.
///
/// Fills `commits`, `tickets` and the three history buckets, so that
/// `history_hits + history_misses + history_skipped == symbols`. History
/// faults degrade per file or symbol (see [`build_index`]); only an index
/// write error aborts the stage.
async fn index_history(
    transaction: &Transaction,
    cache_conn: &Connection,
    repo: &Path,
    files: &[IndexedFile],
    ticket_regex: &Regex,
    stats: &mut IndexStats,
) -> Result<()> {
    let paths: Vec<PathBuf> = files.iter().map(|file| file.path.clone()).collect();
    let dirty = dirty_files(repo, &paths);

    for file in files {
        let relative = file.path.as_path();
        // One `git log -1` per file keys the whole file's cache entries. A file
        // no commit touches has no history to look up, so it is skipped once
        // here rather than failing one `git log -L` per symbol.
        let file_last_commit = match history::file_last_commit(repo, relative) {
            Ok(Some(sha)) => sha,
            Ok(None) => {
                tracing::warn!(
                    path = %relative.display(),
                    "no commit touches this file; indexing it without history"
                );
                stats.history_skipped += file.symbols.len();
                continue;
            }
            Err(err) => {
                tracing::warn!(
                    path = %relative.display(),
                    error = %err,
                    "skipping history for file"
                );
                stats.history_skipped += file.symbols.len();
                continue;
            }
        };

        let cacheable = !dirty.contains(relative);
        // A file's cache writes share one transaction on the cache connection,
        // so a cold run commits once per file rather than once per symbol. It
        // stays separate from the index transaction, so an aborted index never
        // rolls back history that was already computed.
        let cache_transaction = if cacheable {
            match cache_conn.transaction().await {
                Ok(cache_transaction) => Some(cache_transaction),
                Err(err) => {
                    tracing::warn!(
                        error = %err,
                        "cannot batch history cache writes; writing them one by one"
                    );
                    None
                }
            }
        } else {
            None
        };
        let cache = HistoryCache::new(cache_transaction.as_deref().unwrap_or(cache_conn));
        for indexed in &file.symbols {
            let commits = match symbol_history(
                &cache,
                repo,
                relative,
                &indexed.symbol,
                &indexed.content_hash,
                &file_last_commit,
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
            attach_history(transaction, indexed.id, &commits, ticket_regex, stats).await?;
        }
        if let Some(cache_transaction) = cache_transaction
            && let Err(err) = cache_transaction.commit().await
        {
            tracing::warn!(
                path = %relative.display(),
                error = %err,
                "could not commit history cache writes"
            );
        }
    }
    Ok(())
}

/// Ticket stage: every distinct key in `symbol_tickets` (read on the build
/// transaction, so this run's uncommitted rows) is looked up in the ticket
/// cache; only the missing keys are fetched, at most `jira.concurrency` at a
/// time, and every cached entry is copied into the index `tickets` table.
///
/// Outcomes per fetched key: content → cached and copied; 403/404 → cached
/// and copied as unavailable; bad credentials → the run fails and nothing is
/// cached for that key; anything else (429 after the retries, transport,
/// other status, unparseable issue, invalid key) → one warning, counted in
/// `tickets_failed`, not cached, so the next run retries it.
///
/// Before the first fetch (never on a run with nothing to fetch) the source's
/// auth check runs once and counts as one Jira request: rejected credentials
/// fail the run before any key is fetched, so a server that answers every
/// request with a plain 403 (Jira Cloud given a bearer token) cannot get every
/// key cached as unavailable. Any other check failure skips all fetches like
/// the circuit breaker: one warning, every missing key `tickets_not_fetched`.
///
/// A transport failure or a 429 that outlasts its retries trips a circuit
/// breaker on the first occurrence: no further fetch starts, in-flight fetches
/// are aborted, every key left is counted in `tickets_not_fetched` and one
/// warning says so. Such a failure means Jira is unreachable or throttling the
/// whole account, so every other key would fail the same way, each costing up
/// to the request timeout or the full back-off.
///
/// Results are written by this function alone, as they arrive, so the fetches
/// of an aborted run are kept in the cache. A cache read fault is a miss and a
/// failed cache write only loses the reuse; both warn.
async fn index_tickets(
    transaction: &Transaction,
    cache_conn: &Connection,
    jira: Option<&TicketFetch>,
    stats: &mut IndexStats,
) -> Result<()> {
    let keys = distinct_ticket_keys(transaction).await?;
    stats.ticket_keys = keys.len();
    let cache = TicketCache::new(cache_conn);
    let mut known: BTreeMap<String, CachedTicket> = BTreeMap::new();
    let mut missing = Vec::new();
    for key in keys {
        match cache.get(&key).await {
            Ok(Some(entry)) => {
                stats.ticket_hits += 1;
                known.insert(key, entry);
            }
            Ok(None) => missing.push(key),
            Err(err) => {
                tracing::warn!(
                    key,
                    error = format!("{err:#}"),
                    "unreadable ticket cache row; treating it as a miss"
                );
                missing.push(key);
            }
        }
    }

    match jira {
        Some(jira) => fetch_tickets(&cache, jira, missing, &mut known, stats).await?,
        None => stats.tickets_not_fetched += missing.len(),
    }
    if stats.tickets_failed > 0 {
        tracing::warn!(
            tickets = stats.tickets_failed,
            "some tickets could not be fetched; they are not cached and are retried next run"
        );
    }

    for (key, entry) in &known {
        write_ticket(transaction, key, entry).await?;
    }
    Ok(())
}

/// Fetch `missing` through `jira` with bounded concurrency, caching and
/// collecting each outcome as it arrives (see [`index_tickets`]).
async fn fetch_tickets(
    cache: &TicketCache<'_>,
    jira: &TicketFetch,
    missing: Vec<String>,
    known: &mut BTreeMap<String, CachedTicket>,
    stats: &mut IndexStats,
) -> Result<()> {
    if missing.is_empty() {
        return Ok(());
    }
    stats.jira_requests += 1;
    match jira.source.check_auth().await {
        Ok(()) => {}
        Err(err @ FetchError::Unauthorized { .. }) => {
            return Err(anyhow::Error::new(err)).context("checking Jira credentials");
        }
        Err(err) => {
            stats.tickets_not_fetched += missing.len();
            tracing::warn!(
                error = %err,
                remaining = missing.len(),
                "Jira auth check failed; not fetching tickets this run"
            );
            return Ok(());
        }
    }
    let requests = Arc::new(AtomicUsize::new(0));
    let mut pending = missing.into_iter();
    let mut tasks = tokio::task::JoinSet::new();
    let mut tripped = false;
    loop {
        while !tripped
            && tasks.len() < jira.concurrency
            && let Some(key) = pending.next()
        {
            let jira = jira.clone();
            let requests = Arc::clone(&requests);
            tasks.spawn(async move { fetch_with_retry(&jira, key, &requests).await });
        }
        let Some(joined) = tasks.join_next().await else {
            break;
        };
        let Fetched { key, result } = match joined {
            Ok(fetched) => fetched,
            Err(err) if err.is_cancelled() => {
                stats.tickets_not_fetched += 1;
                continue;
            }
            Err(err) => return Err(err).context("ticket fetch task failed"),
        };
        let entry = match result {
            Ok(ticket) => {
                if ticket.key != key {
                    tracing::debug!(
                        requested = key,
                        returned = ticket.key,
                        "Jira returned another key (moved issue?); caching under the requested key"
                    );
                }
                stats.tickets_fetched += 1;
                CachedTicket::Available(Ticket {
                    key: key.clone(),
                    ..ticket
                })
            }
            Err(FetchError::Unavailable { status }) => {
                stats.tickets_unavailable += 1;
                if let Err(err) = cache.put_unavailable(&key, status).await {
                    tracing::warn!(key, error = format!("{err:#}"), "could not cache ticket");
                }
                known.insert(key, CachedTicket::Unavailable);
                continue;
            }
            Err(err @ FetchError::Unauthorized { .. }) => {
                return Err(anyhow::Error::new(err))
                    .with_context(|| format!("fetching ticket {key}"));
            }
            Err(err) => {
                tracing::warn!(key, error = %err, "could not fetch ticket; skipping it this run");
                stats.tickets_failed += 1;
                if !tripped
                    && matches!(
                        err,
                        FetchError::Transport(_) | FetchError::RateLimited { .. }
                    )
                {
                    tripped = true;
                    tasks.abort_all();
                    let queued = pending.by_ref().count();
                    stats.tickets_not_fetched += queued;
                    tracing::warn!(
                        key,
                        remaining = queued + tasks.len(),
                        "Jira is unreachable or rate limiting; not fetching the remaining tickets this run"
                    );
                }
                continue;
            }
        };
        if let CachedTicket::Available(ticket) = &entry
            && let Err(err) = cache.put_available(&key, ticket).await
        {
            tracing::warn!(key, error = format!("{err:#}"), "could not cache ticket");
        }
        known.insert(key, entry);
    }
    stats.jira_requests += requests.load(Ordering::Relaxed);
    Ok(())
}

/// The distinct ticket keys in `symbol_tickets`, sorted.
async fn distinct_ticket_keys(conn: &Connection) -> Result<Vec<String>> {
    let mut rows = conn
        .query(
            "SELECT DISTINCT ticket_key FROM symbol_tickets ORDER BY ticket_key",
            (),
        )
        .await
        .context("reading ticket keys")?;
    let mut keys = Vec::new();
    while let Some(row) = rows.next().await.context("reading ticket key row")? {
        keys.push(
            row.get::<String>(0)
                .context("reading symbol_tickets.ticket_key")?,
        );
    }
    Ok(keys)
}

/// Copy one cached entry into the index `tickets` table.
async fn write_ticket(conn: &Connection, key: &str, entry: &CachedTicket) -> Result<()> {
    let ticket = match entry {
        CachedTicket::Available(ticket) => Some(ticket),
        CachedTicket::Unavailable => None,
    };
    conn.execute(
        "INSERT INTO tickets (key, unavailable, issue_type, summary, description, parent_key)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            key,
            i64::from(ticket.is_none()),
            ticket.map(|ticket| ticket.issue_type.as_str()),
            ticket.map(|ticket| ticket.summary.as_str()),
            ticket.and_then(|ticket| ticket.description.as_deref()),
            ticket.and_then(|ticket| ticket.parent_key.as_deref()),
        ],
    )
    .await
    .with_context(|| format!("writing ticket {key}"))?;
    Ok(())
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

    for span in ticket_span(commits, ticket_regex) {
        let inserted = conn
            .execute(
                "INSERT OR IGNORE INTO symbol_tickets
                    (symbol_id, ticket_key, first_date, last_date)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    symbol_id,
                    span.key.as_str(),
                    span.first_date,
                    span.last_date
                ],
            )
            .await
            .with_context(|| format!("writing ticket {} for symbol {symbol_id}", span.key))?;
        stats.tickets += inserted as usize;
    }
    Ok(())
}

/// One ticket key and the dates of the oldest and most recent commit that
/// mention it.
struct TicketSpan {
    key: String,
    first_date: String,
    last_date: String,
}

/// The distinct ticket keys mentioned across `commits` (newest first), each
/// with its oldest (`first_date`) and most recent (`last_date`) date, in the
/// order they were first seen.
fn ticket_span(commits: &[Commit], ticket_regex: &Regex) -> Vec<TicketSpan> {
    let mut spans: Vec<TicketSpan> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    for commit in commits {
        let mut text = commit.subject.clone();
        text.push('\n');
        text.push_str(&commit.body);
        for key in history::ticket_keys(ticket_regex, &text) {
            match index.get(&key) {
                // Seen before, so this commit is older: move the first date back.
                Some(&at) => spans[at].first_date = commit.date.clone(),
                // First sighting is the newest commit, so it sets both ends.
                None => {
                    index.insert(key.clone(), spans.len());
                    spans.push(TicketSpan {
                        key,
                        first_date: commit.date.clone(),
                        last_date: commit.date.clone(),
                    });
                }
            }
        }
    }
    spans
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
    use crate::tickets::fake::{self, Answer, FakeSource};
    use libsql::params;

    fn ticket_regex() -> Regex {
        Regex::new(DEFAULT_TICKET_REGEX).unwrap()
    }

    async fn build(repo: &Path, data_dir: &Path) -> Result<IndexStats> {
        let store = Store::open(data_dir).await.unwrap();
        build_index(&store, repo, None, &ticket_regex(), None).await
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

        let err = build_index(
            &store,
            Path::new("/does/not/exist"),
            None,
            &ticket_regex(),
            None,
        )
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
        build_index(&store, repo, None, &ticket_regex(), None)
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

    async fn fqns(data: &Path) -> Vec<String> {
        let reader = IndexReader::open(data).await.unwrap();
        let mut rows = reader
            .connection()
            .query("SELECT fqn FROM symbols ORDER BY fqn", ())
            .await
            .unwrap();
        let mut fqns = Vec::new();
        while let Some(row) = rows.next().await.unwrap() {
            fqns.push(row.get::<String>(0).unwrap());
        }
        fqns
    }

    #[tokio::test]
    async fn aborted_build_after_all_stages_keeps_previous_index_and_no_temp_file() {
        let repo = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        write(
            repo.path(),
            "src/main/java/com/acme/Old.java",
            &java_class("Old", 1),
        );
        commit(repo.path(), "GRLD-1 add old", "2024-01-01T00:00:00+00:00");
        let data = tempfile::tempdir().unwrap();
        index_git_repo(repo.path(), data.path()).await;

        std::fs::remove_file(repo.path().join("src/main/java/com/acme/Old.java")).unwrap();
        write(
            repo.path(),
            "src/main/java/com/acme/New.java",
            &java_class("New", 2),
        );
        commit(
            repo.path(),
            "GRLD-2 replace old",
            "2024-02-01T00:00:00+00:00",
        );

        let store = Store::open(data.path()).await.unwrap();
        let files = walk::java_files(repo.path(), None).unwrap();
        let build = store.begin_index().await.unwrap();
        let transaction = build.connection().transaction().await.unwrap();
        let mut stats = IndexStats::default();
        let indexed = index_structure(&transaction, repo.path(), &files, &mut stats)
            .await
            .unwrap();
        index_history(
            &transaction,
            store.cache(),
            repo.path(),
            &indexed,
            &ticket_regex(),
            &mut stats,
        )
        .await
        .unwrap();
        assert_eq!(stats.symbols, 2, "both stages ran on the new sources");
        assert_eq!(stats.history_misses, 2);
        assert!(stats.tickets > 0, "the history stage wrote ticket rows");

        // A later stage failing makes the orchestrator return early, dropping
        // the transaction and then the build, never committing either.
        drop(transaction);
        drop(build);

        assert_eq!(
            fqns(data.path()).await,
            vec!["com.acme.Old", "com.acme.Old#value()"],
            "an aborted build must leave the previous index.db untouched"
        );
        let leftovers = std::fs::read_dir(data.path())
            .unwrap()
            .filter(|entry| {
                entry
                    .as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".index-")
            })
            .count();
        assert_eq!(leftovers, 0, "the aborted build's temp file is removed");
    }

    #[tokio::test]
    async fn path_run_replaces_the_index_with_only_that_prefix() {
        let repo = tempfile::tempdir().unwrap();
        write(
            repo.path(),
            "src/main/java/com/acme/a/A.java",
            "package com.acme.a;\nclass A {}\n",
        );
        write(
            repo.path(),
            "src/main/java/com/acme/b/B.java",
            "package com.acme.b;\nclass B {}\n",
        );
        let data = tempfile::tempdir().unwrap();
        let store = Store::open(data.path()).await.unwrap();

        build_index(&store, repo.path(), None, &ticket_regex(), None)
            .await
            .unwrap();
        assert_eq!(
            fqns(data.path()).await,
            vec!["com.acme.a.A", "com.acme.b.B"]
        );

        let stats = build_index(
            &store,
            repo.path(),
            Some(Path::new("src/main/java/com/acme/a")),
            &ticket_regex(),
            None,
        )
        .await
        .unwrap();
        assert_eq!(stats.symbols, 1);
        assert_eq!(
            fqns(data.path()).await,
            vec!["com.acme.a.A"],
            "--path narrows the whole index by design (open #19)"
        );
    }

    async fn index_with(
        repo: &Path,
        data: &Path,
        jira: Option<&TicketFetch>,
    ) -> Result<IndexStats> {
        let store = Store::open(data).await.unwrap();
        build_index(&store, repo, None, &ticket_regex(), jira).await
    }

    /// A git repo whose one file was committed once per message.
    fn repo_with_commits(messages: &[&str]) -> tempfile::TempDir {
        let repo = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        for (value, message) in messages.iter().enumerate() {
            write(
                repo.path(),
                "src/main/java/com/acme/Service.java",
                &java(value as i64),
            );
            commit(
                repo.path(),
                message,
                &format!("2024-01-{:02}T00:00:00+00:00", value + 1),
            );
        }
        repo
    }

    /// `(key, unavailable, summary)` for every row of the index `tickets`.
    async fn indexed_tickets(data: &Path) -> Vec<(String, i64, Option<String>)> {
        let reader = IndexReader::open(data).await.unwrap();
        let mut rows = reader
            .connection()
            .query(
                "SELECT key, unavailable, summary FROM tickets ORDER BY key",
                (),
            )
            .await
            .unwrap();
        let mut out = Vec::new();
        while let Some(row) = rows.next().await.unwrap() {
            out.push((
                row.get::<String>(0).unwrap(),
                row.get::<i64>(1).unwrap(),
                row.get::<Option<String>>(2).unwrap(),
            ));
        }
        out
    }

    fn assert_ticket_buckets(stats: &IndexStats) {
        assert_eq!(
            stats.ticket_hits
                + stats.tickets_fetched
                + stats.tickets_unavailable
                + stats.tickets_failed
                + stats.tickets_not_fetched,
            stats.ticket_keys,
            "{stats:?}"
        );
    }

    #[tokio::test]
    async fn second_run_makes_no_jira_calls() {
        let repo = repo_with_commits(&["GRLD-1 add", "GRLD-2 change", "GRLD-3 change"]);
        let data = tempfile::tempdir().unwrap();
        let source = FakeSource::new();
        source.script("GRLD-1", &[Answer::Ok(fake::ticket("GRLD-1"))]);
        source.script("GRLD-2", &[Answer::Unavailable(404)]);
        source.script("GRLD-3", &[Answer::Unavailable(403)]);
        let jira = fake::instant(source.clone(), 4);

        let cold = index_with(repo.path(), data.path(), Some(&jira))
            .await
            .unwrap();
        assert_eq!(cold.ticket_keys, 3);
        assert_eq!(cold.tickets_fetched, 1);
        assert_eq!(cold.tickets_unavailable, 2);
        assert_eq!(cold.ticket_hits, 0);
        assert_eq!(cold.jira_requests, 1 + 3, "one auth check, three fetches");
        assert_eq!(source.auth_checks(), 1);
        assert_eq!(source.calls(), 3);
        assert_ticket_buckets(&cold);
        assert_eq!(
            cold.history_hits + cold.history_misses + cold.history_skipped,
            cold.symbols,
            "the R1 invariant still holds"
        );
        let expected = vec![
            (
                "GRLD-1".to_string(),
                0,
                Some("Summary of GRLD-1".to_string()),
            ),
            ("GRLD-2".to_string(), 1, None),
            ("GRLD-3".to_string(), 1, None),
        ];
        assert_eq!(indexed_tickets(data.path()).await, expected);
        assert_eq!(cache_count(data.path(), "ticket_cache").await, 3);

        let warm = index_with(repo.path(), data.path(), Some(&jira))
            .await
            .unwrap();
        assert_eq!(source.calls(), 3, "a second run makes no Jira calls");
        assert_eq!(
            source.auth_checks(),
            1,
            "no auth check when every key is cached"
        );
        assert_eq!(warm.jira_requests, 0);
        assert_eq!(warm.ticket_hits, 3);
        assert_eq!(warm.tickets_fetched + warm.tickets_unavailable, 0);
        assert_ticket_buckets(&warm);
        assert_eq!(indexed_tickets(data.path()).await, expected);

        let reader = IndexReader::open(data.path()).await.unwrap();
        let output = crate::show::render(reader.connection(), "com.acme.Service#value()")
            .await
            .unwrap();
        assert!(
            output.contains(
                ") [Story] Summary of GRLD-1
"
            ),
            "{output}"
        );
        assert!(output.contains(") (unavailable)"), "{output}");
    }

    #[tokio::test]
    async fn failed_fetches_are_not_cached_and_are_retried_next_run() {
        let repo = repo_with_commits(&["GRLD-1 add", "GRLD-2 change"]);
        let data = tempfile::tempdir().unwrap();
        let source = FakeSource::new();
        source.script(
            "GRLD-1",
            &[Answer::Status(503), Answer::Ok(fake::ticket("GRLD-1"))],
        );
        source.script("GRLD-2", &[Answer::RateLimited(None)]);
        let jira = fake::instant(source.clone(), 4);

        let first = index_with(repo.path(), data.path(), Some(&jira))
            .await
            .expect("transport-class failures do not fail the run");
        assert_eq!(first.tickets_failed, 2);
        assert_eq!(first.tickets_fetched, 0);
        assert_eq!(
            first.jira_requests,
            1 + 1 + 1 + crate::tickets::MAX_RATE_LIMIT_RETRIES as usize,
            "auth check, 503 once, 429 plus its retries"
        );
        assert_ticket_buckets(&first);
        assert_eq!(cache_count(data.path(), "ticket_cache").await, 0);
        assert!(indexed_tickets(data.path()).await.is_empty());

        let second = index_with(repo.path(), data.path(), Some(&jira))
            .await
            .unwrap();
        assert_eq!(second.tickets_fetched, 1, "GRLD-1 is retried and succeeds");
        assert_eq!(second.tickets_failed, 1);
        assert_eq!(source.calls_for("GRLD-1"), 2);
        assert_eq!(cache_count(data.path(), "ticket_cache").await, 1);
    }

    #[tokio::test]
    async fn bad_credentials_fail_the_run_cache_nothing_and_keep_the_previous_index() {
        let repo = repo_with_commits(&["GRLD-1 add"]);
        let data = tempfile::tempdir().unwrap();
        index_with(repo.path(), data.path(), None).await.unwrap();
        write(
            repo.path(),
            "src/main/java/com/acme/Other.java",
            &java_class("Other", 1),
        );
        commit(repo.path(), "GRLD-2 other", "2024-02-01T00:00:00+00:00");
        let source = FakeSource::new();
        source.script("GRLD-1", &[Answer::Unauthorized]);
        source.script("GRLD-2", &[Answer::Unauthorized]);

        let err = index_with(repo.path(), data.path(), Some(&fake::instant(source, 1)))
            .await
            .expect_err("bad credentials must fail the run");

        assert!(format!("{err:#}").contains("credentials"), "{err:#}");
        assert_eq!(cache_count(data.path(), "ticket_cache").await, 0);
        assert_eq!(
            fqns(data.path()).await,
            vec!["com.acme.Service", "com.acme.Service#value()"],
            "the previous index is untouched"
        );
    }

    #[tokio::test]
    async fn rejected_auth_check_fails_the_run_before_any_fetch() {
        let repo = repo_with_commits(&["GRLD-1 add", "GRLD-2 change"]);
        let data = tempfile::tempdir().unwrap();
        index_with(repo.path(), data.path(), None).await.unwrap();
        let source = FakeSource::new();
        source.script_auth(Answer::Unauthorized);
        source.script("GRLD-1", &[Answer::Unavailable(403)]);
        source.script("GRLD-2", &[Answer::Unavailable(403)]);

        let err = index_with(
            repo.path(),
            data.path(),
            Some(&fake::instant(source.clone(), 4)),
        )
        .await
        .expect_err("rejected credentials must fail the run");

        assert!(format!("{err:#}").contains("credentials"), "{err:#}");
        assert_eq!(source.auth_checks(), 1);
        assert_eq!(source.calls(), 0, "no ticket is fetched");
        assert!(ticket_cache_keys(data.path()).await.is_empty());
        assert_eq!(
            fqns(data.path()).await,
            vec!["com.acme.Service", "com.acme.Service#value()"],
            "the previous index is untouched"
        );
    }

    #[tokio::test]
    async fn failed_auth_check_skips_fetching_without_failing_the_run() {
        let repo = repo_with_commits(&["GRLD-1 add", "GRLD-2 change"]);
        let data = tempfile::tempdir().unwrap();
        let source = FakeSource::new();
        source.script_auth(Answer::Transport);
        source.script("GRLD-1", &[Answer::Ok(fake::ticket("GRLD-1"))]);

        let stats = index_with(
            repo.path(),
            data.path(),
            Some(&fake::instant(source.clone(), 4)),
        )
        .await
        .expect("an unreachable Jira does not fail the run");

        assert_eq!(stats.tickets_not_fetched, 2);
        assert_eq!(stats.jira_requests, 1);
        assert_ticket_buckets(&stats);
        assert_eq!(source.calls(), 0);
        assert!(ticket_cache_keys(data.path()).await.is_empty());
    }

    #[tokio::test]
    async fn offline_run_uses_cached_tickets_and_makes_no_calls() {
        let repo = repo_with_commits(&["GRLD-1 add"]);
        let data = tempfile::tempdir().unwrap();
        let source = FakeSource::new();
        source.script("GRLD-1", &[Answer::Ok(fake::ticket("GRLD-1"))]);
        index_with(
            repo.path(),
            data.path(),
            Some(&fake::instant(source.clone(), 4)),
        )
        .await
        .unwrap();
        write(repo.path(), "src/main/java/com/acme/Service.java", &java(9));
        commit(repo.path(), "GRLD-2 change", "2024-02-01T00:00:00+00:00");
        let store = Store::open(data.path()).await.unwrap();
        TicketCache::new(store.cache())
            .put_available("GRLD-99", &fake::ticket("GRLD-99"))
            .await
            .unwrap();

        let offline = index_with(repo.path(), data.path(), None).await.unwrap();

        assert_eq!(source.calls(), 1);
        assert_eq!(offline.jira_requests, 0);
        assert_eq!(offline.ticket_hits, 1);
        assert_eq!(offline.tickets_not_fetched, 1, "GRLD-2 is not in the cache");
        assert_ticket_buckets(&offline);
        assert_eq!(
            indexed_tickets(data.path()).await,
            vec![(
                "GRLD-1".to_string(),
                0,
                Some("Summary of GRLD-1".to_string())
            )],
            "cached tickets reach the index offline; uncached and unreferenced ones have no row"
        );
    }

    #[tokio::test]
    async fn aborted_run_keeps_the_fetches_it_already_cached() {
        let repo = repo_with_commits(&["GRLD-1 add", "GRLD-2 change", "GRLD-3 change"]);
        let data = tempfile::tempdir().unwrap();
        index_with(repo.path(), data.path(), None).await.unwrap();
        let source = FakeSource::new();
        source.script("GRLD-1", &[Answer::Ok(fake::ticket("GRLD-1"))]);
        source.script("GRLD-2", &[Answer::Unauthorized]);
        source.script("GRLD-3", &[Answer::Ok(fake::ticket("GRLD-3"))]);

        index_with(
            repo.path(),
            data.path(),
            Some(&fake::instant(source.clone(), 1)),
        )
        .await
        .expect_err("bad credentials must fail the run");

        assert_eq!(ticket_cache_keys(data.path()).await, vec!["GRLD-1"]);
        assert_eq!(source.calls_for("GRLD-3"), 0);
        assert!(
            indexed_tickets(data.path()).await.is_empty(),
            "the previous index is untouched"
        );
        assert_eq!(
            fqns(data.path()).await,
            vec!["com.acme.Service", "com.acme.Service#value()"]
        );
    }

    async fn ticket_cache_keys(data: &Path) -> Vec<String> {
        let store = Store::open(data).await.unwrap();
        let mut rows = store
            .cache()
            .query("SELECT key FROM ticket_cache ORDER BY key", ())
            .await
            .unwrap();
        let mut keys = Vec::new();
        while let Some(row) = rows.next().await.unwrap() {
            keys.push(row.get::<String>(0).unwrap());
        }
        keys
    }

    #[tokio::test]
    async fn transport_failure_trips_the_breaker_and_leaves_queued_keys_unfetched() {
        let repo = repo_with_commits(&["GRLD-1 add", "GRLD-2 change", "GRLD-3 change"]);
        let data = tempfile::tempdir().unwrap();
        let source = FakeSource::new();
        source.script("GRLD-1", &[Answer::Transport]);
        source.script("GRLD-2", &[Answer::Ok(fake::ticket("GRLD-2"))]);
        source.script("GRLD-3", &[Answer::Ok(fake::ticket("GRLD-3"))]);

        let stats = index_with(
            repo.path(),
            data.path(),
            Some(&fake::instant(source.clone(), 1)),
        )
        .await
        .expect("an unreachable Jira does not fail the run");

        assert_eq!(stats.tickets_failed, 1);
        assert_eq!(stats.tickets_not_fetched, 2);
        assert_eq!(stats.jira_requests, 1 + 1);
        assert_ticket_buckets(&stats);
        assert_eq!(source.calls(), 1, "queued keys are never requested");
        assert!(ticket_cache_keys(data.path()).await.is_empty());
        assert!(indexed_tickets(data.path()).await.is_empty());
    }

    #[tokio::test]
    async fn breaker_aborts_in_flight_fetches() {
        let repo = repo_with_commits(&["GRLD-1 add", "GRLD-2 change", "GRLD-3 change"]);
        let data = tempfile::tempdir().unwrap();
        let source = FakeSource::new();
        source.script("GRLD-1", &[Answer::Transport]);
        source.script("GRLD-2", &[Answer::Hang]);
        source.script("GRLD-3", &[Answer::Ok(fake::ticket("GRLD-3"))]);

        let stats = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            index_with(
                repo.path(),
                data.path(),
                Some(&fake::instant(source.clone(), 2)),
            ),
        )
        .await
        .expect("the hanging fetch is aborted, not awaited")
        .unwrap();

        assert_eq!(stats.tickets_failed, 1);
        assert_eq!(stats.tickets_not_fetched, 2);
        assert_eq!(
            stats.jira_requests,
            1 + 2,
            "the auth check, and the aborted request was sent"
        );
        assert_ticket_buckets(&stats);
        assert_eq!(source.calls_for("GRLD-3"), 0);
        assert!(ticket_cache_keys(data.path()).await.is_empty());
    }

    #[tokio::test]
    async fn exhausted_rate_limit_trips_the_breaker() {
        let repo = repo_with_commits(&["GRLD-1 add", "GRLD-2 change"]);
        let data = tempfile::tempdir().unwrap();
        let source = FakeSource::new();
        source.script("GRLD-1", &[Answer::RateLimited(None)]);
        source.script("GRLD-2", &[Answer::Ok(fake::ticket("GRLD-2"))]);

        let stats = index_with(
            repo.path(),
            data.path(),
            Some(&fake::instant(source.clone(), 1)),
        )
        .await
        .unwrap();

        assert_eq!(stats.tickets_failed, 1);
        assert_eq!(stats.tickets_not_fetched, 1);
        assert_eq!(
            stats.jira_requests,
            1 + 1 + crate::tickets::MAX_RATE_LIMIT_RETRIES as usize
        );
        assert_ticket_buckets(&stats);
        assert_eq!(source.calls_for("GRLD-2"), 0);
        assert!(ticket_cache_keys(data.path()).await.is_empty());
    }

    #[tokio::test]
    async fn moved_issue_is_cached_under_the_requested_key() {
        let repo = repo_with_commits(&["GRLD-1 add"]);
        let data = tempfile::tempdir().unwrap();
        let source = FakeSource::new();
        source.script("GRLD-1", &[Answer::Ok(fake::ticket("NEW-7"))]);
        let jira = fake::instant(source.clone(), 4);

        index_with(repo.path(), data.path(), Some(&jira))
            .await
            .unwrap();
        let warm = index_with(repo.path(), data.path(), Some(&jira))
            .await
            .unwrap();

        assert_eq!(source.calls(), 1);
        assert_eq!(warm.ticket_hits, 1);
        assert_eq!(
            indexed_tickets(data.path()).await,
            vec![(
                "GRLD-1".to_string(),
                0,
                Some("Summary of NEW-7".to_string())
            )]
        );
    }

    #[tokio::test]
    async fn fetches_run_concurrently_up_to_the_cap() {
        let keys: Vec<String> = (1..=10).map(|n| format!("GRLD-{n}")).collect();
        let repo = repo_with_commits(&[&keys.join(" ")]);
        let data = tempfile::tempdir().unwrap();
        let source = FakeSource::new();
        for key in &keys {
            source.script(key, &[Answer::Ok(fake::ticket(key))]);
        }

        let stats = index_with(
            repo.path(),
            data.path(),
            Some(&fake::instant(source.clone(), 3)),
        )
        .await
        .unwrap();

        assert_eq!(stats.tickets_fetched, 10);
        assert_eq!(source.calls(), 10);
        assert_eq!(source.peak(), 3, "three requests overlap, never more");
    }
}
