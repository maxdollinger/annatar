//! The ticket cache and how the ticket stage talks to Jira.
//!
//! [`TicketCache`] stores one row per *requested* ticket key in `cache.db`
//! (`ticket_cache`): the content fields of an available issue, or an
//! unavailable marker for a 403/404 or another permanent failure
//! ([`FetchError::is_permanent`]), plus `fetched_at`. Keying by the
//! requested key matters because Jira answers a moved issue with its new key,
//! and the index looks keys up by what the commits mention. Rows never expire;
//! dropping the table refreshes them.
//!
//! [`TicketFetch`] bundles a [`TicketSource`] with the request policy: the
//! concurrency cap and the 429 back-off. [`fetch_with_retry`] fetches one key
//! and [`check_auth_with_retry`] runs the auth check under that policy; both
//! count the requests they send. Which outcomes are
//! cached, skipped or fatal, and when the source's auth check runs, is decided
//! by the ticket stage in [`crate::indexer`].

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use libsql::{Connection, Row, params};

use crate::config::{ENV_JIRA_TOKEN, JiraConfig};
use crate::jira::{self, FetchError, JiraClient, Ticket, TicketSource};

/// Retries after a 429 before the key is given up for this run.
pub const MAX_RATE_LIMIT_RETRIES: u32 = 3;

/// Wait before the first retry when a 429 carries no usable `Retry-After`;
/// doubled for each further retry.
pub const FALLBACK_BACKOFF: Duration = Duration::from_secs(5);

/// Upper bound on any single 429 wait, so a huge `Retry-After` cannot stall a
/// run indefinitely.
pub const MAX_BACKOFF: Duration = Duration::from_secs(60);

/// What the cache knows about one requested key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CachedTicket {
    /// Fetched content. `key` is the requested key, not the one Jira returned.
    Available(Ticket),
    /// Jira answered 403 or 404, or failed permanently.
    Unavailable,
}

/// `ticket_cache` in `cache.db`, keyed by the requested ticket key.
pub struct TicketCache<'a> {
    conn: &'a Connection,
}

impl<'a> TicketCache<'a> {
    /// Wrap a cache connection (typically [`crate::store::Store::cache`]).
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    /// The cached entry for `key`, if any. A row that does not decode is an
    /// error; the caller treats it as a miss.
    pub async fn get(&self, key: &str) -> Result<Option<CachedTicket>> {
        let mut rows = self
            .conn
            .query(
                "SELECT unavailable, issue_type, summary, description, parent_key
                 FROM ticket_cache WHERE key = ?1",
                params![key],
            )
            .await
            .context("reading ticket cache")?;
        let Some(row) = rows.next().await.context("reading ticket cache row")? else {
            return Ok(None);
        };
        decode(key, &row).map(Some)
    }

    /// Store fetched content under the requested `key`.
    pub async fn put_available(&self, key: &str, ticket: &Ticket) -> Result<()> {
        self.conn
            .execute(
                "INSERT OR REPLACE INTO ticket_cache
                    (key, unavailable, status, issue_type, summary, description, parent_key, fetched_at)
                 VALUES (?1, 0, NULL, ?2, ?3, ?4, ?5, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))",
                params![
                    key,
                    ticket.issue_type.as_str(),
                    ticket.summary.as_str(),
                    ticket.description.as_deref(),
                    ticket.parent_key.as_deref(),
                ],
            )
            .await
            .with_context(|| format!("caching ticket {key}"))?;
        Ok(())
    }

    /// Mark `key` unavailable, recording the HTTP status Jira answered with
    /// (`None` for an invalid key or an unparseable issue).
    pub async fn put_unavailable(&self, key: &str, status: Option<u16>) -> Result<()> {
        self.conn
            .execute(
                "INSERT OR REPLACE INTO ticket_cache
                    (key, unavailable, status, issue_type, summary, description, parent_key, fetched_at)
                 VALUES (?1, 1, ?2, NULL, NULL, NULL, NULL, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))",
                params![key, status.map(i64::from)],
            )
            .await
            .with_context(|| format!("caching ticket {key} as unavailable"))?;
        Ok(())
    }
}

