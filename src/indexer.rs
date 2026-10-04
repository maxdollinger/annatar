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
//!    `tickets` table; runs after history, on git work trees only;
//! 4. **summaries** (`index_summaries`) gives every available ticket in the
//!    index `tickets` table an English summary and purpose from the chat
//!    model (through the LLM cache); runs after tickets;
//! 5. **describe** (`index_descriptions`) gives every method and constructor
//!    a short `description` from the chat model, built from its code, its
//!    enclosing type, its tickets' summaries and its commit subjects; runs
//!    after summaries, also on a repository without git (code only);
//! 6. **types** (`index_type_descriptions`) gives every type a `description`
//!    the same way, bottom-up (nested types first), from its declaration, its
//!    members' and nested types' descriptions and its history; runs after
//!    describe.
//!
//! A stage never commits: the transaction commits and the temporary file is
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

use crate::config::DescribeConfig;
use crate::describe::{
    ChildDescription, Description, Member, MemberCommit, MemberTicket, Parent, Summary,
    TicketState, TypeChild, TypeContext, TypeDescription, is_public, member_prompt, outline,
    select_children, select_history, type_prompt, validate_description, validate_type_description,
};
use crate::history::{self, Commit, HistoryCache};
use crate::jira::{FetchError, Ticket};
use crate::llm::{InvalidOutput, LlmStats, LlmUnavailable};
use crate::store::Store;
use crate::summaries::{Summarizer, TicketSummary, ticket_prompt, validate_summary};
use crate::symbols::{JavaParser, Symbol, SymbolKind};
use crate::tickets::{
    CachedTicket, Fetched, JiraMode, TicketCache, TicketFetch, check_auth_with_retry,
    fetch_with_retry,
};
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
    /// Keys that failed permanently this run (403/404, another 4xx except
    /// 408/425/429, an unparseable issue, not a Jira key), cached as
    /// unavailable.
    pub tickets_unavailable: usize,
    /// Keys whose fetch failed transiently this run (rate limit after
    /// retries, transport, 5xx, a response that is not JSON); not cached,
    /// retried next run.
    pub tickets_failed: usize,
    /// Keys missing from the cache that were not fetched because Jira is not
    /// configured, has no token, the run is `--offline`, or the circuit
    /// breaker stopped fetching after a transport failure or exhausted 429.
    pub tickets_not_fetched: usize,
    /// Jira requests sent, retries and fetches aborted by the circuit breaker
    /// included. Zero on a fully cached run.
    pub jira_requests: usize,
    /// Available tickets in the index `tickets` table. When the summaries
    /// stage runs, `summaries + summaries_invalid + summaries_failed +
    /// summaries_skipped == summary_tickets`.
    pub summary_tickets: usize,
    /// Tickets given a summary and purpose, from the LLM cache or the model.
    pub summaries: usize,
    /// Tickets whose reply was still invalid after the retry; not cached,
    /// retried next run.
    pub summaries_invalid: usize,
    /// Tickets whose chat call failed (backend error); this trips the
    /// summaries circuit breaker.
    pub summaries_failed: usize,
    /// Tickets left without a summary and not sent to the model: no
    /// `[ollama]` section, or not in the LLM cache with `--no-llm` or after
    /// the circuit breaker tripped.
    pub summaries_skipped: usize,
    /// LLM client counters of the summaries stage.
    pub summary_llm: LlmStats,
    /// Methods and constructors indexed. When an LLM stage runs, `described +
    /// describe_invalid + describe_failed + describe_incomplete +
    /// describe_skipped == describe_members`.
    pub describe_members: usize,
    /// Members given a description, from the LLM cache or the model.
    pub described: usize,
    /// Of `described`, those answered from the LLM cache.
    pub described_cached: usize,
    /// Members whose reply was still invalid after the retry; not cached,
    /// asked again next run.
    pub describe_invalid: usize,
    /// Members whose chat call failed (backend error; trips the breaker).
    pub describe_failed: usize,
    /// Members not described because their input is incomplete this run: a
    /// chosen ticket was not fetched yet or has no summary (circuit breaker,
    /// `--no-llm`). Described on a later run that has it.
    pub describe_incomplete: usize,
    /// Members not sent to the model: no `[ollama]` section, or not in the
    /// LLM cache with `--no-llm` or after the circuit breaker tripped.
    pub describe_skipped: usize,
    /// LLM client counters of the describe stage.
    pub describe_llm: LlmStats,
    /// Types indexed. When an LLM stage runs, `types_described +
    /// types_invalid + types_failed + types_incomplete + types_skipped ==
    /// type_symbols`.
    pub type_symbols: usize,
    /// Types given a description, from the LLM cache or the model.
    pub types_described: usize,
    /// Of `types_described`, those answered from the LLM cache.
    pub types_described_cached: usize,
    /// Types whose reply was still invalid after the retry; not cached,
    /// asked again next run.
    pub types_invalid: usize,
    /// Types whose chat call failed (backend error; trips the breaker).
    pub types_failed: usize,
    /// Types held back because their input is incomplete this run: a chosen
    /// ticket was not fetched yet or has no summary, or a listed member or
    /// nested type has no description this run. Described on a later run.
    pub types_incomplete: usize,
    /// Types not sent to the model: no `[ollama]` section, or not in the LLM
    /// cache with `--no-llm` or after the circuit breaker tripped.
    pub types_skipped: usize,
    /// LLM client counters of the type stage.
    pub type_llm: LlmStats,
    /// LLM client counters for this run (all stages); `llm.chat_calls` is
    /// zero on a fully cached run.
    pub llm: LlmStats,
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
/// `jira` is how missing tickets are fetched; [`JiraMode::Offline`] and
/// [`JiraMode::Disabled`] still copy already-cached tickets into the index but
/// make no request (with `Disabled` the describe stage treats the uncached
/// ones as unavailable). Bad Jira credentials abort the run; see
/// `index_tickets` for the other outcomes.
///
/// `llm` summarises the available tickets and describes every method,
/// constructor and type; `None` (no `[ollama]` section) leaves them without a summary
/// or description. Its chat model is checked once before the
/// stages run: a model the server does not have fails the run (like rejected
/// Jira credentials). Other LLM failures never fail the run; see
/// `index_summaries`, `index_descriptions` and `index_type_descriptions`.
pub async fn build_index(
    store: &Store,
    repo: &Path,
    path_prefix: Option<&Path>,
    ticket_regex: &Regex,
    jira: &JiraMode,
    llm: Option<&Summarizer>,
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

    if let Some(llm) = llm {
        llm.client.check_chat_model().await?;
    }

    let build = store.begin_index().await?;
    let transaction = build
        .connection()
        .transaction()
        .await
        .context("starting index transaction")?;
    let mut stats = IndexStats::default();
    let llm_stats = || llm.map(|llm| llm.client.stats()).unwrap_or_default();
    let run_before = llm_stats();

    let indexed = index_structure(&transaction, repo, &files, &mut stats).await?;
    let mut invalid_summaries = HashSet::new();
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
        let before = llm_stats();
        invalid_summaries = index_summaries(&transaction, llm, ticket_regex, &mut stats).await?;
        stats.summary_llm = llm_stats().since(&before);
    }
    let before = llm_stats();
    let unknown_tickets = match jira {
        JiraMode::Disabled => UnknownTickets::Unavailable,
        JiraMode::Fetch(_) | JiraMode::Offline => UnknownTickets::Pending,
    };
    let inputs = DescribeInputs {
        ticket_regex,
        invalid_summaries: &invalid_summaries,
        unknown_tickets,
    };
    let described = index_descriptions(&transaction, &indexed, llm, &inputs, &mut stats).await?;
    stats.describe_llm = llm_stats().since(&before);
    let before = llm_stats();
    index_type_descriptions(&transaction, &indexed, llm, &inputs, described, &mut stats).await?;
    stats.type_llm = llm_stats().since(&before);
    stats.llm = llm_stats().since(&run_before);

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
        summary_tickets = stats.summary_tickets,
        summaries = stats.summaries,
        summaries_invalid = stats.summaries_invalid,
        summaries_failed = stats.summaries_failed,
        summaries_skipped = stats.summaries_skipped,
        describe_members = stats.describe_members,
        described = stats.described,
        described_cached = stats.described_cached,
        describe_invalid = stats.describe_invalid,
        describe_failed = stats.describe_failed,
        describe_incomplete = stats.describe_incomplete,
        describe_skipped = stats.describe_skipped,
        type_symbols = stats.type_symbols,
        types_described = stats.types_described,
        types_described_cached = stats.types_described_cached,
        types_invalid = stats.types_invalid,
        types_failed = stats.types_failed,
        types_incomplete = stats.types_incomplete,
        types_skipped = stats.types_skipped,
        chat_calls = stats.llm.chat_calls,
        chat_hits = stats.llm.chat_hits,
        chat_retries = stats.llm.chat_retries,
        empty = stats.empty,
        parse_errors = stats.parse_errors,
        unreadable = stats.unreadable,
        "indexed repository"
    );
    Ok(stats)
}