fn decode(key: &str, row: &Row) -> Result<CachedTicket> {
    let unavailable: i64 = row.get(0).context("reading ticket_cache.unavailable")?;
    if unavailable != 0 {
        return Ok(CachedTicket::Unavailable);
    }
    let issue_type: Option<String> = row.get(1).context("reading ticket_cache.issue_type")?;
    let summary: Option<String> = row.get(2).context("reading ticket_cache.summary")?;
    let (Some(issue_type), Some(summary)) = (issue_type, summary) else {
        bail!("ticket_cache row for {key} has content missing");
    };
    Ok(CachedTicket::Available(Ticket {
        key: key.to_string(),
        issue_type,
        summary,
        description: row.get(3).context("reading ticket_cache.description")?,
        parent_key: row.get(4).context("reading ticket_cache.parent_key")?,
    }))
}

/// How an `index` run may reach Jira.
#[derive(Clone)]
pub enum JiraMode {
    /// Fetch the tickets the cache lacks.
    Fetch(TicketFetch),
    /// `--offline`: Jira is configured but not called this run; a later run
    /// may fetch the tickets the cache lacks.
    Offline,
    /// No `[jira]` section or no token: tickets the cache lacks are never
    /// fetched, so later stages treat them as unavailable.
    Disabled,
}

/// A ticket source plus the policy the ticket stage fetches with.
#[derive(Clone)]
pub struct TicketFetch {
    pub(crate) source: Arc<dyn TicketSource>,
    /// Maximum requests in flight; at least 1.
    pub(crate) concurrency: usize,
    /// Retries after a 429 before the key is given up for this run.
    pub(crate) max_retries: u32,
    /// First wait when a 429 has no `Retry-After`; doubled per retry.
    pub(crate) fallback_backoff: Duration,
    /// Cap on any single 429 wait.
    pub(crate) max_backoff: Duration,
}

impl TicketFetch {
    /// The default policy around `source`.
    pub fn new(source: Arc<dyn TicketSource>, concurrency: usize) -> Self {
        Self {
            source,
            concurrency: concurrency.max(1),
            max_retries: MAX_RATE_LIMIT_RETRIES,
            fallback_backoff: FALLBACK_BACKOFF,
            max_backoff: MAX_BACKOFF,
        }
    }

    /// How an `index` run reaches Jira: [`JiraMode::Disabled`] without a
    /// `[jira]` section or a token (one warning, or an info log under
    /// `offline`), [`JiraMode::Offline`] when `offline`, otherwise a fetcher.
    /// A configured section that is otherwise invalid (bad `base_url`, zero
    /// concurrency) is an error.
    pub fn from_config(jira: Option<&JiraConfig>, offline: bool) -> Result<JiraMode> {
        let jira = match jira {
            None => Err("no [jira] section in the config".to_string()),
            Some(jira) if jira.token.as_deref().is_none_or(str::is_empty) => {
                Err(format!("{ENV_JIRA_TOKEN} is not set"))
            }
            Some(jira) => Ok(jira),
        };
        let jira = match jira {
            Ok(jira) => jira,
            Err(missing) => {
                if offline {
                    tracing::info!("{missing}; not fetching tickets, using cached tickets only");
                } else {
                    tracing::warn!("{missing}; not fetching tickets, using cached tickets only");
                }
                return Ok(JiraMode::Disabled);
            }
        };
        if offline {
            tracing::info!("--offline: not fetching tickets; using cached tickets only");
            return Ok(JiraMode::Offline);
        }
        if jira.concurrency == 0 {
            bail!("jira.concurrency must be at least 1");
        }
        let client = JiraClient::new(jira)?;
        Ok(JiraMode::Fetch(Self::new(
            Arc::new(client),
            jira.concurrency,
        )))
    }