/// One file the structure stage parsed, with its source and the rows it
/// wrote (none if every fqn was a duplicate).
struct IndexedFile {
    path: PathBuf,
    source: String,
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
            source,
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
/// auth check runs once, with the fetches' 429 retries, each request counted
/// in `jira_requests`: rejected credentials fail the run before any key is
/// fetched, so a server that answers every request with a plain 403 (Jira
/// Cloud given a bearer token) cannot get every key cached as unavailable. A
/// 401 for a missing scope is inconclusive (`myself` needs a scope that
/// reading issues does not): it is logged at debug and the fetches go ahead,
/// where a 401 fails the run. Any other check failure skips all fetches like
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
    jira: &JiraMode,
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
        JiraMode::Fetch(jira) => fetch_tickets(&cache, jira, missing, &mut known, stats).await?,
        JiraMode::Offline | JiraMode::Disabled => stats.tickets_not_fetched += missing.len(),
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
    let requests = Arc::new(AtomicUsize::new(0));
    match check_auth_with_retry(jira, &requests).await {
        Ok(()) => {}
        Err(err @ FetchError::Unauthorized { scope: true, .. }) => {
            tracing::debug!(
                error = %err,
                "Jira auth check inconclusive (token lacks the scope for /myself); fetching anyway"
            );
        }
        Err(err @ FetchError::Unauthorized { .. }) => {
            return Err(anyhow::Error::new(err)).context("checking Jira credentials");
        }
        Err(err) => {
            stats.tickets_not_fetched += missing.len();
            stats.jira_requests += requests.load(Ordering::Relaxed);
            tracing::warn!(
                error = %err,
                remaining = missing.len(),
                "Jira auth check failed; not fetching tickets this run"
            );
            return Ok(());
        }
    }
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
            Err(err @ FetchError::Unauthorized { .. }) => {
                return Err(anyhow::Error::new(err))
                    .with_context(|| format!("fetching ticket {key}"));
            }
            Err(err) if err.is_permanent() => {
                if !matches!(err, FetchError::Unavailable { .. }) {
                    tracing::warn!(
                        key,
                        error = %err,
                        "ticket cannot be fetched; caching it as unavailable"
                    );
                }
                stats.tickets_unavailable += 1;
                if let Err(err) = cache.put_unavailable(&key, err.status()).await {
                    tracing::warn!(key, error = format!("{err:#}"), "could not cache ticket");
                }
                known.insert(key, CachedTicket::Unavailable);
                continue;
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

/// Summaries stage: every available ticket in the index `tickets` table (read
/// on the build transaction, in key order) gets an English summary and
/// purpose from the chat model, through the LLM cache, written to
/// `llm_summary` / `llm_purpose` (`NULL` purpose when the ticket gives no
/// reason). Unavailable tickets are left alone. The prompt depends on the
/// ticket alone, so a `--path` run hits the same cache rows.
///
/// Calls are sequential: the host Ollama answers one request at a time, so
/// concurrency only queues (open #23). Outcomes per ticket: a cached or valid
/// reply → written; a reply still invalid after the client's retry → one
/// warning, `summaries_invalid`, nothing cached, retried next run; a backend
/// failure (after the client's own transient retry) → `summaries_failed`. The
/// circuit breaker lives on the client (it trips on a backend failure or on
/// [`crate::llm::MAX_CONSECUTIVE_INVALID`] invalid replies in a row, and
/// `--no-llm` starts it tripped): from then on, in this and every later LLM
/// stage, only cached summaries are used and the rest count as
/// `summaries_skipped`. Without a summarizer every available ticket is
/// `summaries_skipped`. Only an index write error fails the stage.
///
/// Returns the keys whose summary was invalid, for the describe stage's
/// title fallback.
async fn index_summaries(
    transaction: &Transaction,
    llm: Option<&Summarizer>,
    ticket_regex: &Regex,
    stats: &mut IndexStats,
) -> Result<HashSet<String>> {
    let tickets = available_tickets(transaction).await?;
    stats.summary_tickets = tickets.len();
    let mut invalid = HashSet::new();
    let Some(llm) = llm else {
        stats.summaries_skipped += tickets.len();
        return Ok(invalid);
    };
    llm.client.reset_invalid_streak();
    let started = std::time::Instant::now();
    for (done, ticket) in tickets.iter().enumerate() {
        let prompt = ticket_prompt(ticket);
        let summary = match llm
            .client
            .complete_with::<TicketSummary, _>(&prompt, |summary| {
                validate_summary(summary, ticket_regex)
            })
            .await
        {
            Ok(summary) => summary,
            Err(err) if err.downcast_ref::<LlmUnavailable>().is_some() => {
                stats.summaries_skipped += 1;
                continue;
            }
            Err(err) if err.downcast_ref::<InvalidOutput>().is_some() => {
                tracing::warn!(
                    key = ticket.key,
                    error = format!("{err:#}"),
                    "no valid ticket summary; skipping it this run"
                );
                stats.summaries_invalid += 1;
                invalid.insert(ticket.key.clone());
                continue;
            }
            Err(err) => {
                tracing::warn!(
                    key = ticket.key,
                    error = format!("{err:#}"),
                    remaining = tickets.len() - done - 1,
                    "the chat model failed; remaining tickets use cached summaries only"
                );
                stats.summaries_failed += 1;
                continue;
            }
        };
        write_summary(transaction, &ticket.key, &summary).await?;
        stats.summaries += 1;
        tracing::debug!(
            key = ticket.key,
            done = done + 1,
            total = tickets.len(),
            "ticket summarised"
        );
    }
    tracing::info!(
        tickets = tickets.len(),
        summaries = stats.summaries,
        elapsed_ms = started.elapsed().as_millis() as u64,
        "ticket summaries done"
    );
    if stats.summaries_invalid + stats.summaries_failed > 0 {
        tracing::warn!(
            tickets = tickets.len(),
            summaries = stats.summaries,
            invalid = stats.summaries_invalid,
            failed = stats.summaries_failed,
            skipped = stats.summaries_skipped,
            invalid_keys = key_list(&invalid),
            "some tickets got no summary this run; they are asked again next run"
        );
    }
    Ok(invalid)
}

/// Most keys a warning lists.
const MAX_LISTED_KEYS: usize = 10;

/// `keys` sorted and joined, at most [`MAX_LISTED_KEYS`] of them, then how
/// many more.
fn key_list<'a>(keys: impl IntoIterator<Item = &'a String>) -> String {
    let keys: std::collections::BTreeSet<&String> = keys.into_iter().collect();
    let mut list = keys
        .iter()
        .take(MAX_LISTED_KEYS)
        .map(|key| key.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    if keys.len() > MAX_LISTED_KEYS {
        list.push_str(&format!(" (+{} more)", keys.len() - MAX_LISTED_KEYS));
    }
    list
}

/// What the describe stage makes of a member's ticket that is not in the
/// index `tickets` table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UnknownTickets {
    /// Jira may fetch it on a later run (`--offline`, a transient failure):
    /// a member that would choose it waits.
    Pending,
    /// Jira is not configured: it is never fetched, so it is unavailable.
    Unavailable,
}

/// What the describe and type stages take from the earlier stages and the
/// configuration.
struct DescribeInputs<'a> {
    /// Rejects replies that name a ticket key.
    ticket_regex: &'a Regex,
    /// Tickets whose summary was invalid this run: their Jira title stands in.
    invalid_summaries: &'a HashSet<String>,
    unknown_tickets: UnknownTickets,
}

/// The available tickets in the index `tickets` table, by key.
async fn available_tickets(conn: &Connection) -> Result<Vec<Ticket>> {
    let mut rows = conn
        .query(
            "SELECT key, issue_type, summary, description, parent_key
             FROM tickets WHERE unavailable = 0 ORDER BY key",
            (),
        )
        .await
        .context("reading tickets")?;
    let mut tickets = Vec::new();
    while let Some(row) = rows.next().await.context("reading tickets row")? {
        tickets.push(Ticket {
            key: row.get(0).context("reading tickets.key")?,
            issue_type: row.get(1).context("reading tickets.issue_type")?,
            summary: row.get(2).context("reading tickets.summary")?,
            description: row.get(3).context("reading tickets.description")?,
            parent_key: row.get(4).context("reading tickets.parent_key")?,
        });
    }
    Ok(tickets)
}

/// Store one ticket's summary and purpose (whitespace collapsed, an empty
/// purpose as `NULL`) in the index `tickets` table.
async fn write_summary(conn: &Connection, key: &str, summary: &TicketSummary) -> Result<()> {
    conn.execute(
        "UPDATE tickets SET llm_summary = ?2, llm_purpose = ?3 WHERE key = ?1",
        params![key, summary.summary_text(), summary.purpose_text()],
    )
    .await
    .with_context(|| format!("writing the summary of ticket {key}"))?;
    Ok(())
}

/// Describe stage: give every method and constructor the structure stage
/// wrote a description from the chat model, stored on its `symbols` row. Runs after summaries, which it reads from the index `tickets` table.
///
/// Members are taken in walk and source order and asked one at a time (the
/// host Ollama serves one request at a time, D-ap). Each prompt is built by
/// [`member_prompt`] from the member alone, so a rerun hits the LLM cache. A
/// chosen ticket whose summary was invalid this run (`invalid_summaries`)
/// stands in with its Jira title. A member whose input is incomplete (a
/// chosen ticket not fetched yet, or without a summary because the breaker
/// tripped or `--no-llm`) is `describe_incomplete` and not asked; one warning
/// lists the blocking keys. Tickets not in the index count as unavailable
/// when `unknown_tickets` says so (Jira not configured). Invalid replies,
/// backend failures and the circuit breaker behave as in `index_summaries`
/// (the breaker is shared, so a tripped summaries stage leaves this one
/// cache-only). Without a summarizer every member is `describe_skipped`. Only
/// an index write error fails the stage.
async fn index_descriptions(
    transaction: &Transaction,
    files: &[IndexedFile],
    llm: Option<&Summarizer>,
    inputs: &DescribeInputs<'_>,
    stats: &mut IndexStats,
) -> Result<HashMap<i64, ChildDescription>> {
    let mut described = HashMap::new();
    let members: Vec<(&IndexedFile, &IndexedSymbol)> = files
        .iter()
        .flat_map(|file| file.symbols.iter().map(move |symbol| (file, symbol)))
        .filter(|(_, indexed)| !indexed.symbol.kind.is_type())
        .collect();
    stats.describe_members = members.len();
    let Some(llm) = llm else {
        stats.describe_skipped += members.len();
        return Ok(described);
    };
    let types: HashMap<&str, &Symbol> = files
        .iter()
        .flat_map(|file| &file.symbols)
        .filter(|indexed| indexed.symbol.kind.is_type())
        .map(|indexed| (indexed.symbol.fqn.as_str(), &indexed.symbol))
        .collect();
    let mut tickets = member_tickets(
        transaction,
        inputs.invalid_summaries,
        inputs.unknown_tickets,
    )
    .await?;
    let mut commits = member_commits(transaction).await?;
    let config = &llm.describe;
    let mut blocking = HashSet::new();
    llm.client.reset_invalid_streak();
    let started = std::time::Instant::now();
    for (done, (file, indexed)) in members.iter().enumerate() {
        let symbol = &indexed.symbol;
        let history = match select_history(
            &tickets.remove(&indexed.id).unwrap_or_default(),
            &commits.remove(&indexed.id).unwrap_or_default(),
            config,
        ) {
            Ok(history) => history,
            Err(incomplete) => {
                tracing::debug!(fqn = %symbol.fqn, reason = %incomplete, "member input incomplete; not described this run");
                blocking.extend(incomplete.keys().cloned());
                stats.describe_incomplete += 1;
                continue;
            }
        };
        let member = Member {
            kind: symbol.kind.as_str().to_string(),
            signature: symbol.signature.clone(),
            javadoc: symbol.javadoc.clone(),
            source: file
                .source
                .get(symbol.start_byte..symbol.end_byte)
                .unwrap_or_default()
                .to_string(),
            parent: symbol
                .parent
                .as_deref()
                .and_then(|fqn| types.get(fqn))
                .map(|parent| Parent {
                    fqn: parent.fqn.clone(),
                    kind: parent.kind.as_str().to_string(),
                    role: parent.role.map(|role| role.as_str().to_string()),
                }),
        };
        let prompt = member_prompt(&member, &history, config.body_chars);
        let hits = llm.client.stats().chat_hits;
        let description = match llm
            .client
            .complete_with::<Description, _>(&prompt, |description| {
                validate_description(description, inputs.ticket_regex)
            })
            .await
        {
            Ok(description) => description,
            Err(err) if err.downcast_ref::<LlmUnavailable>().is_some() => {
                stats.describe_skipped += 1;
                continue;
            }
            Err(err) if err.downcast_ref::<InvalidOutput>().is_some() => {
                tracing::warn!(
                    fqn = %symbol.fqn,
                    error = format!("{err:#}"),
                    "no valid description; skipping the member this run"
                );
                stats.describe_invalid += 1;
                described.insert(indexed.id, ChildDescription::Invalid);
                continue;
            }
            Err(err) => {
                tracing::warn!(
                    fqn = %symbol.fqn,
                    error = format!("{err:#}"),
                    remaining = members.len() - done - 1,
                    "the chat model failed; remaining members use cached descriptions only"
                );
                stats.describe_failed += 1;
                continue;
            }
        };
        write_description(transaction, indexed.id, &description).await?;
        described.insert(indexed.id, ChildDescription::Described(description.text()));
        stats.described += 1;
        if llm.client.stats().chat_hits > hits {
            stats.described_cached += 1;
        }
        tracing::debug!(
            fqn = %symbol.fqn,
            done = done + 1,
            total = members.len(),
            elapsed_ms = started.elapsed().as_millis() as u64,
            "member described"
        );
    }
    tracing::info!(
        members = members.len(),
        described = stats.described,
        cached = stats.described_cached,
        elapsed_ms = started.elapsed().as_millis() as u64,
        "member descriptions done"
    );
    if stats.describe_invalid + stats.describe_failed + stats.describe_incomplete > 0 {
        tracing::warn!(
            members = members.len(),
            described = stats.described,
            invalid = stats.describe_invalid,
            failed = stats.describe_failed,
            incomplete = stats.describe_incomplete,
            skipped = stats.describe_skipped,
            blocking_tickets = key_list(&blocking),
            "some methods got no description this run; they are asked again next run"
        );
    }
    Ok(described)
}