    /// The wait before retry number `retry` (0-based) after a 429.
    fn backoff(&self, retry_after: Option<Duration>, retry: u32) -> Duration {
        retry_after
            .unwrap_or_else(|| self.fallback_backoff.saturating_mul(1 << retry.min(16)))
            .min(self.max_backoff)
    }
}

/// The result of fetching one key.
pub(crate) struct Fetched {
    pub(crate) key: String,
    pub(crate) result: Result<Ticket, FetchError>,
}

/// Fetch `key`, retrying a 429 up to `max_retries` times after the wait Jira
/// asks for (or the doubling fallback), capped at `max_backoff`. Any other
/// outcome, and the last 429, is returned as is. `requests` is incremented as
/// each request is sent, so a fetch that is aborted midway is still counted;
/// a key that is not a plain Jira key is answered
/// [`FetchError::InvalidKey`] without a request.
pub(crate) async fn fetch_with_retry(
    fetch: &TicketFetch,
    key: String,
    requests: &AtomicUsize,
) -> Fetched {
    if !jira::is_plain_key(&key) {
        return Fetched {
            result: Err(FetchError::InvalidKey(key.clone())),
            key,
        };
    }
    let result = with_retry(fetch, &key, requests, || fetch.source.fetch(&key)).await;
    Fetched { key, result }
}

/// Run the source's auth check under the same 429 policy as
/// [`fetch_with_retry`], counting each request in `requests`.
pub(crate) async fn check_auth_with_retry(
    fetch: &TicketFetch,
    requests: &AtomicUsize,
) -> Result<(), FetchError> {
    with_retry(fetch, "auth check", requests, || fetch.source.check_auth()).await
}

/// Send `call` until it is not a 429 or `max_retries` retries are used up,
/// waiting [`TicketFetch::backoff`] in between; `what` names the request in
/// the debug log.
async fn with_retry<T, F, Fut>(
    fetch: &TicketFetch,
    what: &str,
    requests: &AtomicUsize,
    mut call: F,
) -> Result<T, FetchError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, FetchError>>,
{
    let mut retry = 0;
    loop {
        requests.fetch_add(1, Ordering::Relaxed);
        match call().await {
            Err(FetchError::RateLimited { retry_after }) if retry < fetch.max_retries => {
                let wait = fetch.backoff(retry_after, retry);
                tracing::debug!(
                    request = what,
                    wait_ms = wait.as_millis() as u64,
                    "rate limited; backing off"
                );
                tokio::time::sleep(wait).await;
                retry += 1;
            }
            result => return result,
        }
    }
}

#[cfg(test)]
pub(crate) mod fake {
    use std::collections::{HashMap, VecDeque};
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::jira::{CheckFuture, FetchFuture};

    /// One scripted answer.
    #[derive(Clone)]
    pub enum Answer {
        Ok(Ticket),
        Unavailable(u16),
        Unauthorized,
        /// A 401 saying the token's scopes do not match the request.
        ScopeMismatch,
        RateLimited(Option<Duration>),
        Status(u16),
        /// A JSON response that is not a parseable issue.
        Parse,
        /// A connection failure.
        Transport,
        /// Never answers, like a request to a host that hangs.
        Hang,
    }

    impl Answer {
        fn into_result(self) -> Result<Ticket, FetchError> {
            match self {
                Self::Ok(ticket) => Ok(ticket),
                Self::Unavailable(status) => Err(FetchError::Unavailable { status }),
                Self::Unauthorized => Err(FetchError::Unauthorized {
                    basic: false,
                    cloud: false,
                    scope: false,
                }),
                Self::ScopeMismatch => Err(FetchError::Unauthorized {
                    basic: true,
                    cloud: false,
                    scope: true,
                }),
                Self::RateLimited(retry_after) => Err(FetchError::RateLimited { retry_after }),
                Self::Status(status) => Err(FetchError::Status { status }),
                Self::Parse => Err(FetchError::Parse(anyhow::anyhow!("no summary"))),
                Self::Transport => Err(FetchError::Transport(
                    reqwest::Client::new()
                        .get("http://")
                        .build()
                        .expect_err("a URL without a host does not build"),
                )),
                Self::Hang => unreachable!("a hanging answer never resolves"),
            }
        }
    }