/// Type stage: give every type the structure stage wrote a description from
/// the chat model, stored on its `symbols` row. Runs after the describe
/// stage, whose members' descriptions (`described`, by symbol id) it lists.
///
/// Types are taken bottom-up: the most deeply nested first, then by walk and
/// source order, so an outer type lists its nested types' descriptions. Each
/// prompt is built by [`type_prompt`] from the type's own file and history:
/// its declaration with members and nested types cut out (capped at
/// `describe.body_chars`), up to `describe.type_members` children with their
/// description (public ones first; the rest only counted) and its first plus
/// `describe.type_recent_tickets` most recent tickets, chosen like a
/// member's. A listed child without a description this run holds the type back
/// (`types_incomplete`, which in turn holds back its outer type), so nothing
/// built from partial input is cached; a child whose reply was invalid is
/// listed without one. Ticket blocking, the title fallback, invalid replies,
/// backend failures and the circuit breaker behave as in
/// `index_descriptions`. Without a summarizer every type is `types_skipped`.
/// Only an index write error fails the stage.
async fn index_type_descriptions(
    transaction: &Transaction,
    files: &[IndexedFile],
    llm: Option<&Summarizer>,
    inputs: &DescribeInputs<'_>,
    mut described: HashMap<i64, ChildDescription>,
    stats: &mut IndexStats,
) -> Result<()> {
    let types: HashMap<&str, &IndexedSymbol> = files
        .iter()
        .flat_map(|file| &file.symbols)
        .filter(|indexed| indexed.symbol.kind.is_type())
        .map(|indexed| (indexed.symbol.fqn.as_str(), indexed))
        .collect();
    let depth = |symbol: &Symbol| {
        let mut depth = 0;
        let mut parent = symbol.parent.as_deref();
        while let Some(fqn) = parent {
            depth += 1;
            parent = types
                .get(fqn)
                .and_then(|indexed| indexed.symbol.parent.as_deref());
        }
        depth
    };
    let mut order: Vec<(usize, &IndexedFile, &IndexedSymbol)> = files
        .iter()
        .flat_map(|file| file.symbols.iter().map(move |indexed| (file, indexed)))
        .filter(|(_, indexed)| indexed.symbol.kind.is_type())
        .map(|(file, indexed)| (depth(&indexed.symbol), file, indexed))
        .collect();
    order.sort_by_key(|(depth, _, _)| std::cmp::Reverse(*depth));
    stats.type_symbols = order.len();
    let Some(llm) = llm else {
        stats.types_skipped += order.len();
        return Ok(());
    };
    let mut children: HashMap<&str, Vec<&IndexedSymbol>> = HashMap::new();
    for indexed in files.iter().flat_map(|file| &file.symbols) {
        if let Some(parent) = indexed.symbol.parent.as_deref() {
            children.entry(parent).or_default().push(indexed);
        }
    }
    let mut tickets = member_tickets(
        transaction,
        inputs.invalid_summaries,
        inputs.unknown_tickets,
    )
    .await?;
    let mut commits = member_commits(transaction).await?;
    let config = &llm.describe;
    let history_config = DescribeConfig {
        recent_tickets: config.type_recent_tickets,
        ..*config
    };
    let mut blocking = HashSet::new();
    let mut held_back = 0;
    llm.client.reset_invalid_streak();
    let started = std::time::Instant::now();
    for (done, (_, file, indexed)) in order.iter().enumerate() {
        let symbol = &indexed.symbol;
        let history = match select_history(
            &tickets.remove(&indexed.id).unwrap_or_default(),
            &commits.remove(&indexed.id).unwrap_or_default(),
            &history_config,
        ) {
            Ok(history) => history,
            Err(incomplete) => {
                tracing::debug!(fqn = %symbol.fqn, reason = %incomplete, "type input incomplete; not described this run");
                blocking.extend(incomplete.keys().cloned());
                stats.types_incomplete += 1;
                continue;
            }
        };
        let own: &[&IndexedSymbol] = children
            .get(symbol.fqn.as_str())
            .map(Vec::as_slice)
            .unwrap_or_default();
        let in_interface = matches!(symbol.kind, SymbolKind::Interface | SymbolKind::Annotation);
        let listed: Vec<TypeChild> = own
            .iter()
            .map(|child| {
                let (kind, name) = child_name(symbol, &child.symbol);
                TypeChild {
                    kind,
                    name,
                    public: is_public(&child.symbol.signature, in_interface),
                    description: described
                        .get(&child.id)
                        .cloned()
                        .unwrap_or(ChildDescription::Missing),
                }
            })
            .collect();
        let listed = match select_children(&listed, config.type_members) {
            Ok(listed) => listed,
            Err(missing) => {
                tracing::debug!(fqn = %symbol.fqn, missing = missing.join(", "), "members without a description this run; type not described");
                held_back += 1;
                stats.types_incomplete += 1;
                continue;
            }
        };
        let spans: Vec<std::ops::Range<usize>> = own
            .iter()
            .map(|child| child.symbol.start_byte..child.symbol.end_byte)
            .collect();
        let context = TypeContext {
            kind: symbol.kind.as_str().to_string(),
            fqn: symbol.fqn.clone(),
            role: symbol.role.map(|role| role.as_str().to_string()),
            signature: symbol.signature.clone(),
            javadoc: symbol.javadoc.clone(),
            outline: outline(
                &file.source,
                symbol.start_byte..symbol.end_byte,
                &spans,
                &symbol.blocks,
            ),
            enclosing: symbol
                .parent
                .as_deref()
                .and_then(|fqn| types.get(fqn))
                .map(|parent| Parent {
                    fqn: parent.symbol.fqn.clone(),
                    kind: parent.symbol.kind.as_str().to_string(),
                    role: parent.symbol.role.map(|role| role.as_str().to_string()),
                }),
        };
        let prompt = type_prompt(&context, &listed, &history, config.body_chars);
        let hits = llm.client.stats().chat_hits;
        let description = match llm
            .client
            .complete_with::<TypeDescription, _>(&prompt, |description| {
                validate_type_description(description, inputs.ticket_regex)
            })
            .await
        {
            Ok(description) => Description::from(description),
            Err(err) if err.downcast_ref::<LlmUnavailable>().is_some() => {
                stats.types_skipped += 1;
                continue;
            }
            Err(err) if err.downcast_ref::<InvalidOutput>().is_some() => {
                tracing::warn!(
                    fqn = %symbol.fqn,
                    error = format!("{err:#}"),
                    "no valid description; skipping the type this run"
                );
                stats.types_invalid += 1;
                described.insert(indexed.id, ChildDescription::Invalid);
                continue;
            }
            Err(err) => {
                tracing::warn!(
                    fqn = %symbol.fqn,
                    error = format!("{err:#}"),
                    remaining = order.len() - done - 1,
                    "the chat model failed; remaining types use cached descriptions only"
                );
                stats.types_failed += 1;
                continue;
            }
        };
        write_description(transaction, indexed.id, &description).await?;
        described.insert(indexed.id, ChildDescription::Described(description.text()));
        stats.types_described += 1;
        if llm.client.stats().chat_hits > hits {
            stats.types_described_cached += 1;
        }
        tracing::debug!(
            fqn = %symbol.fqn,
            done = done + 1,
            total = order.len(),
            elapsed_ms = started.elapsed().as_millis() as u64,
            "type described"
        );
    }
    tracing::info!(
        types = order.len(),
        described = stats.types_described,
        cached = stats.types_described_cached,
        elapsed_ms = started.elapsed().as_millis() as u64,
        "type descriptions done"
    );
    if stats.types_invalid + stats.types_failed + stats.types_incomplete > 0 {
        tracing::warn!(
            types = order.len(),
            described = stats.types_described,
            invalid = stats.types_invalid,
            failed = stats.types_failed,
            incomplete = stats.types_incomplete,
            waiting_for_members = held_back,
            skipped = stats.types_skipped,
            blocking_tickets = key_list(&blocking),
            "some types got no description this run; they are asked again next run"
        );
    }
    Ok(())
}

/// How a type's prompt lists its `child`, as kind and name: `method`
/// `find(Long)`, `constructor` `UserService(Repo)` (a record's compact
/// constructor, which has no parameter list, as `compact constructor`
/// `Point`), a nested type by its kind and simple name.
fn child_name(parent: &Symbol, child: &Symbol) -> (String, String) {
    let kind = child.kind.as_str().to_string();
    let member = child.fqn.rsplit_once('#').map(|(_, member)| member);
    match (child.kind, member) {
        (SymbolKind::Constructor, Some(_)) if !child.signature.contains('(') => {
            ("compact constructor".to_string(), parent.name.clone())
        }
        (SymbolKind::Constructor, Some(member)) => (
            kind,
            format!("{}{}", parent.name, member.trim_start_matches("<init>")),
        ),
        (_, Some(member)) => (kind, member.to_string()),
        (_, None) => (kind, child.name.clone()),
    }
}

/// Every member's tickets with what the index `tickets` table knows about
/// them, by symbol id. A ticket whose summary is missing because it was
/// invalid (`invalid_summaries`) gets its Jira title instead; a ticket not in
/// the table is [`TicketState::Unknown`] or, per `unknown_tickets`,
/// unavailable.
async fn member_tickets(
    conn: &Connection,
    invalid_summaries: &HashSet<String>,
    unknown_tickets: UnknownTickets,
) -> Result<HashMap<i64, Vec<MemberTicket>>> {
    let mut rows = conn
        .query(
            "SELECT st.symbol_id, st.ticket_key, st.first_date, st.last_date,
                    t.unavailable, t.issue_type, t.llm_summary, t.llm_purpose, t.summary
             FROM symbol_tickets st LEFT JOIN tickets t ON t.key = st.ticket_key",
            (),
        )
        .await
        .context("reading member tickets")?;
    let mut tickets: HashMap<i64, Vec<MemberTicket>> = HashMap::new();
    while let Some(row) = rows.next().await.context("reading member ticket row")? {
        let key: String = row.get(1).context("reading symbol_tickets.ticket_key")?;
        let unavailable: Option<i64> = row.get(4).context("reading tickets.unavailable")?;
        let issue_type: Option<String> = row.get(5).context("reading tickets.issue_type")?;
        let state = match (unavailable, issue_type) {
            (None, _) if unknown_tickets == UnknownTickets::Pending => TicketState::Unknown,
            (Some(0), Some(issue_type)) => {
                let summary: Option<String> = row.get(6).context("reading tickets.llm_summary")?;
                let title: Option<String> = row.get(8).context("reading tickets.summary")?;
                let summary = match (summary, title) {
                    (Some(summary), _) => Summary::Model {
                        summary,
                        purpose: row.get(7).context("reading tickets.llm_purpose")?,
                    },
                    (None, Some(title)) if invalid_summaries.contains(&key) => {
                        Summary::Title(title)
                    }
                    (None, _) => Summary::Missing,
                };
                TicketState::Available {
                    issue_type,
                    summary,
                }
            }
            _ => TicketState::Unavailable,
        };
        tickets
            .entry(row.get(0).context("reading symbol_tickets.symbol_id")?)
            .or_default()
            .push(MemberTicket {
                key,
                first_date: row.get(2).context("reading symbol_tickets.first_date")?,
                last_date: row.get(3).context("reading symbol_tickets.last_date")?,
                state,
            });
    }
    Ok(tickets)
}