    /// A ticket for `key` with a summary derived from it.
    pub fn ticket(key: &str) -> Ticket {
        Ticket {
            key: key.to_string(),
            issue_type: "Story".to_string(),
            summary: format!("Summary of {key}"),
            description: Some(format!("Description of {key}")),
            parent_key: None,
        }
    }

    /// A [`TicketSource`] that answers from a per-key script and counts
    /// calls. Each call takes the next scripted answer; the last one repeats.
    /// An unscripted key is a 404. Every call yields once, so concurrent calls
    /// really overlap, and the peak number in flight is recorded. The auth
    /// check answers from its own script the same way (`Ok` when unscripted;
    /// any [`Answer::Ok`] passes) and is counted apart from fetches.
    #[derive(Default)]
    pub struct FakeSource {
        script: Mutex<HashMap<String, VecDeque<Answer>>>,
        auth: Mutex<VecDeque<Answer>>,
        auth_checks: AtomicUsize,
        calls: Mutex<Vec<String>>,
        in_flight: AtomicUsize,
        peak: AtomicUsize,
    }

    impl FakeSource {
        pub fn new() -> Arc<Self> {
            Arc::new(Self::default())
        }

        pub fn script(&self, key: &str, answers: &[Answer]) {
            self.script
                .lock()
                .unwrap()
                .insert(key.to_string(), answers.iter().cloned().collect());
        }

        pub fn script_auth(&self, answers: &[Answer]) {
            *self.auth.lock().unwrap() = answers.iter().cloned().collect();
        }

        pub fn auth_checks(&self) -> usize {
            self.auth_checks.load(Ordering::SeqCst)
        }

        /// Ticket fetches only; auth checks are counted by [`Self::auth_checks`].
        pub fn calls(&self) -> usize {
            self.calls.lock().unwrap().len()
        }

        pub fn calls_for(&self, key: &str) -> usize {
            self.calls
                .lock()
                .unwrap()
                .iter()
                .filter(|called| *called == key)
                .count()
        }

        pub fn peak(&self) -> usize {
            self.peak.load(Ordering::SeqCst)
        }

        fn answer(&self, key: &str) -> Answer {
            let mut script = self.script.lock().unwrap();
            match script.get_mut(key) {
                Some(answers) => next(answers).unwrap_or(Answer::Unavailable(404)),
                None => Answer::Unavailable(404),
            }
        }
    }

    /// The next scripted answer; the last one repeats.
    fn next(answers: &mut VecDeque<Answer>) -> Option<Answer> {
        if answers.len() > 1 {
            answers.pop_front()
        } else {
            answers.front().cloned()
        }
    }

    impl TicketSource for FakeSource {
        fn fetch<'a>(&'a self, key: &'a str) -> FetchFuture<'a> {
            Box::pin(async move {
                self.calls.lock().unwrap().push(key.to_string());
                let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                self.peak.fetch_max(now, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(2)).await;
                self.in_flight.fetch_sub(1, Ordering::SeqCst);
                let answer = self.answer(key);
                if matches!(answer, Answer::Hang) {
                    std::future::pending::<()>().await;
                }
                answer.into_result()
            })
        }