/// Every member's commits, by symbol id.
async fn member_commits(conn: &Connection) -> Result<HashMap<i64, Vec<MemberCommit>>> {
    let mut rows = conn
        .query("SELECT symbol_id, date, subject FROM symbol_commits", ())
        .await
        .context("reading member commits")?;
    let mut commits: HashMap<i64, Vec<MemberCommit>> = HashMap::new();
    while let Some(row) = rows.next().await.context("reading member commit row")? {
        commits
            .entry(row.get(0).context("reading symbol_commits.symbol_id")?)
            .or_default()
            .push(MemberCommit {
                date: row.get(1).context("reading symbol_commits.date")?,
                subject: row.get(2).context("reading symbol_commits.subject")?,
            });
    }
    Ok(commits)
}

/// Store one symbol's description (whitespace collapsed) on its `symbols`
/// row.
async fn write_description(conn: &Connection, id: i64, description: &Description) -> Result<()> {
    conn.execute(
        "UPDATE symbols SET description = ?2 WHERE id = ?1",
        params![id, description.text()],
    )
    .await
    .with_context(|| format!("writing the description of symbol {id}"))?;
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
    use crate::config::{DEFAULT_TICKET_REGEX, OllamaConfig};
    use crate::llm::LlmClient;
    use crate::llm::MAX_CONSECUTIVE_INVALID;
    use crate::llm::fake::{FakeBackend, ModelCheck};
    use crate::store::IndexReader;
    use crate::test_support::{commit, init_repo};
    use crate::tickets::fake::{self, Answer, FakeSource};
    use libsql::params;

    fn ticket_regex() -> Regex {
        Regex::new(DEFAULT_TICKET_REGEX).unwrap()
    }

    /// `jira` as the run's Jira mode; `None` is `--offline`.
    fn jira_mode(jira: Option<&TicketFetch>) -> JiraMode {
        jira.map_or(JiraMode::Offline, |jira| JiraMode::Fetch(jira.clone()))
    }

    async fn build(repo: &Path, data_dir: &Path) -> Result<IndexStats> {
        let store = Store::open(data_dir).await.unwrap();
        build_index(
            &store,
            repo,
            None,
            &ticket_regex(),
            &JiraMode::Disabled,
            None,
        )
        .await
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
            blocks: Vec::new(),
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
            &JiraMode::Disabled,
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
        build_index(
            &store,
            repo,
            None,
            &ticket_regex(),
            &JiraMode::Disabled,
            None,
        )
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

        build_index(
            &store,
            repo.path(),
            None,
            &ticket_regex(),
            &JiraMode::Disabled,
            None,
        )
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
            &JiraMode::Disabled,
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
        build_index(&store, repo, None, &ticket_regex(), &jira_mode(jira), None).await
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
    async fn permanent_fetch_failures_are_cached_as_unavailable() {
        let repo = repo_with_commits(&["GRLD-1 add", "GRLD-2 change", "GRLD-3 fix", "GRLD-4 x"]);
        let data = tempfile::tempdir().unwrap();
        let source = FakeSource::new();
        source.script("GRLD-1", &[Answer::Status(400)]);
        source.script("GRLD-2", &[Answer::Parse]);
        source.script("GRLD-3", &[Answer::Status(500)]);
        source.script("GRLD-4", &[Answer::Status(408)]);
        let jira = fake::instant(source.clone(), 4);

        let first = index_with(repo.path(), data.path(), Some(&jira))
            .await
            .unwrap();
        assert_eq!(
            first.tickets_unavailable, 2,
            "400 and the unparseable issue"
        );
        assert_eq!(first.tickets_failed, 2, "500 and 408 are transient");
        assert_ticket_buckets(&first);
        assert_eq!(ticket_cache_keys(data.path()).await, ["GRLD-1", "GRLD-2"]);
        let reader = IndexReader::open(data.path()).await.unwrap();
        let mut rows = reader
            .connection()
            .query(
                "SELECT key FROM tickets WHERE unavailable = 1 ORDER BY key",
                (),
            )
            .await
            .unwrap();
        let mut unavailable = Vec::new();
        while let Some(row) = rows.next().await.unwrap() {
            unavailable.push(row.get::<String>(0).unwrap());
        }
        assert_eq!(unavailable, ["GRLD-1", "GRLD-2"]);

        let second = index_with(repo.path(), data.path(), Some(&jira))
            .await
            .unwrap();
        assert_eq!(second.ticket_hits, 2);
        assert_eq!(second.tickets_failed, 2);
        assert_eq!(source.calls_for("GRLD-1"), 1, "never asked again");
        assert_eq!(source.calls_for("GRLD-2"), 1);
        assert_eq!(source.calls_for("GRLD-3"), 2);
        assert_eq!(source.calls_for("GRLD-4"), 2);
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
        source.script_auth(&[Answer::Unauthorized]);
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
        source.script_auth(&[Answer::Transport]);
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
    async fn scope_mismatch_on_the_auth_check_still_fetches() {
        let repo = repo_with_commits(&["GRLD-1 add"]);
        let data = tempfile::tempdir().unwrap();
        let source = FakeSource::new();
        source.script_auth(&[Answer::ScopeMismatch]);
        source.script("GRLD-1", &[Answer::Ok(fake::ticket("GRLD-1"))]);

        let stats = index_with(
            repo.path(),
            data.path(),
            Some(&fake::instant(source.clone(), 4)),
        )
        .await
        .expect("a scope-limited auth check is inconclusive");

        assert_eq!(stats.tickets_fetched, 1);
        assert_eq!(stats.jira_requests, 2);
        assert_eq!(source.auth_checks(), 1);
        assert_eq!(ticket_cache_keys(data.path()).await, vec!["GRLD-1"]);
    }

    #[tokio::test]
    async fn scope_mismatch_on_a_fetch_fails_the_run_and_caches_nothing() {
        let repo = repo_with_commits(&["GRLD-1 add"]);
        let data = tempfile::tempdir().unwrap();
        let source = FakeSource::new();
        source.script_auth(&[Answer::ScopeMismatch]);
        source.script("GRLD-1", &[Answer::ScopeMismatch]);

        let err = index_with(repo.path(), data.path(), Some(&fake::instant(source, 4)))
            .await
            .expect_err("a fetch 401 is fatal");

        assert!(format!("{err:#}").contains("read:jira-work"), "{err:#}");
        assert!(ticket_cache_keys(data.path()).await.is_empty());
    }

    #[tokio::test]
    async fn rate_limited_auth_check_is_retried_before_fetching() {
        let repo = repo_with_commits(&["GRLD-1 add"]);
        let data = tempfile::tempdir().unwrap();
        let source = FakeSource::new();
        source.script_auth(&[
            Answer::RateLimited(None),
            Answer::Ok(fake::ticket("GRLD-1")),
        ]);
        source.script("GRLD-1", &[Answer::Ok(fake::ticket("GRLD-1"))]);

        let stats = index_with(
            repo.path(),
            data.path(),
            Some(&fake::instant(source.clone(), 4)),
        )
        .await
        .unwrap();

        assert_eq!(source.auth_checks(), 2);
        assert_eq!(stats.tickets_fetched, 1);
        assert_eq!(stats.jira_requests, 3);

        let data = tempfile::tempdir().unwrap();
        let source = FakeSource::new();
        source.script_auth(&[Answer::RateLimited(None)]);
        let stats = index_with(
            repo.path(),
            data.path(),
            Some(&fake::instant(source.clone(), 4)),
        )
        .await
        .expect("a 429 that outlasts the retries skips fetching");

        let checks = 1 + crate::tickets::MAX_RATE_LIMIT_RETRIES as usize;
        assert_eq!(source.auth_checks(), checks);
        assert_eq!(stats.jira_requests, checks);
        assert_eq!(stats.tickets_not_fetched, 1);
        assert_eq!(source.calls(), 0);
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

    const BRIEF: &str = r#"{"summary": "Adds the service.", "purpose": "Users need it."}"#;
    const DESCRIBED: &str = r#"{"description": "Returns the value."}"#;
    const TYPE_DESCRIBED: &str = r#"{"description": "Serves the values."}"#;
    const BLANK: &str = r#"{"summary": " ", "purpose": "Users need it."}"#;

    /// A summarizer over `backend`; the summary tests' describe stage gets
    /// [`DESCRIBED`] for every member.
    fn summarizer(store: &Store, backend: &Arc<FakeBackend>, cache_only: bool) -> Summarizer {
        backend.always("Description", DESCRIBED);
        backend.always("TypeDescription", TYPE_DESCRIBED);
        let config = OllamaConfig {
            url: "http://localhost:11434".to_string(),
            chat_model: "chat".to_string(),
            embedding_model: "embed".to_string(),
            reasoning_effort: "none".to_string(),
            temperature: 0.0,
        };
        let client = LlmClient::new(backend.clone(), store.connect_cache().unwrap(), &config);
        Summarizer::new(client, cache_only)
    }

    async fn summarise_with(
        repo: &Path,
        data: &Path,
        jira: Option<&TicketFetch>,
        backend: &Arc<FakeBackend>,
        cache_only: bool,
    ) -> IndexStats {
        let store = Store::open(data).await.unwrap();
        let llm = summarizer(&store, backend, cache_only);
        let stats = build_index(
            &store,
            repo,
            None,
            &ticket_regex(),
            &jira_mode(jira),
            Some(&llm),
        )
        .await
        .unwrap();
        assert_summary_buckets(&stats);
        stats
    }

    fn assert_summary_buckets(stats: &IndexStats) {
        assert_eq!(
            stats.summaries
                + stats.summaries_invalid
                + stats.summaries_failed
                + stats.summaries_skipped,
            stats.summary_tickets,
            "{stats:?}"
        );
    }

    /// A Jira fake that knows `available` keys and answers 404 for the rest.
    fn jira_with(available: &[Ticket]) -> TicketFetch {
        let source = FakeSource::new();
        for ticket in available {
            source.script(&ticket.key, &[Answer::Ok(ticket.clone())]);
        }
        fake::instant(source, 4)
    }

    /// `(key, llm_summary, llm_purpose)` for every row of the index `tickets`.
    async fn indexed_summaries(data: &Path) -> Vec<(String, Option<String>, Option<String>)> {
        let reader = IndexReader::open(data).await.unwrap();
        let mut rows = reader
            .connection()
            .query(
                "SELECT key, llm_summary, llm_purpose FROM tickets ORDER BY key",
                (),
            )
            .await
            .unwrap();
        let mut out = Vec::new();
        while let Some(row) = rows.next().await.unwrap() {
            out.push((
                row.get::<String>(0).unwrap(),
                row.get::<Option<String>>(1).unwrap(),
                row.get::<Option<String>>(2).unwrap(),
            ));
        }
        out
    }

    fn summarised(
        key: &str,
        summary: &str,
        purpose: &str,
    ) -> (String, Option<String>, Option<String>) {
        (
            key.to_string(),
            Some(summary.to_string()),
            Some(purpose.to_string()),
        )
    }

    fn unsummarised(key: &str) -> (String, Option<String>, Option<String>) {
        (key.to_string(), None, None)
    }

    #[tokio::test]
    async fn tickets_get_summaries_and_a_rerun_makes_no_chat_calls() {
        let repo = repo_with_commits(&["GRLD-1 add", "GRLD-2 change", "GRLD-3 change"]);
        let data = tempfile::tempdir().unwrap();
        let child = Ticket {
            parent_key: Some("GRLD-1".to_string()),
            ..fake::ticket("GRLD-3")
        };
        let jira = jira_with(&[fake::ticket("GRLD-1"), child]);
        let backend = FakeBackend::new();
        backend.reply(&[
            BRIEF,
            r#"{"summary": "Changes the service.", "purpose": "It was slow."}"#,
        ]);

        let cold = summarise_with(repo.path(), data.path(), Some(&jira), &backend, false).await;

        assert_eq!(cold.summary_tickets, 2, "GRLD-2 is unavailable");
        assert_eq!(cold.summaries, 2);
        assert_eq!(cold.summary_llm.chat_calls, 2);
        assert_eq!(cold.summary_llm.chat_hits, 0);
        let chats = backend.chats();
        let prompt = &chats[1].messages[0].content;
        assert!(prompt.contains("Key: GRLD-3\n"), "{prompt}");
        assert!(!prompt.contains("GRLD-1"), "{prompt}");
        assert!(prompt.contains("Description of GRLD-3"), "{prompt}");
        let expected = vec![
            summarised("GRLD-1", "Adds the service.", "Users need it."),
            unsummarised("GRLD-2"),
            summarised("GRLD-3", "Changes the service.", "It was slow."),
        ];
        assert_eq!(indexed_summaries(data.path()).await, expected);

        let quiet = FakeBackend::new();
        let warm = summarise_with(repo.path(), data.path(), Some(&jira), &quiet, false).await;

        assert_eq!(warm.summary_llm.chat_calls, 0, "a rerun makes no LLM calls");
        assert_eq!(warm.summary_llm.chat_hits, 2);
        assert_eq!(warm.summaries, 2);
        assert_eq!(warm.jira_requests, 0);
        assert!(quiet.chats().is_empty());
        assert_eq!(indexed_summaries(data.path()).await, expected);

        let reader = IndexReader::open(data.path()).await.unwrap();
        let output = crate::show::render(reader.connection(), "com.acme.Service#value()")
            .await
            .unwrap();
        assert!(
            output.contains(
                ") [Story] Summary of GRLD-1
      summary: Adds the service.
      purpose: Users need it.
"
            ),
            "{output}"
        );
        assert!(output.contains(") (unavailable)\n"), "{output}");
    }

    #[tokio::test]
    async fn without_a_summarizer_tickets_get_no_summary() {
        let repo = repo_with_commits(&["GRLD-1 add"]);
        let data = tempfile::tempdir().unwrap();
        let jira = jira_with(&[fake::ticket("GRLD-1")]);

        let stats = index_with(repo.path(), data.path(), Some(&jira))
            .await
            .unwrap();

        assert_eq!(stats.summary_tickets, 1);
        assert_eq!(stats.summaries_skipped, 1);
        assert_eq!(stats.llm, LlmStats::default());
        assert_summary_buckets(&stats);
        assert_eq!(
            indexed_summaries(data.path()).await,
            vec![unsummarised("GRLD-1")]
        );
    }

    #[tokio::test]
    async fn invalid_summary_is_skipped_and_retried_next_run() {
        let repo = repo_with_commits(&["GRLD-1 add", "GRLD-2 change"]);
        let data = tempfile::tempdir().unwrap();
        let jira = jira_with(&[fake::ticket("GRLD-1"), fake::ticket("GRLD-2")]);
        let backend = FakeBackend::new();
        backend.reply(&[
            BLANK,
            r#"{"summary": "x", "purpose": "y", "extra": 1}"#,
            BRIEF,
        ]);

        let first = summarise_with(repo.path(), data.path(), Some(&jira), &backend, false).await;

        assert_eq!(first.summaries_invalid, 1);
        assert_eq!(first.summaries, 1);
        assert_eq!(first.summary_llm.chat_calls, 3);
        assert_eq!(first.summary_llm.chat_retries, 1);
        assert_eq!(
            indexed_summaries(data.path()).await,
            vec![
                unsummarised("GRLD-1"),
                summarised("GRLD-2", "Adds the service.", "Users need it."),
            ]
        );

        let backend = FakeBackend::new();
        backend.reply(&[BRIEF]);
        let second = summarise_with(repo.path(), data.path(), Some(&jira), &backend, false).await;

        assert_eq!(second.summaries, 2);
        assert_eq!(
            second.summary_llm.chat_calls, 1,
            "only the invalid ticket is asked again"
        );
        assert_eq!(second.summary_llm.chat_hits, 1);
    }

    #[tokio::test]
    async fn chat_failure_trips_the_breaker_and_keeps_cached_summaries() {
        let repo = repo_with_commits(&["GRLD-1 add", "GRLD-2 x", "GRLD-3 y", "GRLD-4 z"]);
        let data = tempfile::tempdir().unwrap();
        let jira = jira_with(&["GRLD-1", "GRLD-2", "GRLD-3", "GRLD-4"].map(fake::ticket));
        let backend = FakeBackend::new();
        backend.reply(&[BLANK, BLANK, BRIEF, BRIEF, BLANK, BLANK]);
        let first = summarise_with(repo.path(), data.path(), Some(&jira), &backend, false).await;
        assert_eq!(first.summaries, 2, "GRLD-2 and GRLD-3 are cached");

        let down = FakeBackend::new();
        let stats = summarise_with(repo.path(), data.path(), Some(&jira), &down, false).await;

        assert_eq!(
            down.chats().len(),
            1,
            "no chat call after the first failure"
        );
        assert_eq!(stats.summaries_failed, 1);
        assert_eq!(stats.summaries, 2, "cached summaries are still used");
        assert_eq!(stats.summaries_skipped, 1, "GRLD-4 is not cached");
        assert_eq!(stats.summary_llm.chat_calls, 1);
        assert_eq!(stats.summary_llm.chat_hits, 2);
        assert_eq!(
            indexed_summaries(data.path()).await,
            vec![
                unsummarised("GRLD-1"),
                summarised("GRLD-2", "Adds the service.", "Users need it."),
                summarised("GRLD-3", "Adds the service.", "Users need it."),
                unsummarised("GRLD-4"),
            ]
        );
    }

    #[tokio::test]
    async fn cache_only_run_uses_cached_summaries_and_makes_no_chat_calls() {
        let repo = repo_with_commits(&["GRLD-1 add", "GRLD-2 change"]);
        let data = tempfile::tempdir().unwrap();
        let jira = jira_with(&[fake::ticket("GRLD-1"), fake::ticket("GRLD-2")]);
        let backend = FakeBackend::new();
        backend.reply(&[BRIEF, BLANK, BLANK]);
        summarise_with(repo.path(), data.path(), Some(&jira), &backend, false).await;

        let quiet = FakeBackend::new();
        let stats = summarise_with(repo.path(), data.path(), Some(&jira), &quiet, true).await;

        assert!(quiet.chats().is_empty());
        assert_eq!(stats.summary_llm.chat_calls, 0);
        assert_eq!(stats.summaries, 1);
        assert_eq!(stats.summaries_skipped, 1);
        assert_eq!(stats.summaries_failed + stats.summaries_invalid, 0);
        assert_eq!(
            indexed_summaries(data.path()).await,
            vec![
                summarised("GRLD-1", "Adds the service.", "Users need it."),
                unsummarised("GRLD-2"),
            ]
        );
    }

    #[tokio::test]
    async fn missing_chat_model_fails_the_run_unless_no_llm() {
        let repo = repo_with_commits(&["GRLD-1 add"]);
        let data = tempfile::tempdir().unwrap();
        let jira = jira_with(&[fake::ticket("GRLD-1")]);
        let backend = FakeBackend::new();
        backend.model(ModelCheck::Missing);
        let store = Store::open(data.path()).await.unwrap();
        let llm = summarizer(&store, &backend, false);

        let err = build_index(
            &store,
            repo.path(),
            None,
            &ticket_regex(),
            &JiraMode::Fetch(jira.clone()),
            Some(&llm),
        )
        .await
        .unwrap_err();

        assert!(format!("{err:#}").contains("does not exist"), "{err:#}");
        assert!(backend.chats().is_empty());
        assert!(!store.index_path().exists(), "no index was written");

        let stats = summarise_with(repo.path(), data.path(), Some(&jira), &backend, true).await;
        assert_eq!(stats.summaries_skipped, 1);
        assert_eq!(backend.model_checks().len(), 1, "--no-llm skips the check");
    }

    #[tokio::test]
    async fn unreachable_model_server_runs_cache_only() {
        let repo = repo_with_commits(&["GRLD-1 add"]);
        let data = tempfile::tempdir().unwrap();
        let jira = jira_with(&[fake::ticket("GRLD-1")]);
        let backend = FakeBackend::new();
        backend.model(ModelCheck::Fails);

        let stats = summarise_with(repo.path(), data.path(), Some(&jira), &backend, false).await;

        assert_eq!(stats.summaries_skipped, 1);
        assert_eq!(stats.summaries_failed, 0);
        assert!(backend.chats().is_empty());
    }

    #[tokio::test]
    async fn consecutive_invalid_summaries_stop_the_chat_calls() {
        let keys = ["GRLD-1", "GRLD-2", "GRLD-3", "GRLD-4", "GRLD-5", "GRLD-6"];
        let messages = keys.map(|key| format!("{key} change"));
        let repo = repo_with_commits(&messages.each_ref().map(String::as_str));
        let data = tempfile::tempdir().unwrap();
        let jira = jira_with(&keys.map(fake::ticket));
        let backend = FakeBackend::new();
        backend.reply(&[BLANK; 12]);

        let stats = summarise_with(repo.path(), data.path(), Some(&jira), &backend, false).await;

        assert_eq!(stats.summaries_invalid, MAX_CONSECUTIVE_INVALID);
        assert_eq!(stats.summaries_skipped, 1);
        assert_eq!(stats.summary_llm.chat_calls, 2 * MAX_CONSECUTIVE_INVALID);
    }

    #[tokio::test]
    async fn empty_purpose_is_stored_as_null_and_not_shown() {
        let repo = repo_with_commits(&["GRLD-1 add"]);
        let data = tempfile::tempdir().unwrap();
        let jira = jira_with(&[fake::ticket("GRLD-1")]);
        let backend = FakeBackend::new();
        backend.reply(&[r#"{"summary": "Adds\nthe service.", "purpose": ""}"#]);

        let stats = summarise_with(repo.path(), data.path(), Some(&jira), &backend, false).await;

        assert_eq!(stats.summaries, 1);
        assert_eq!(
            indexed_summaries(data.path()).await,
            vec![(
                "GRLD-1".to_string(),
                Some("Adds the service.".to_string()),
                None
            )]
        );
        let reader = IndexReader::open(data.path()).await.unwrap();
        let output = crate::show::render(reader.connection(), "com.acme.Service#value()")
            .await
            .unwrap();
        assert!(
            output.contains("      summary: Adds the service.\n"),
            "{output}"
        );
        assert!(!output.contains("purpose:"), "{output}");
    }

    const DESCRIPTION: &str =
        r#"{"description": "Returns the\nconfigured value.  Callers need the value."}"#;

    async fn describe_with(
        repo: &Path,
        data: &Path,
        path: Option<&Path>,
        jira: Option<&TicketFetch>,
        backend: &Arc<FakeBackend>,
        cache_only: bool,
    ) -> IndexStats {
        describe_in(repo, data, path, &jira_mode(jira), backend, cache_only).await
    }

    async fn describe_in(
        repo: &Path,
        data: &Path,
        path: Option<&Path>,
        jira: &JiraMode,
        backend: &Arc<FakeBackend>,
        cache_only: bool,
    ) -> IndexStats {
        backend.always("TypeDescription", TYPE_DESCRIBED);
        run_llm_stages(
            repo,
            data,
            path,
            jira,
            backend,
            cache_only,
            DescribeConfig::default(),
        )
        .await
    }

    /// An index run with an LLM over `backend` (no standing replies added)
    /// and `describe` as the prompt settings.
    async fn run_llm_stages(
        repo: &Path,
        data: &Path,
        path: Option<&Path>,
        jira: &JiraMode,
        backend: &Arc<FakeBackend>,
        cache_only: bool,
        describe: DescribeConfig,
    ) -> IndexStats {
        let store = Store::open(data).await.unwrap();
        let config = OllamaConfig {
            url: "http://localhost:11434".to_string(),
            chat_model: "chat".to_string(),
            embedding_model: "embed".to_string(),
            reasoning_effort: "none".to_string(),
            temperature: 0.0,
        };
        let client = LlmClient::new(backend.clone(), store.connect_cache().unwrap(), &config);
        let llm = Summarizer::new(client, cache_only).with_describe(describe);
        let stats = build_index(&store, repo, path, &ticket_regex(), jira, Some(&llm))
            .await
            .unwrap();
        assert_describe_buckets(&stats);
        stats
    }

    fn assert_describe_buckets(stats: &IndexStats) {
        assert_eq!(
            stats.described
                + stats.describe_invalid
                + stats.describe_failed
                + stats.describe_incomplete
                + stats.describe_skipped,
            stats.describe_members,
            "{stats:?}"
        );
        assert_eq!(
            stats.types_described
                + stats.types_invalid
                + stats.types_failed
                + stats.types_incomplete
                + stats.types_skipped,
            stats.type_symbols,
            "{stats:?}"
        );
    }

    /// The description of `fqn` in the committed index.
    async fn description_of(data: &Path, fqn: &str) -> Option<String> {
        let reader = IndexReader::open(data).await.unwrap();
        text_of(reader.connection(), fqn, "description").await
    }

    /// The prompts of the describe requests `backend` received.
    fn describe_prompts(backend: &FakeBackend) -> Vec<String> {
        backend
            .chats()
            .into_iter()
            .filter(|chat| chat.schema_name == "Description")
            .map(|chat| chat.messages[0].content.clone())
            .collect()
    }

    #[tokio::test]
    async fn members_get_descriptions_from_code_tickets_and_commits_and_a_rerun_makes_no_chat_calls()
     {
        let repo = repo_with_commits(&["GRLD-1 add", "GRLD-2 change"]);
        let data = tempfile::tempdir().unwrap();
        let jira = jira_with(&[fake::ticket("GRLD-1")]);
        let backend = FakeBackend::new();
        backend.reply(&[BRIEF, DESCRIPTION]);

        let cold =
            describe_with(repo.path(), data.path(), None, Some(&jira), &backend, false).await;

        assert_eq!(cold.describe_members, 1);
        assert_eq!(cold.described, 1);
        assert_eq!(cold.described_cached, 0);
        assert_eq!(cold.describe_llm.chat_calls, 1);
        assert_eq!(cold.summary_llm.chat_calls, 1);
        assert_eq!(cold.type_llm.chat_calls, 1);
        assert_eq!(cold.llm.chat_calls, 3);
        let prompts = describe_prompts(&backend);
        let prompt = &prompts[0];
        assert!(
            prompt.ends_with(
                "\
<<<CONTEXT
Declared in: class com.acme.Service
Kind: method
Javadoc: (none)
Source:
public int value() {
    return 1;
}
Tickets (the work it was created for, then the most recent changes, newest first):
- Created for GRLD-1 (Story): Adds the service.
  Reason: Users need it.
Commits (most recent messages, newest first):
- GRLD-2 change
- GRLD-1 add
CONTEXT>>>
"
            ),
            "the source is dedented and GRLD-2 (unavailable) left out of the tickets: {prompt}"
        );
        assert_eq!(
            description_of(data.path(), "com.acme.Service#value()").await,
            Some("Returns the configured value. Callers need the value.".to_string())
        );

        let quiet = FakeBackend::new();
        let warm = describe_with(repo.path(), data.path(), None, Some(&jira), &quiet, false).await;

        assert_eq!(warm.llm.chat_calls, 0, "a rerun makes no LLM calls");
        assert_eq!(warm.described, 1);
        assert_eq!(warm.described_cached, 1);
        assert!(quiet.chats().is_empty());

        let reader = IndexReader::open(data.path()).await.unwrap();
        let output = crate::show::render(reader.connection(), "com.acme.Service")
            .await
            .unwrap();
        assert!(
            output.contains(
                "  com.acme.Service#value() [method]
    - file: src/main/java/com/acme/Service.java:4-6
    - signature: public int value()
    - description: Returns the configured value. Callers need the value.
"
            ),
            "{output}"
        );
    }

    #[tokio::test]
    async fn invalid_ticket_summary_falls_back_to_the_jira_title() {
        let repo = repo_with_commits(&["GRLD-1 add"]);
        let data = tempfile::tempdir().unwrap();
        let jira = jira_with(&[fake::ticket("GRLD-1")]);
        let backend = FakeBackend::new();
        backend.reply(&[BLANK, BLANK, DESCRIPTION]);

        let first =
            describe_with(repo.path(), data.path(), None, Some(&jira), &backend, false).await;

        assert_eq!(first.summaries_invalid, 1);
        assert_eq!(first.described, 1, "described from the Jira title");
        let prompt = &describe_prompts(&backend)[0];
        assert!(
            prompt.ends_with(
                "- Created for GRLD-1 (Story): Summary of GRLD-1\nCommits (most recent messages, newest first):\n- GRLD-1 add\nCONTEXT>>>\n"
            ),
            "{prompt}"
        );

        let backend = FakeBackend::new();
        backend.reply(&[BRIEF, DESCRIBED]);
        let second =
            describe_with(repo.path(), data.path(), None, Some(&jira), &backend, false).await;

        assert_eq!(second.described, 1);
        assert_eq!(
            second.described_cached, 0,
            "a valid summary changes the prompt"
        );
        assert_eq!(
            description_of(data.path(), "com.acme.Service#value()").await,
            Some("Returns the value.".to_string())
        );
        let reader = IndexReader::open(data.path()).await.unwrap();
        let output = crate::show::render(reader.connection(), "com.acme.Service#value()")
            .await
            .unwrap();
        assert!(
            output.contains("- description: Returns the value.\n"),
            "{output}"
        );
    }

    #[tokio::test]
    async fn tripped_summaries_serve_cached_members_and_hold_back_the_rest() {
        let repo = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        for name in ["A", "B", "C"] {
            write(
                repo.path(),
                &format!("src/main/java/com/acme/{name}.java"),
                &java_class(name, 1),
            );
        }
        commit(repo.path(), "GRLD-1 add", "2024-01-01T00:00:00+00:00");
        let data = tempfile::tempdir().unwrap();
        let jira = jira_with(&[fake::ticket("GRLD-1"), fake::ticket("GRLD-3")]);
        let backend = FakeBackend::new();
        backend.always("TicketSummary", BRIEF);
        backend.always("Description", DESCRIPTION);
        let first =
            describe_with(repo.path(), data.path(), None, Some(&jira), &backend, false).await;
        assert_eq!(first.described, 3);

        write(
            repo.path(),
            "src/main/java/com/acme/B.java",
            &java_class("B", 2),
        );
        commit(repo.path(), "GRLD-3 change B", "2024-01-02T00:00:00+00:00");
        write(
            repo.path(),
            "src/main/java/com/acme/C.java",
            &java_class("C", 2),
        );
        commit(repo.path(), "Tidy C", "2024-01-03T00:00:00+00:00");
        let down = FakeBackend::new();
        let stats = describe_with(repo.path(), data.path(), None, Some(&jira), &down, false).await;

        assert_eq!(stats.summaries_failed, 1, "GRLD-3 trips the breaker");
        assert_eq!(down.chats().len(), 1, "no chat call after the failure");
        assert_eq!(stats.described, 1);
        assert_eq!(stats.described_cached, 1, "A is unchanged");
        assert_eq!(
            stats.describe_incomplete, 1,
            "B chose GRLD-3, which has no summary"
        );
        assert_eq!(stats.describe_skipped, 1, "C changed and is not cached");
        assert_eq!(stats.describe_llm.chat_calls, 0);
        assert_eq!(
            description_of(data.path(), "com.acme.A#value()").await,
            Some("Returns the configured value. Callers need the value.".to_string())
        );
        assert_eq!(
            description_of(data.path(), "com.acme.B#value()").await,
            None
        );
        assert_eq!(
            description_of(data.path(), "com.acme.C#value()").await,
            None
        );
    }

    #[tokio::test]
    async fn invalid_streak_does_not_carry_into_the_describe_stage() {
        let repo = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        write(
            repo.path(),
            "src/main/java/com/acme/Service.java",
            "package com.acme;\nclass Service {\n    int a() { return 1; }\n    int b() { return 2; }\n}\n",
        );
        commit(
            repo.path(),
            "GRLD-1 GRLD-2 GRLD-3 GRLD-4 add",
            "2024-01-01T00:00:00+00:00",
        );
        let data = tempfile::tempdir().unwrap();
        let keys = ["GRLD-1", "GRLD-2", "GRLD-3", "GRLD-4"];
        let jira = jira_with(&keys.map(fake::ticket));
        let backend = FakeBackend::new();
        let bad = r#"{"description": " "}"#;
        backend.reply(&[
            BLANK,
            BLANK,
            BLANK,
            BLANK,
            BLANK,
            BLANK,
            BLANK,
            BLANK,
            bad,
            bad,
            DESCRIPTION,
        ]);

        let stats =
            describe_with(repo.path(), data.path(), None, Some(&jira), &backend, false).await;

        assert_eq!(stats.summaries_invalid, MAX_CONSECUTIVE_INVALID - 1);
        assert_eq!(stats.describe_invalid, 1);
        assert_eq!(
            stats.described, 1,
            "the fifth invalid reply in a row is the stage's first, so b() is still asked"
        );
        assert_eq!(stats.describe_skipped, 0);
    }

    #[tokio::test]
    async fn without_jira_unfetched_tickets_count_as_unavailable() {
        let repo = repo_with_commits(&["GRLD-1 add", "Tidy up"]);
        let data = tempfile::tempdir().unwrap();
        let backend = FakeBackend::new();
        backend.reply(&[DESCRIPTION]);

        let stats = describe_in(
            repo.path(),
            data.path(),
            None,
            &JiraMode::Disabled,
            &backend,
            false,
        )
        .await;

        assert_eq!(stats.tickets_not_fetched, 1);
        assert_eq!(stats.describe_incomplete, 0);
        assert_eq!(stats.described, 1);
        let prompt = &describe_prompts(&backend)[0];
        assert!(
            prompt.contains(
                "Tickets: (none)\nCommits (most recent messages, newest first):\n- Tidy up\n- GRLD-1 add\nCONTEXT>>>"
            ),
            "{prompt}"
        );
    }

    #[tokio::test]
    async fn unfetched_ticket_leaves_the_member_incomplete() {
        let repo = repo_with_commits(&["GRLD-1 add"]);
        let data = tempfile::tempdir().unwrap();
        let backend = FakeBackend::new();

        let stats = describe_with(repo.path(), data.path(), None, None, &backend, false).await;

        assert_eq!(stats.tickets_not_fetched, 1);
        assert_eq!(stats.describe_incomplete, 1);
        assert!(backend.chats().is_empty());
    }

    #[tokio::test]
    async fn commit_subjects_without_an_available_ticket() {
        let repo = repo_with_commits(&["GRLD-2 change", "Tidy up", "Tidy up"]);
        let data = tempfile::tempdir().unwrap();
        let jira = jira_with(&[]);
        let backend = FakeBackend::new();
        backend.reply(&[DESCRIPTION]);

        let stats =
            describe_with(repo.path(), data.path(), None, Some(&jira), &backend, false).await;

        assert_eq!(stats.summary_tickets, 0);
        assert_eq!(stats.described, 1);
        let prompt = &describe_prompts(&backend)[0];
        assert!(
            prompt.contains(
                "Tickets: (none)\nCommits (most recent messages, newest first):\n- Tidy up\n- GRLD-2 change\nCONTEXT>>>"
            ),
            "{prompt}"
        );
    }

    #[tokio::test]
    async fn non_git_repo_members_are_described_from_code_alone() {
        let repo = tempfile::tempdir().unwrap();
        write(repo.path(), "src/main/java/com/acme/Service.java", &java(7));
        let data = tempfile::tempdir().unwrap();
        let backend = FakeBackend::new();
        backend.reply(&[DESCRIPTION]);

        let stats = describe_with(repo.path(), data.path(), None, None, &backend, false).await;

        assert_eq!(stats.described, 1);
        let prompt = &describe_prompts(&backend)[0];
        assert!(
            prompt.ends_with("Tickets: (none)\nCommits: (none)\nCONTEXT>>>\n"),
            "{prompt}"
        );
    }

    #[tokio::test]
    async fn prompt_does_not_depend_on_the_rest_of_a_path_run() {
        let repo = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        write(
            repo.path(),
            "src/main/java/com/acme/a/A.java",
            &java_class("A", 1).replace("package com.acme;", "package com.acme.a;"),
        );
        write(
            repo.path(),
            "src/main/java/com/acme/b/B.java",
            &java_class("B", 2).replace("package com.acme;", "package com.acme.b;"),
        );
        commit(repo.path(), "Add A and B", "2024-01-01T00:00:00+00:00");
        let data = tempfile::tempdir().unwrap();
        let backend = FakeBackend::new();
        backend.reply(&[DESCRIPTION, DESCRIPTION]);
        let full = describe_with(repo.path(), data.path(), None, None, &backend, false).await;
        assert_eq!(full.described, 2);

        let quiet = FakeBackend::new();
        let narrow = describe_with(
            repo.path(),
            data.path(),
            Some(Path::new("src/main/java/com/acme/a")),
            None,
            &quiet,
            false,
        )
        .await;

        assert_eq!(narrow.describe_members, 1);
        assert_eq!(narrow.described_cached, 1, "same prompt as in the full run");
        assert_eq!(narrow.types_described_cached, 1, "same type prompt too");
        assert!(quiet.chats().is_empty());
    }

    #[tokio::test]
    async fn without_a_summarizer_or_with_no_llm_members_are_skipped() {
        let repo = repo_with_commits(&["Add service"]);
        let data = tempfile::tempdir().unwrap();

        let stats = index_with(repo.path(), data.path(), None).await.unwrap();
        assert_eq!(stats.describe_members, 1);
        assert_eq!(stats.describe_skipped, 1);
        assert_describe_buckets(&stats);

        let backend = FakeBackend::new();
        let stats = describe_with(repo.path(), data.path(), None, None, &backend, true).await;
        assert_eq!(stats.describe_skipped, 1);
        assert!(backend.chats().is_empty());
        assert_eq!(
            description_of(data.path(), "com.acme.Service#value()").await,
            None
        );
    }

    #[tokio::test]
    async fn invalid_and_failed_descriptions_are_counted_and_the_breaker_holds() {
        let repo = tempfile::tempdir().unwrap();
        write(
            repo.path(),
            "src/main/java/com/acme/Service.java",
            "package com.acme;\nclass Service {\n    Service() {}\n    int a() { return 1; }\n    int b() { return 2; }\n}\n",
        );
        let data = tempfile::tempdir().unwrap();
        let backend = FakeBackend::new();
        backend.reply(&[
            r#"{"description": "This method does things."}"#,
            r#"{"description": " "}"#,
        ]);

        let stats = describe_with(repo.path(), data.path(), None, None, &backend, false).await;

        assert_eq!(stats.describe_members, 3);
        assert_eq!(stats.describe_invalid, 1, "the constructor");
        assert_eq!(stats.describe_failed, 1, "a(): no scripted reply");
        assert_eq!(stats.describe_skipped, 1, "b() after the breaker tripped");
        assert_eq!(stats.describe_llm.chat_calls, 3);
        assert_eq!(stats.describe_llm.chat_retries, 1);
        let prompt = &describe_prompts(&backend)[0];
        assert!(prompt.contains("one Java constructor"), "{prompt}");
    }

    const ORDERS: &str = "\
package com.acme;

/** Order book. */
@Service
public class Orders extends Base {
    private final Repo repo;

    /** Creates it. */
    public Orders(Repo repo) {
        this.repo = repo;
    }

    private int helper() { return 1; }

    public static class Line {
        private int amount;

        public int amount() { return amount; }

        enum Kind { A, B }
    }
}
";

    fn type_reply(text: &str) -> String {
        format!(r#"{{"description": "{text} Billing needs it."}}"#)
    }

    /// The prompts of the type requests `backend` received.
    fn type_prompts(backend: &FakeBackend) -> Vec<String> {
        backend
            .chats()
            .into_iter()
            .filter(|chat| chat.schema_name == "TypeDescription")
            .map(|chat| chat.messages[0].content.clone())
            .collect()
    }

    #[tokio::test]
    async fn types_are_described_bottom_up_from_their_members_and_a_rerun_makes_no_chat_calls() {
        let repo = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        write(repo.path(), "src/main/java/com/acme/Orders.java", ORDERS);
        commit(
            repo.path(),
            "GRLD-1 add orders",
            "2024-01-01T00:00:00+00:00",
        );
        let data = tempfile::tempdir().unwrap();
        let jira = jira_with(&[fake::ticket("GRLD-1")]);
        let backend = FakeBackend::new();
        backend.always("TicketSummary", BRIEF);
        backend.always("Description", DESCRIBED);
        let replies = [
            type_reply("Lists the kinds."),
            type_reply("Holds one order line."),
            type_reply("Keeps the order book."),
        ];
        backend.reply(&replies.iter().map(String::as_str).collect::<Vec<_>>());

        let cold = run_llm_stages(
            repo.path(),
            data.path(),
            None,
            &jira_mode(Some(&jira)),
            &backend,
            false,
            DescribeConfig::default(),
        )
        .await;

        assert_eq!(cold.type_symbols, 3);
        assert_eq!(cold.types_described, 3);
        assert_eq!(cold.type_llm.chat_calls, 3);
        let prompts = type_prompts(&backend);
        let kinds: Vec<&str> = prompts
            .iter()
            .map(|prompt| {
                let start = prompt.find("\nType: ").unwrap() + 7;
                &prompt[start..start + prompt[start..].find('\n').unwrap()]
            })
            .collect();
        assert_eq!(
            kinds,
            [
                "enum com.acme.Orders.Line.Kind",
                "class com.acme.Orders.Line",
                "class com.acme.Orders (Spring service)",
            ],
            "deepest first"
        );
        assert!(
            prompts[1].contains(
                "\
Nested in: class com.acme.Orders
Javadoc: (none)
Declaration (members and nested types are cut out and listed below):
public static class Line {
    private int amount;
}
Methods, constructors and nested types (with their descriptions):
- method amount(): Returns the value.
- enum Kind: Lists the kinds. Billing needs it.
"
            ),
            "{}",
            prompts[1]
        );
        assert!(
            prompts[2].ends_with(
                "\
<<<CONTEXT
Type: class com.acme.Orders (Spring service)
Javadoc:
Order book.
Declaration (members and nested types are cut out and listed below):
@Service
public class Orders extends Base {
    private final Repo repo;
}
Methods, constructors and nested types (with their descriptions):
- constructor Orders(Repo): Returns the value.
- method helper(): Returns the value.
- class Line: Holds one order line. Billing needs it.
Tickets (the work it was created for, then the most recent changes, newest first):
- Created for GRLD-1 (Story): Adds the service.
  Reason: Users need it.
Commits (most recent messages, newest first):
- GRLD-1 add orders
CONTEXT>>>
"
            ),
            "{}",
            prompts[2]
        );

        let quiet = FakeBackend::new();
        let warm = run_llm_stages(
            repo.path(),
            data.path(),
            None,
            &jira_mode(Some(&jira)),
            &quiet,
            false,
            DescribeConfig::default(),
        )
        .await;
        assert_eq!(warm.llm.chat_calls, 0, "a rerun makes no LLM calls");
        assert_eq!(warm.types_described_cached, 3);
        assert!(quiet.chats().is_empty());

        let reader = IndexReader::open(data.path()).await.unwrap();
        let output = crate::show::render(reader.connection(), "com.acme.Orders")
            .await
            .unwrap();
        assert!(
            output.starts_with(
                "\
com.acme.Orders [class] role=service
  - file: src/main/java/com/acme/Orders.java:4-22
  - signature: public class Orders extends Base
  - description: Keeps the order book. Billing needs it.
"
            ),
            "{output}"
        );
        assert!(
            output.contains(
                "
  com.acme.Orders.Line [class]
    - file: src/main/java/com/acme/Orders.java:15-21
    - signature: public static class Line
    - description: Holds one order line. Billing needs it.
"
            ),
            "{output}"
        );
        assert!(
            output.contains(
                "
    com.acme.Orders.Line.Kind [enum]
      - file: src/main/java/com/acme/Orders.java:20-20
      - signature: enum Kind
      - description: Lists the kinds. Billing needs it.
"
            ),
            "{output}"
        );
    }

    #[tokio::test]
    async fn type_lists_public_members_first_up_to_the_cap() {
        let repo = tempfile::tempdir().unwrap();
        write(repo.path(), "src/main/java/com/acme/Orders.java", ORDERS);
        let data = tempfile::tempdir().unwrap();
        let backend = FakeBackend::new();
        backend.always("Description", DESCRIBED);
        backend.always("TypeDescription", TYPE_DESCRIBED);
        let config = DescribeConfig {
            type_members: 1,
            ..DescribeConfig::default()
        };

        run_llm_stages(
            repo.path(),
            data.path(),
            None,
            &JiraMode::Disabled,
            &backend,
            false,
            config,
        )
        .await;

        let prompts = type_prompts(&backend);
        assert!(
            prompts[2].contains(
                "\
Methods, constructors and nested types (with their descriptions):
- constructor Orders(Repo): Returns the value.
- (2 more not listed)
Tickets: (none)
Commits: (none)
"
            ),
            "{}",
            prompts[2]
        );
        assert!(
            prompts[1].contains("- method amount(): Returns the value.\n- (1 more not listed)\n"),
            "{}",
            prompts[1]
        );
    }

    #[tokio::test]
    async fn invalid_member_is_listed_bare_and_a_missing_one_holds_its_types_back() {
        let repo = tempfile::tempdir().unwrap();
        write(repo.path(), "src/main/java/com/acme/Orders.java", ORDERS);
        let data = tempfile::tempdir().unwrap();
        let backend = FakeBackend::new();
        backend.always("TypeDescription", TYPE_DESCRIBED);
        let blank = r#"{"description": " "}"#;
        backend.reply(&[DESCRIBED, DESCRIBED, blank, blank]);

        let first = run_llm_stages(
            repo.path(),
            data.path(),
            None,
            &JiraMode::Disabled,
            &backend,
            false,
            DescribeConfig::default(),
        )
        .await;

        assert_eq!(first.describe_invalid, 1, "Line#amount() is invalid");
        assert_eq!(first.types_described, 3);
        let prompts = type_prompts(&backend);
        assert!(
            prompts[1].contains("- method amount(): (not described)\n"),
            "{}",
            prompts[1]
        );

        let quiet = FakeBackend::new();
        let cache_only = run_llm_stages(
            repo.path(),
            data.path(),
            None,
            &JiraMode::Disabled,
            &quiet,
            true,
            DescribeConfig::default(),
        )
        .await;

        assert_eq!(cache_only.describe_skipped, 1, "the invalid one is a miss");
        assert_eq!(cache_only.types_described, 1, "Kind lists no members");
        assert_eq!(
            cache_only.types_incomplete, 2,
            "Line waits for amount(), Orders for Line"
        );
        assert!(quiet.chats().is_empty());
        assert_eq!(
            description_of(data.path(), "com.acme.Orders.Line").await,
            None
        );
        assert_eq!(description_of(data.path(), "com.acme.Orders").await, None);
        assert_eq!(
            description_of(data.path(), "com.acme.Orders.Line.Kind").await,
            Some("Serves the values.".to_string())
        );
    }

    /// `future`'s output and the warnings it logged, as plain text.
    async fn with_warnings<T>(future: impl std::future::Future<Output = T>) -> (T, String) {
        use tracing::instrument::WithSubscriber;

        #[derive(Clone, Default)]
        struct Buffer(Arc<std::sync::Mutex<Vec<u8>>>);
        impl std::io::Write for Buffer {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let buffer = Buffer::default();
        let writer = buffer.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(move || writer.clone())
            .with_ansi(false)
            .with_max_level(tracing::Level::WARN)
            .finish();
        let output = future.with_subscriber(subscriber).await;
        let logs = String::from_utf8(buffer.0.lock().unwrap().clone()).unwrap();
        (output, logs)
    }

    /// The logged line containing `message`.
    fn log_line<'a>(logs: &'a str, message: &str) -> &'a str {
        logs.lines()
            .find(|line| line.contains(message))
            .unwrap_or_else(|| panic!("no {message:?} in {logs}"))
    }

    #[tokio::test]
    async fn only_the_types_own_chosen_ticket_holds_it_back() {
        let repo = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        let service = |limit: i64| {
            format!(
                "package com.acme;\nclass Service {{\n    int limit = {limit};\n\n    int value() {{ return 1; }}\n}}\n"
            )
        };
        write(
            repo.path(),
            "src/main/java/com/acme/Service.java",
            &service(1),
        );
        commit(repo.path(), "GRLD-1 add", "2024-01-01T00:00:00+00:00");
        write(
            repo.path(),
            "src/main/java/com/acme/Service.java",
            &service(2),
        );
        commit(repo.path(), "GRLD-2 limit", "2024-01-02T00:00:00+00:00");
        let data = tempfile::tempdir().unwrap();
        let source = FakeSource::new();
        source.script("GRLD-1", &[Answer::Ok(fake::ticket("GRLD-1"))]);
        source.script("GRLD-2", &[Answer::Status(503)]);
        let jira = fake::instant(source, 4);
        let backend = FakeBackend::new();
        backend.always("TicketSummary", BRIEF);
        backend.always("Description", DESCRIBED);
        backend.always("TypeDescription", TYPE_DESCRIBED);

        let (stats, logs) = with_warnings(run_llm_stages(
            repo.path(),
            data.path(),
            None,
            &jira_mode(Some(&jira)),
            &backend,
            false,
            DescribeConfig::default(),
        ))
        .await;

        assert_eq!(stats.tickets_failed, 1, "GRLD-2 stays unknown");
        assert_eq!(stats.described, 1, "value() only has GRLD-1");
        assert_eq!(stats.describe_incomplete, 0);
        assert_eq!(stats.types_incomplete, 1, "Service chose GRLD-2");
        assert_eq!(stats.type_llm.chat_calls, 0);
        let warning = log_line(&logs, "some types got no description");
        assert!(
            warning.contains("incomplete=1")
                && warning.contains("waiting_for_members=0")
                && warning.contains("blocking_tickets=\"GRLD-2\""),
            "{warning}"
        );
        assert!(!logs.contains("some methods got no description"), "{logs}");

        let fetched = jira_with(&[fake::ticket("GRLD-1"), fake::ticket("GRLD-2")]);
        let stats = run_llm_stages(
            repo.path(),
            data.path(),
            None,
            &jira_mode(Some(&fetched)),
            &backend,
            false,
            DescribeConfig::default(),
        )
        .await;
        assert_eq!(stats.types_described, 1, "described once GRLD-2 is in");
        assert!(
            type_prompts(&backend)[0].contains("- Changed for GRLD-2 (Story): Adds the service.\n"),
            "{}",
            type_prompts(&backend)[0]
        );
    }

    #[tokio::test]
    async fn interface_members_are_public_and_compact_constructors_are_named() {
        let repo = tempfile::tempdir().unwrap();
        write(
            repo.path(),
            "src/main/java/com/acme/Repo.java",
            "\
package com.acme;

public interface Repo {
    int MAX = 3;

    private void helper() {}

    User find(Long id);

    default int size() { return 0; }

    record Point(int x, int y) {
        public Point {
            if (x < 0) throw new IllegalArgumentException();
        }

        Point(int x) { this(x, 0); }
    }
}
",
        );
        let data = tempfile::tempdir().unwrap();
        let backend = FakeBackend::new();
        backend.always("Description", DESCRIBED);
        backend.always("TypeDescription", TYPE_DESCRIBED);
        let config = DescribeConfig {
            type_members: 3,
            ..DescribeConfig::default()
        };

        let stats = run_llm_stages(
            repo.path(),
            data.path(),
            None,
            &JiraMode::Disabled,
            &backend,
            false,
            config,
        )
        .await;

        assert_eq!(stats.types_described, 2);
        let prompts = type_prompts(&backend);
        assert!(
            prompts[0].contains(
                "\
Declaration (members and nested types are cut out and listed below):
record Point(int x, int y) {
}
Methods, constructors and nested types (with their descriptions):
- compact constructor Point: Returns the value.
- constructor Point(int): Returns the value.
"
            ),
            "{}",
            prompts[0]
        );
        assert!(
            prompts[1].contains(
                "\
public interface Repo {
    int MAX = 3;
}
Methods, constructors and nested types (with their descriptions):
- method find(Long): Returns the value.
- method size(): Returns the value.
- record Point: Serves the values.
- (1 more not listed)
"
            ),
            "the private helper() is the one left out: {}",
            prompts[1]
        );
    }

    #[tokio::test]
    async fn invalid_nested_type_is_listed_bare_in_its_outer_type() {
        let repo = tempfile::tempdir().unwrap();
        write(repo.path(), "src/main/java/com/acme/Orders.java", ORDERS);
        let data = tempfile::tempdir().unwrap();
        let backend = FakeBackend::new();
        backend.always("Description", DESCRIBED);
        let blank = r#"{"description": " "}"#;
        let kinds = type_reply("Lists the kinds.");
        let book = type_reply("Keeps the order book.");
        backend.reply(&[&kinds, blank, blank, &book]);

        let stats = run_llm_stages(
            repo.path(),
            data.path(),
            None,
            &JiraMode::Disabled,
            &backend,
            false,
            DescribeConfig::default(),
        )
        .await;

        assert_eq!(stats.types_described, 2);
        assert_eq!(stats.types_invalid, 1, "Line");
        assert_eq!(stats.type_llm.chat_retries, 1);
        let prompts = type_prompts(&backend);
        assert!(
            prompts[3].contains("- class Line: (not described)\n"),
            "{}",
            prompts[3]
        );
        assert_eq!(
            description_of(data.path(), "com.acme.Orders").await,
            Some("Keeps the order book. Billing needs it.".to_string())
        );
        assert_eq!(
            description_of(data.path(), "com.acme.Orders.Line").await,
            None
        );
    }

    #[tokio::test]
    async fn failed_type_call_trips_the_breaker_and_holds_the_outer_types_back() {
        let repo = tempfile::tempdir().unwrap();
        write(repo.path(), "src/main/java/com/acme/Orders.java", ORDERS);
        write(
            repo.path(),
            "src/main/java/com/acme/Other.java",
            "package com.acme;\nclass Other {}\n",
        );
        let data = tempfile::tempdir().unwrap();
        let backend = FakeBackend::new();
        backend.always("Description", DESCRIBED);

        let (stats, logs) = with_warnings(run_llm_stages(
            repo.path(),
            data.path(),
            None,
            &JiraMode::Disabled,
            &backend,
            false,
            DescribeConfig::default(),
        ))
        .await;

        assert_eq!(stats.described, 3, "members are cached before the trip");
        assert_eq!(stats.types_failed, 1, "Kind: no scripted reply");
        assert_eq!(
            stats.types_incomplete, 2,
            "Line waits for Kind, Orders for Line"
        );
        assert_eq!(stats.types_skipped, 1, "Other after the trip");
        assert_eq!(stats.type_llm.chat_calls, 1);
        let warning = log_line(&logs, "some types got no description");
        assert!(
            warning.contains("failed=1")
                && warning.contains("waiting_for_members=2")
                && warning.contains("skipped=1"),
            "{warning}"
        );

        backend.always("TypeDescription", TYPE_DESCRIBED);
        let stats = run_llm_stages(
            repo.path(),
            data.path(),
            None,
            &JiraMode::Disabled,
            &backend,
            false,
            DescribeConfig::default(),
        )
        .await;
        assert_eq!(stats.types_described, 4, "the next run catches up");
        assert_eq!(stats.described_cached, 3);
    }

    #[tokio::test]
    async fn without_a_summarizer_types_are_skipped() {
        let repo = tempfile::tempdir().unwrap();
        write(repo.path(), "src/main/java/com/acme/Orders.java", ORDERS);
        let data = tempfile::tempdir().unwrap();

        let stats = index_with(repo.path(), data.path(), None).await.unwrap();

        assert_eq!(stats.type_symbols, 3);
        assert_eq!(stats.types_skipped, 3);
        assert_describe_buckets(&stats);
    }
}