        fn check_auth(&self) -> CheckFuture<'_> {
            Box::pin(async move {
                self.auth_checks.fetch_add(1, Ordering::SeqCst);
                let answer = next(&mut self.auth.lock().unwrap());
                match answer {
                    None | Some(Answer::Ok(_)) => Ok(()),
                    Some(Answer::Hang) => unreachable!("the auth check is never scripted to hang"),
                    Some(answer) => answer.into_result().map(drop),
                }
            })
        }
    }

    /// A fetch policy around `source` that never actually sleeps on a 429.
    pub fn instant(source: Arc<FakeSource>, concurrency: usize) -> TicketFetch {
        TicketFetch {
            fallback_backoff: Duration::ZERO,
            max_backoff: Duration::ZERO,
            ..TicketFetch::new(source, concurrency)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::{Answer, FakeSource, instant, ticket};
    use super::*;
    use crate::config::Config;
    use crate::store::Store;

    #[tokio::test]
    async fn cache_round_trips_content_and_unavailable_under_the_requested_key() {
        let data = tempfile::tempdir().unwrap();
        let store = Store::open(data.path()).await.unwrap();
        let cache = TicketCache::new(store.cache());

        assert_eq!(cache.get("GRLD-1").await.unwrap(), None);

        let mut moved = ticket("GRLD-900");
        moved.parent_key = Some("GRLD-5".to_string());
        moved.description = None;
        cache.put_available("GRLD-1", &moved).await.unwrap();
        cache.put_unavailable("GRLD-2", Some(404)).await.unwrap();

        let Some(CachedTicket::Available(stored)) = cache.get("GRLD-1").await.unwrap() else {
            panic!("GRLD-1 should be cached with content");
        };
        assert_eq!(stored.key, "GRLD-1", "keyed by the requested key");
        assert_eq!(stored.summary, "Summary of GRLD-900");
        assert_eq!(stored.parent_key.as_deref(), Some("GRLD-5"));
        assert_eq!(stored.description, None);
        assert_eq!(
            cache.get("GRLD-2").await.unwrap(),
            Some(CachedTicket::Unavailable)
        );

        let mut rows = store
            .cache()
            .query(
                "SELECT status, fetched_at FROM ticket_cache WHERE key = 'GRLD-2'",
                (),
            )
            .await
            .unwrap();
        let row = rows.next().await.unwrap().unwrap();
        assert_eq!(row.get::<i64>(0).unwrap(), 404);
        let fetched_at = row.get::<String>(1).unwrap();
        assert!(
            fetched_at.len() == 20 && fetched_at.ends_with('Z'),
            "{fetched_at}"
        );
    }

    #[tokio::test]
    async fn rate_limit_is_retried_up_to_the_cap() {
        let source = FakeSource::new();
        source.script(
            "GRLD-1",
            &[
                Answer::RateLimited(Some(Duration::from_secs(0))),
                Answer::RateLimited(None),
                Answer::Ok(ticket("GRLD-1")),
            ],
        );
        source.script("GRLD-2", &[Answer::RateLimited(None)]);
        let fetch = instant(source.clone(), 1);

        let requests = AtomicUsize::new(0);
        let fetched = fetch_with_retry(&fetch, "GRLD-1".to_string(), &requests).await;
        assert_eq!(requests.load(Ordering::Relaxed), 3);
        assert!(fetched.result.is_ok());

        let requests = AtomicUsize::new(0);
        let fetched = fetch_with_retry(&fetch, "GRLD-2".to_string(), &requests).await;
        assert_eq!(
            requests.load(Ordering::Relaxed),
            1 + MAX_RATE_LIMIT_RETRIES as usize
        );
        assert!(matches!(
            fetched.result,
            Err(FetchError::RateLimited { .. })
        ));
        assert_eq!(source.calls(), 3 + 4);
    }

    #[tokio::test]
    async fn auth_check_retries_a_rate_limit_like_a_fetch() {
        let source = FakeSource::new();
        source.script_auth(&[
            Answer::RateLimited(Some(Duration::from_secs(0))),
            Answer::RateLimited(None),
            Answer::Ok(ticket("GRLD-1")),
        ]);
        let fetch = instant(source.clone(), 1);

        let requests = AtomicUsize::new(0);
        assert!(check_auth_with_retry(&fetch, &requests).await.is_ok());
        assert_eq!(requests.load(Ordering::Relaxed), 3);

        source.script_auth(&[Answer::RateLimited(None)]);
        let requests = AtomicUsize::new(0);
        assert!(matches!(
            check_auth_with_retry(&fetch, &requests).await,
            Err(FetchError::RateLimited { .. })
        ));
        assert_eq!(
            requests.load(Ordering::Relaxed),
            1 + MAX_RATE_LIMIT_RETRIES as usize
        );
        assert_eq!(source.auth_checks(), 3 + 4);
        assert_eq!(source.calls(), 0);
    }

    #[tokio::test]
    async fn invalid_key_is_not_sent_and_not_counted() {
        let source = FakeSource::new();
        let requests = AtomicUsize::new(0);

        let fetched = fetch_with_retry(
            &instant(source.clone(), 1),
            "GRLD 1/../x".to_string(),
            &requests,
        )
        .await;

        assert!(matches!(fetched.result, Err(FetchError::InvalidKey(_))));
        assert_eq!(requests.load(Ordering::Relaxed), 0);
        assert_eq!(source.calls(), 0);
    }

    #[test]
    fn backoff_honours_retry_after_and_doubles_the_fallback_up_to_the_cap() {
        let fetch = TicketFetch::new(FakeSource::new(), 4);

        assert_eq!(
            fetch.backoff(Some(Duration::from_secs(7)), 0),
            Duration::from_secs(7)
        );
        assert_eq!(fetch.backoff(None, 0), FALLBACK_BACKOFF);
        assert_eq!(fetch.backoff(None, 2), FALLBACK_BACKOFF * 4);
        assert_eq!(
            fetch.backoff(Some(Duration::from_secs(3600)), 0),
            MAX_BACKOFF
        );
        assert_eq!(fetch.backoff(None, 30), MAX_BACKOFF);
    }

    fn jira_config(extra: &str) -> JiraConfig {
        Config::parse(&format!(
            "repo = \".\"\ndata_dir = \".\"\n[jira]\nbase_url = \"https://jira.example.com\"\n{extra}"
        ))
        .unwrap()
        .jira
        .unwrap()
    }

    fn fetch_of(mode: JiraMode) -> TicketFetch {
        match mode {
            JiraMode::Fetch(fetch) => fetch,
            _ => panic!("expected a fetcher"),
        }
    }

    #[test]
    fn from_config_tells_disabled_from_offline() {
        let disabled = |jira: Option<&JiraConfig>, offline: bool| {
            matches!(
                TicketFetch::from_config(jira, offline).unwrap(),
                JiraMode::Disabled
            )
        };
        assert!(disabled(None, false));
        assert!(
            disabled(None, true),
            "--offline without Jira is still disabled"
        );

        let mut jira = jira_config("");
        assert!(disabled(Some(&jira), false));
        jira.token = Some(String::new());
        assert!(disabled(Some(&jira), true));

        jira.token = Some("t".to_string());
        assert!(matches!(
            TicketFetch::from_config(Some(&jira), true).unwrap(),
            JiraMode::Offline
        ));
        let fetch = fetch_of(TicketFetch::from_config(Some(&jira), false).unwrap());
        assert_eq!(fetch.concurrency, 4);
    }

    #[test]
    fn from_config_reads_concurrency_and_rejects_bad_settings() {
        let mut jira = jira_config("concurrency = 2");
        jira.token = Some("t".to_string());
        let fetch = fetch_of(TicketFetch::from_config(Some(&jira), false).unwrap());
        assert_eq!(fetch.concurrency, 2);

        jira.concurrency = 0;
        assert!(TicketFetch::from_config(Some(&jira), false).is_err());

        jira.concurrency = 4;
        jira.base_url = "jira.example.com".to_string();
        assert!(TicketFetch::from_config(Some(&jira), false).is_err());
    }
}
