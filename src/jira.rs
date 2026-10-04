//! Fetch one Jira issue and parse it into a [`Ticket`].
//!
//! The client calls `GET /rest/api/2/issue/{key}?expand=renderedFields`.
//! REST v2 works on both Cloud and Server/DC, and `renderedFields` carries the
//! description as HTML on both, so [`html_to_text`] turns it into plain text
//! without an Atlassian Document Format or wiki-markup parser.
//!
//! Auth: with an email configured ([`ENV_JIRA_EMAIL`]) requests use basic auth
//! `email:token` (Cloud API token); without one they send the token as a bearer
//! personal access token (Server/DC). A classic Cloud API token works against
//! the site URL; a scoped one only through the Atlassian API gateway
//! (`https://api.atlassian.com/ex/jira/<cloudId>`) and needs `read:jira-work`.
//!
//! The parent key comes from `fields.parent.key` (sub-tasks, and epic children
//! on Cloud). An optional `jira.epic_link_field` names a custom field that
//! holds the epic key on Server/DC; it is read only when there is no parent
//! (or its key is empty).
//!
//! [`FetchError`] keeps the HTTP outcomes 3.2 needs apart: bad credentials,
//! an unavailable issue, rate limiting and everything else. A 403 carrying
//! `X-Authentication-Denied-Reason` (Server/DC CAPTCHA after failed logins)
//! counts as bad credentials, and so does a 401 or 403 whose body is Jira
//! Cloud's "Failed to parse Connect Session Auth Token" (Cloud's answer to a
//! bearer token). A 401 whose body says `scope does not match` is bad
//! credentials with `scope` set: a scoped token lacking the scope that request
//! needs. [`JiraClient::check_auth`] calls `GET /rest/api/2/myself`
//! once before the ticket stage fetches, so credentials Jira rejects with a
//! plain 403 cannot be mistaken for unavailable issues. Error bodies are read
//! up to [`MAX_ERROR_BODY`] bytes. [`TicketSource`] is
//! the seam the ticket stage fetches through; caching, retries and
//! concurrency live in [`crate::tickets`] and the index build, not here.

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use reqwest::header::{ACCEPT, HeaderMap, RETRY_AFTER};
use serde_json::Value;

use crate::config::{ENV_JIRA_EMAIL, ENV_JIRA_TOKEN, JiraConfig};

/// Per-request timeout.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Timeout for establishing the connection, so an unreachable host fails
/// fast instead of using the whole request timeout.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// How much of a non-success response body is read to classify the error.
const MAX_ERROR_BODY: usize = 4 * 1024;

/// Wrap width handed to the HTML converter; wide enough that paragraphs stay
/// on one line.
const TEXT_WIDTH: usize = 10_000;

/// A Jira issue reduced to what the summaries need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ticket {
    /// Issue key as Jira reports it (a moved issue reports its new key).
    pub key: String,
    /// Issue type name, e.g. `Story`, `Bug`, `Task` (Cloud sub-tasks are often
    /// typed `Task` with `subtask: true`).
    pub issue_type: String,
    /// Summary with surrounding whitespace trimmed.
    pub summary: String,
    /// Plain-text description; `None` when the issue has none.
    pub description: Option<String>,
    /// Parent issue or epic key.
    pub parent_key: Option<String>,
}

/// Parse an issue response (`expand=renderedFields`) into a [`Ticket`].
pub fn parse_issue(json: &Value, epic_link_field: Option<&str>) -> Result<Ticket> {
    let key = string_at(json, "/key").context("issue has no key")?;
    let issue_type = string_at(json, "/fields/issuetype/name")
        .with_context(|| format!("{key}: no issue type"))?;
    let summary = string_at(json, "/fields/summary")
        .map(|summary| summary.trim().to_string())
        .filter(|summary| !summary.is_empty())
        .with_context(|| format!("{key}: no summary"))?;
    let description = match json.pointer("/renderedFields/description") {
        Some(Value::String(html)) => {
            Some(html_to_text(html).with_context(|| format!("{key}: description"))?)
        }
        _ => None,
    }
    .filter(|text| !text.is_empty());
    let parent_key = string_at(json, "/fields/parent/key")
        .filter(|key| !key.is_empty())
        .or_else(|| {
            let field = json.get("fields")?.get(epic_link_field?)?;
            match field {
                Value::String(key) => Some(key.clone()),
                other => other.get("key")?.as_str().map(str::to_string),
            }
        });
    Ok(Ticket {
        key,
        issue_type,
        summary,
        description,
        parent_key: parent_key.filter(|key| !key.is_empty()),
    })
}

fn string_at(json: &Value, pointer: &str) -> Option<String> {
    json.pointer(pointer)?.as_str().map(str::to_string)
}

/// Convert Jira's rendered HTML to plain text: no emphasis or strikeout
/// markers, table borders or link URLs (links keep their text in `[text]`
/// brackets); list items and code blocks keep their lines; runs of blank lines
/// collapse to one, and surrounding blank lines and trailing spaces are
/// dropped.
fn html_to_text(html: &str) -> Result<String> {
    let text = html2text::config::plain_no_decorate()
        .no_table_borders()
        .link_footnotes(false)
        .unicode_strikeout(false)
        .string_from_read(html.as_bytes(), TEXT_WIDTH)
        .map_err(|err| anyhow!("converting HTML to text: {err}"))?;
    let mut lines: Vec<&str> = Vec::new();
    for line in text.lines().map(str::trim_end) {
        if !(line.is_empty() && lines.last().is_some_and(|last| last.is_empty())) {
            lines.push(line);
        }
    }
    Ok(lines.join("\n").trim().to_string())
}

/// How requests authenticate.
#[derive(Clone)]
pub enum Auth {
    /// Cloud: account email plus API token.
    Basic { email: String, token: String },
    /// Server/DC: personal access token.
    Bearer { token: String },
}

impl Auth {
    /// Basic auth when an email is configured, bearer otherwise. The token is
    /// required; empty values count as unset.
    pub fn from_config(config: &JiraConfig) -> Result<Self> {
        let token = config
            .token
            .clone()
            .filter(|token| !token.is_empty())
            .with_context(|| format!("{ENV_JIRA_TOKEN} is not set"))?;
        Ok(
            match config.email.clone().filter(|email| !email.is_empty()) {
                Some(email) => Self::Basic { email, token },
                None => Self::Bearer { token },
            },
        )
    }
}

/// Why fetching an issue failed.
#[derive(Debug)]
pub enum FetchError {
    /// 401, a 403 with `X-Authentication-Denied-Reason` or Cloud's Connect
    /// token error, or any 401/403 from the auth check: credentials missing,
    /// wrong or of the wrong kind. Must never be cached. `basic` names the
    /// auth mode for the message; `cloud` is set when the response showed the
    /// server is Jira Cloud; `scope` when a 401 said the token's scopes do not
    /// match the request (a scoped API token without the scope it needs).
    Unauthorized {
        basic: bool,
        cloud: bool,
        scope: bool,
    },
    /// 403 or 404: the issue does not exist or is not visible to this account.
    Unavailable { status: u16 },
    /// 429: back off, for `retry_after` when Jira sent a seconds value.
    RateLimited { retry_after: Option<Duration> },
    /// Any other non-success status.
    Status { status: u16 },
    /// The key is not a plain Jira key and was not sent.
    InvalidKey(String),
    /// Connection, TLS or timeout failure.
    Transport(reqwest::Error),
    /// A success response that is JSON but not a parseable issue (e.g. no
    /// summary).
    Parse(anyhow::Error),
    /// A success response that is not JSON at all (a proxy or login page).
    NotJson(anyhow::Error),
}

impl FetchError {
    /// Whether asking again cannot help: the issue is unavailable, the key is
    /// not a Jira key, the issue does not parse, or Jira answered a 4xx other
    /// than 408, 425 and 429 (401 is [`FetchError::Unauthorized`]). The
    /// ticket stage caches such keys as unavailable.
    pub fn is_permanent(&self) -> bool {
        match self {
            Self::Unavailable { .. } | Self::InvalidKey(_) | Self::Parse(_) => true,
            Self::Status { status } => {
                (400..500).contains(status) && !matches!(status, 408 | 425 | 429)
            }
            _ => false,
        }
    }

    /// The HTTP status behind the error, if a non-success one was received.
    pub fn status(&self) -> Option<u16> {
        match self {
            Self::Unavailable { status } | Self::Status { status } => Some(*status),
            _ => None,
        }
    }
}

impl fmt::Display for FetchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unauthorized { scope: true, .. } => write!(
                f,
                "Jira rejected the credentials: the token lacks the scope Jira needs for this \
                 request; a scoped API token needs read:jira-work (and goes through \
                 https://api.atlassian.com/ex/jira/<cloudId> as jira.base_url)"
            ),
            Self::Unauthorized { basic: true, .. } => write!(
                f,
                "Jira rejected the credentials; check {ENV_JIRA_TOKEN} and {ENV_JIRA_EMAIL}"
            ),
            Self::Unauthorized {
                basic: false,
                cloud: true,
                ..
            } => write!(
                f,
                "Jira rejected the credentials; this is Jira Cloud, which does not accept a \
                 bearer token: set {ENV_JIRA_EMAIL} to the account email so {ENV_JIRA_TOKEN} \
                 (an API token) is sent as basic auth; a classic API token works with the site \
                 URL as jira.base_url, a scoped one (scope read:jira-work) only with \
                 https://api.atlassian.com/ex/jira/<cloudId>"
            ),
            Self::Unauthorized {
                basic: false,
                cloud: false,
                ..
            } => write!(f, "Jira rejected the credentials; check {ENV_JIRA_TOKEN}"),
            Self::Unavailable { status } => write!(f, "issue unavailable (HTTP {status})"),
            Self::RateLimited {
                retry_after: Some(wait),
            } => write!(f, "rate limited (429), retry after {}s", wait.as_secs()),
            Self::RateLimited { retry_after: None } => write!(f, "rate limited (429)"),
            Self::Status { status } => write!(f, "Jira returned HTTP {status}"),
            Self::InvalidKey(key) => write!(f, "not a Jira key: {key:?}"),
            Self::Transport(err) => write!(f, "Jira request failed: {err}"),
            Self::Parse(err) => write!(f, "unexpected Jira response: {err:#}"),
            Self::NotJson(err) => write!(f, "Jira response is not JSON: {err:#}"),
        }
    }
}

impl std::error::Error for FetchError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Transport(err) => Some(err),
            _ => None,
        }
    }
}

/// Header Jira Server/DC adds to a 403 when it refuses the login itself
/// (e.g. CAPTCHA after repeated failures).
const AUTH_DENIED_REASON: &str = "x-authentication-denied-reason";

/// What Jira Cloud answers a request whose bearer token it cannot read.
const CLOUD_CONNECT_TOKEN_ERROR: &[u8] = b"Connect Session Auth Token";

/// What the Atlassian gateway answers a scoped token that lacks the scope a
/// request needs.
const SCOPE_MISMATCH_ERROR: &[u8] = b"scope does not match";

fn contains(body: &[u8], needle: &[u8]) -> bool {
    body.windows(needle.len()).any(|window| window == needle)
}

/// Whether an error body is Jira Cloud's answer to an unreadable bearer token.
fn is_cloud_token_error(body: &[u8]) -> bool {
    contains(body, CLOUD_CONNECT_TOKEN_ERROR)
}

/// Whether a 401 body says the token's scopes do not cover the request.
fn is_scope_error(body: &[u8]) -> bool {
    contains(body, SCOPE_MISMATCH_ERROR)
}

/// Map a non-success status, its headers and its body to a [`FetchError`];
/// `None` for 2xx. `basic` is the auth mode, carried into
/// [`FetchError::Unauthorized`].
fn status_error(status: u16, headers: &HeaderMap, body: &[u8], basic: bool) -> Option<FetchError> {
    let cloud = is_cloud_token_error(body);
    match status {
        200..=299 => None,
        401 => Some(FetchError::Unauthorized {
            basic,
            cloud,
            scope: is_scope_error(body),
        }),
        403 if cloud || headers.contains_key(AUTH_DENIED_REASON) => {
            Some(FetchError::Unauthorized {
                basic,
                cloud,
                scope: false,
            })
        }
        403 | 404 => Some(FetchError::Unavailable { status }),
        429 => Some(FetchError::RateLimited {
            retry_after: headers
                .get(RETRY_AFTER)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.trim().parse().ok())
                .map(Duration::from_secs),
        }),
        _ => Some(FetchError::Status { status }),
    }
}

/// Map the auth check's status to a [`FetchError`]; `None` for 2xx. Unlike
/// [`status_error`], any 403 is bad credentials: `myself` exists for every
/// account that may log in. A 401 with `scope does not match` is
/// `Unauthorized { scope: true }`, which is inconclusive rather than bad
/// credentials: `myself` needs `read:jira-user`, which a scoped token that
/// can read issues (`read:jira-work`) may lack. The ticket stage goes on to
/// fetch, where a 401 is fatal anyway.
fn auth_check_error(
    status: u16,
    headers: &HeaderMap,
    body: &[u8],
    basic: bool,
) -> Option<FetchError> {
    match status {
        401 | 403 => Some(FetchError::Unauthorized {
            basic,
            cloud: is_cloud_token_error(body),
            scope: status == 401 && is_scope_error(body),
        }),
        _ => status_error(status, headers, body, basic),
    }
}

/// The issue URL for `key` under `base_url`.
fn issue_url(base_url: &str, key: &str) -> String {
    format!(
        "{}/rest/api/2/issue/{key}?expand=renderedFields",
        base_url.trim_end_matches('/')
    )
}

/// The auth check URL under `base_url`.
fn myself_url(base_url: &str) -> String {
    format!("{}/rest/api/2/myself", base_url.trim_end_matches('/'))
}

/// A key is safe to put in the URL path: ASCII letters, digits, `_` and `-`.
pub(crate) fn is_plain_key(key: &str) -> bool {
    !key.is_empty()
        && key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

/// Jira REST client for single-issue fetches.
pub struct JiraClient {
    http: reqwest::Client,
    base_url: String,
    auth: Auth,
    epic_link_field: Option<String>,
}

impl JiraClient {
    /// Build a client from the Jira config. Fails when no token is set or
    /// `base_url` is not an absolute http(s) URL.
    pub fn new(config: &JiraConfig) -> Result<Self> {
        let base_url = config.base_url.trim();
        if base_url.is_empty() {
            bail!("jira.base_url is empty");
        }
        let url = reqwest::Url::parse(base_url)
            .with_context(|| format!("jira.base_url {base_url:?} is not a URL"))?;
        if !matches!(url.scheme(), "http" | "https") {
            bail!("jira.base_url {base_url:?} must start with http:// or https://");
        }
        let auth = Auth::from_config(config)?;
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
            .context("building the HTTP client")?;
        Ok(Self {
            http,
            base_url: base_url.to_string(),
            auth,
            epic_link_field: config.epic_link_field.clone(),
        })
    }

    /// Fetch and parse one issue.
    pub async fn fetch(&self, key: &str) -> Result<Ticket, FetchError> {
        let body = self.send(self.request(key)?, status_error).await?;
        let json: Value =
            serde_json::from_slice(&body).map_err(|err| FetchError::NotJson(anyhow!(err)))?;
        parse_issue(&json, self.epic_link_field.as_deref()).map_err(FetchError::Parse)
    }

    /// Check that Jira accepts the credentials: `GET /rest/api/2/myself`.
    /// 401 and 403 are [`FetchError::Unauthorized`] (see [`auth_check_error`]
    /// for the inconclusive scope case).
    pub async fn check_auth(&self) -> Result<(), FetchError> {
        self.send(self.myself_request()?, auth_check_error)
            .await
            .map(drop)
    }

    /// Send `request` and return the body of a 2xx response; other statuses
    /// go through `map_status` with at most [`MAX_ERROR_BODY`] bytes of their
    /// body (a body that fails to read counts as cut short).
    async fn send(
        &self,
        request: reqwest::Request,
        map_status: fn(u16, &HeaderMap, &[u8], bool) -> Option<FetchError>,
    ) -> Result<Vec<u8>, FetchError> {
        let mut response = self
            .http
            .execute(request)
            .await
            .map_err(FetchError::Transport)?;
        let status = response.status().as_u16();
        if response.status().is_success() {
            return response
                .bytes()
                .await
                .map(Vec::from)
                .map_err(FetchError::Transport);
        }
        let headers = response.headers().clone();
        let mut body = Vec::new();
        while body.len() < MAX_ERROR_BODY {
            let Ok(Some(chunk)) = response.chunk().await else {
                break;
            };
            let take = chunk.len().min(MAX_ERROR_BODY - body.len());
            body.extend_from_slice(&chunk[..take]);
        }
        let basic = matches!(self.auth, Auth::Basic { .. });
        Err(map_status(status, &headers, &body, basic).unwrap_or(FetchError::Status { status }))
    }

    /// The authenticated issue request for `key`, built but not sent.
    fn request(&self, key: &str) -> Result<reqwest::Request, FetchError> {
        if !is_plain_key(key) {
            return Err(FetchError::InvalidKey(key.to_string()));
        }
        self.get(issue_url(&self.base_url, key))
    }

    /// The authenticated auth-check request, built but not sent.
    fn myself_request(&self) -> Result<reqwest::Request, FetchError> {
        self.get(myself_url(&self.base_url))
    }

    /// An authenticated JSON `GET` of `url`, built but not sent.
    fn get(&self, url: String) -> Result<reqwest::Request, FetchError> {
        let request = self.http.get(url).header(ACCEPT, "application/json");
        let request = match &self.auth {
            Auth::Basic { email, token } => request.basic_auth(email, Some(token)),
            Auth::Bearer { token } => request.bearer_auth(token),
        };
        request.build().map_err(FetchError::Transport)
    }
}

/// The future [`TicketSource::fetch`] returns: boxed so the trait stays
/// object-safe, `Send` so the ticket stage can run fetches as tasks.
pub type FetchFuture<'a> = Pin<Box<dyn Future<Output = Result<Ticket, FetchError>> + Send + 'a>>;

/// The future [`TicketSource::check_auth`] returns.
pub type CheckFuture<'a> = Pin<Box<dyn Future<Output = Result<(), FetchError>> + Send + 'a>>;

/// Where the ticket stage gets issues from: [`JiraClient`] in production, a
/// scripted fake in tests. One call is one Jira request.
pub trait TicketSource: Send + Sync {
    /// Fetch and parse the issue `key`.
    fn fetch<'a>(&'a self, key: &'a str) -> FetchFuture<'a>;

    /// Check that the credentials are accepted, before any fetch.
    fn check_auth(&self) -> CheckFuture<'_>;
}

impl TicketSource for JiraClient {
    fn fetch<'a>(&'a self, key: &'a str) -> FetchFuture<'a> {
        Box::pin(JiraClient::fetch(self, key))
    }

    fn check_auth(&self) -> CheckFuture<'_> {
        Box::pin(JiraClient::check_auth(self))
    }
}

/// Status, header and body mapping is tested on [`status_error`] and
/// [`auth_check_error`] directly, and requests are built without sending.
/// How [`JiraClient::send`] reads a body (2xx in full, error bodies capped at
/// [`MAX_ERROR_BODY`]) is untested by design: there is no HTTP mock (D-ad).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn fixture(name: &str) -> Value {
        let path = format!(
            "{}/tests/fixtures/jira/{name}.json",
            env!("CARGO_MANIFEST_DIR")
        );
        let text = std::fs::read_to_string(&path).unwrap();
        serde_json::from_str(&text).unwrap()
    }

    fn jira(token: Option<&str>, email: Option<&str>) -> JiraConfig {
        let mut config = Config::parse(
            "repo = \".\"\ndata_dir = \".\"\n[jira]\nbase_url = \"https://jira.example.com\"\n",
        )
        .unwrap()
        .jira
        .unwrap();
        config.token = token.map(str::to_string);
        config.email = email.map(str::to_string);
        config
    }

    #[test]
    fn parses_a_story_with_a_list() {
        let ticket = parse_issue(&fixture("story"), None).unwrap();

        assert_eq!(ticket.key, "GRLD-24229");
        assert_eq!(ticket.issue_type, "Story");
        assert_eq!(ticket.summary, "SMS Versand (Dev & Color)");
        assert_eq!(ticket.parent_key.as_deref(), Some("GRLD-23870"));
        assert_eq!(
            ticket.description.as_deref(),
            Some("Out of scope:\n* keine Validierung\n* keine Historie")
        );
    }

    #[test]
    fn parses_a_bug_with_an_image_and_blank_lines() {
        let ticket = parse_issue(&fixture("bug"), None).unwrap();

        assert_eq!(ticket.key, "GRLD-82584");
        assert_eq!(ticket.issue_type, "Bug");
        assert_eq!(ticket.summary, "Argus Export Validierung Problem");
        assert_eq!(ticket.parent_key, None);
        let description = ticket.description.unwrap();
        assert!(
            description.starts_with(
                "“Ich denke, das problem ist, dass die Export-Validierung im Argus nur schaut"
            ),
            "{description}"
        );
        assert!(
            description.contains("gehört.\n\n[image-20240513-110026.png]\n\nSollte gefixt werden…"),
            "{description}"
        );
        assert!(
            description.ends_with("Sollte gefixt werden…"),
            "{description}"
        );
        assert!(!description.contains("\n\n\n"), "{description}");
        assert!(!description.contains('<'), "HTML left in: {description}");
        assert!(!description.contains("jira.example.com"), "{description}");
    }

    #[test]
    fn parses_a_rich_description() {
        let ticket = parse_issue(&fixture("rich_description"), None).unwrap();

        assert_eq!(ticket.key, "GRLD-72066");
        assert_eq!(ticket.issue_type, "Verbesserung");
        assert_eq!(
            ticket.summary,
            "🎄Unit-Test - Privilegien Kollisionen vermeiden"
        );
        assert_eq!(ticket.parent_key.as_deref(), Some("GRLD-52540"));
        let description = ticket.description.unwrap();
        assert!(
            description.starts_with(
                "Um mögliche Kollisionen von Privilegien zu erkennen, welche in unterschiedlichen \
                 Java Klassen gepflegt werden, sollte jeder Microservice einen entsprechenden \
                 Unit-Test einbauen.\n\n## Möglichkeit 1\n\nNutzt die service-starter Bibliothek: \
                 [https://git.example.com/acme/service-starter/src/main/]"
            ),
            "{description}"
        );
        assert!(
            description.contains(
                "Wichtig: Bitte ergänzt alle Klassen, welche Privilegien für euer Projekt \
                 definieren im privilegeClasses Set."
            ),
            "{description}"
        );
        assert!(
            description.contains(
                "    final Set<Class<?>> privilegeClasses = Set.of();\n\n    \
                 CodeGuidelinePlausibilityTestDataFactory.setPrivilegeClasses(privilegeClasses);"
            ),
            "{description}"
        );
        assert!(
            description.contains(
                "  void checkForPrivilegeCollisions() throws IllegalAccessException {\n    \
                 Map<String, Set<String>> privilegeClassMap = new HashMap<>();"
            ),
            "{description}"
        );
        assert!(
            description.contains(
                "* Ein Text-Schlüssel \"ORGANIZATION_TITLE\", über den die Einstellungsseite"
            ),
            "{description}"
        );
        assert!(
            description.ends_with(
                "  * Da beide Seiten nun den gleichen Schlüssel nutzen ist es möglich:\n    \
                 * Auf der Einstellungsseite den Admin Text anzuzeigen (andere Fallback-Texte \
                 erstmal ignoriert)\n    * Auf der Übersichtsseite den Nutzer-Text anzuzeigen"
            ),
            "{description}"
        );
        assert!(!description.contains("<span"), "{description}");
        assert!(!description.contains("&lt;"), "{description}");
        assert!(!description.contains("**"), "{description}");
        assert!(!description.contains("\n\n\n"), "{description}");
    }

    #[test]
    fn sub_task_takes_its_parent_key() {
        let ticket = parse_issue(&fixture("subtask"), None).unwrap();

        assert_eq!(ticket.key, "GRLD-100");
        assert_eq!(ticket.issue_type, "Task");
        assert_eq!(ticket.summary, "Implementierung + Klärung offener Fragen");
        assert_eq!(ticket.parent_key.as_deref(), Some("GRLD-84"));
        assert_eq!(ticket.description, None);
    }

    #[test]
    fn epic_child_takes_the_epic_from_its_parent() {
        let ticket = parse_issue(&fixture("epic_child"), None).unwrap();

        assert_eq!(ticket.key, "GRLD-21115");
        assert_eq!(ticket.issue_type, "Verbesserung");
        assert_eq!(
            ticket.summary,
            "Neues Projekt \"Argus\" anlegen - CachingService zukft. AutorisierungsService"
        );
        assert_eq!(ticket.parent_key.as_deref(), Some("GRLD-20898"));
        assert_eq!(
            ticket.description.as_deref(),
            Some("Projekt soll in Dev-Umgebung lauffähig sein")
        );
    }

    #[test]
    fn summary_is_trimmed() {
        let mut json = fixture("story");
        assert_eq!(json["fields"]["summary"], "SMS Versand (Dev & Color) ");
        json["fields"]["summary"] = Value::from("\t SMS Versand \n");

        assert_eq!(parse_issue(&json, None).unwrap().summary, "SMS Versand");
    }

    #[test]
    fn epic_link_field_is_used_only_when_configured() {
        let mut json = fixture("epic_child");
        json["fields"].as_object_mut().unwrap().remove("parent");

        let ticket = parse_issue(&json, None).unwrap();
        assert_eq!(ticket.parent_key, None);

        let ticket = parse_issue(&json, Some("customfield_10930")).unwrap();
        assert_eq!(ticket.parent_key.as_deref(), Some("GRLD-20898"));

        let ticket = parse_issue(&json, Some("customfield_99999")).unwrap();
        assert_eq!(ticket.parent_key, None);
    }

    #[test]
    fn epic_link_field_may_hold_an_object_with_a_key() {
        let mut json = fixture("epic_child");
        json["fields"].as_object_mut().unwrap().remove("parent");
        json["fields"]["customfield_10930"] = serde_json::json!({ "key": "GRLD-301" });

        let ticket = parse_issue(&json, Some("customfield_10930")).unwrap();
        assert_eq!(ticket.parent_key.as_deref(), Some("GRLD-301"));
    }

    #[test]
    fn empty_parent_key_is_none() {
        let mut json = fixture("subtask");
        json["fields"]["parent"]["key"] = Value::from("");

        assert_eq!(parse_issue(&json, None).unwrap().parent_key, None);
    }

    #[test]
    fn empty_parent_key_falls_back_to_the_epic_link_field() {
        let mut json = fixture("epic_child");
        json["fields"]["parent"] = serde_json::json!({ "key": "" });

        let ticket = parse_issue(&json, Some("customfield_10930")).unwrap();
        assert_eq!(ticket.parent_key.as_deref(), Some("GRLD-20898"));
    }

    #[test]
    fn parent_wins_over_epic_link_field() {
        let mut json = fixture("epic_child");
        json["fields"]["customfield_10930"] = Value::from("GRLD-300");

        let ticket = parse_issue(&json, Some("customfield_10930")).unwrap();
        assert_eq!(ticket.parent_key.as_deref(), Some("GRLD-20898"));
    }

    #[test]
    fn null_or_blank_description_is_none() {
        let mut json = fixture("empty_description");
        assert_eq!(json["fields"]["description"], Value::Null);
        assert_eq!(json["renderedFields"]["description"], "");
        let ticket = parse_issue(&json, None).unwrap();
        assert_eq!(ticket.key, "GRLD-1500");
        assert_eq!(ticket.issue_type, "Story");
        assert_eq!(
            ticket.summary,
            "Branch für FU-Test (Features deaktiviert) aktualisieren/neu anlegen"
        );
        assert_eq!(ticket.parent_key, None);
        assert_eq!(ticket.description, None);

        json["renderedFields"]["description"] = Value::Null;
        assert_eq!(parse_issue(&json, None).unwrap().description, None);

        json["renderedFields"]["description"] = Value::from("<p> </p>\n");
        assert_eq!(parse_issue(&json, None).unwrap().description, None);

        json.as_object_mut().unwrap().remove("renderedFields");
        assert_eq!(parse_issue(&json, None).unwrap().description, None);
    }

    #[test]
    fn html_drops_strikeout_and_collapses_blank_lines() {
        assert_eq!(
            html_to_text("<p>was <del>struck</del> out</p>").unwrap(),
            "was struck out"
        );
        assert_eq!(
            html_to_text("<p>one</p><br/><br/><br/><p>two</p><p></p><p></p><p>three</p>").unwrap(),
            "one\n\ntwo\n\nthree"
        );
        assert_eq!(
            html_to_text("<p>see <a href=\"https://wiki.example.com/x\">the spec</a></p>").unwrap(),
            "see [the spec]"
        );
    }

    #[test]
    fn html_table_keeps_cells_without_borders() {
        let text = html_to_text(
            "<table><tr><th>Modul</th><th>Status</th></tr>\
             <tr><td>Login</td><td>Offen</td></tr>\
             <tr><td>Export</td><td>Geschlossen</td></tr></table>",
        )
        .unwrap();

        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3, "{text}");
        for (line, cells) in lines.iter().zip([
            ["Modul", "Status"],
            ["Login", "Offen"],
            ["Export", "Geschlossen"],
        ]) {
            assert!(line.starts_with(cells[0]), "{text}");
            assert!(line.ends_with(cells[1]), "{text}");
        }
        for border in ['|', '─', '│', '┼', '-', '+'] {
            assert!(!text.contains(border), "{border:?} in {text}");
        }
    }

    #[test]
    fn long_paragraph_stays_on_one_line() {
        let paragraph = "word ".repeat(400);
        let text = html_to_text(&format!("<p>{paragraph}</p>")).unwrap();

        assert_eq!(text, paragraph.trim_end());
    }

    #[test]
    fn missing_required_field_is_an_error() {
        let mut json = fixture("story");
        json["fields"].as_object_mut().unwrap().remove("summary");

        let err = parse_issue(&json, None).unwrap_err();
        assert!(
            format!("{err:#}").contains("GRLD-24229: no summary"),
            "{err:#}"
        );

        json["fields"]["summary"] = Value::from(" \t\n");
        let err = parse_issue(&json, None).unwrap_err();
        assert!(
            format!("{err:#}").contains("GRLD-24229: no summary"),
            "{err:#}"
        );
    }

    #[test]
    fn auth_is_basic_with_email_and_bearer_without() {
        assert!(matches!(
            Auth::from_config(&jira(Some("t"), Some("a@example.com"))).unwrap(),
            Auth::Basic { email, token } if email == "a@example.com" && token == "t"
        ));
        assert!(matches!(
            Auth::from_config(&jira(Some("t"), None)).unwrap(),
            Auth::Bearer { token } if token == "t"
        ));
        assert!(matches!(
            Auth::from_config(&jira(Some("t"), Some(""))).unwrap(),
            Auth::Bearer { .. }
        ));
        for token in [None, Some("")] {
            let err = Auth::from_config(&jira(token, Some("a@example.com")))
                .err()
                .unwrap();
            assert!(err.to_string().contains(ENV_JIRA_TOKEN), "{err}");
        }
    }

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        pairs
            .iter()
            .map(|(name, value)| {
                (
                    reqwest::header::HeaderName::from_static(name),
                    value.parse().unwrap(),
                )
            })
            .collect()
    }

    #[test]
    fn statuses_map_to_distinct_errors() {
        let none = HeaderMap::new();
        assert!(status_error(200, &none, b"", false).is_none());
        assert!(matches!(
            status_error(401, &none, b"", true),
            Some(FetchError::Unauthorized {
                basic: true,
                cloud: false,
                scope: false,
            })
        ));
        assert!(matches!(
            status_error(403, &none, b"", false),
            Some(FetchError::Unavailable { status: 403 })
        ));
        assert!(matches!(
            status_error(404, &none, b"", false),
            Some(FetchError::Unavailable { status: 404 })
        ));
        assert!(matches!(
            status_error(429, &headers(&[("retry-after", " 17 ")]), b"", false),
            Some(FetchError::RateLimited { retry_after: Some(wait) }) if wait == Duration::from_secs(17)
        ));
        assert!(matches!(
            status_error(
                429,
                &headers(&[("retry-after", "Wed, 21 Oct 2026 07:28:00 GMT")]),
                b"",
                false
            ),
            Some(FetchError::RateLimited { retry_after: None })
        ));
        assert!(matches!(
            status_error(500, &none, b"", false),
            Some(FetchError::Status { status: 500 })
        ));
    }

    #[test]
    fn forbidden_with_auth_denied_reason_is_unauthorized() {
        let denied = headers(&[(
            "x-authentication-denied-reason",
            "CAPTCHA_CHALLENGE; login-url=https://jira.example.com/login.jsp",
        )]);

        assert!(matches!(
            status_error(403, &denied, b"", false),
            Some(FetchError::Unauthorized {
                basic: false,
                cloud: false,
                scope: false,
            })
        ));
    }

    #[test]
    fn unauthorized_names_the_email_only_in_basic_mode() {
        let basic = FetchError::Unauthorized {
            basic: true,
            cloud: false,
            scope: false,
        }
        .to_string();
        assert!(basic.contains(ENV_JIRA_TOKEN), "{basic}");
        assert!(basic.contains(ENV_JIRA_EMAIL), "{basic}");

        let bearer = FetchError::Unauthorized {
            basic: false,
            cloud: false,
            scope: false,
        }
        .to_string();
        assert!(bearer.contains(ENV_JIRA_TOKEN), "{bearer}");
        assert!(!bearer.contains(ENV_JIRA_EMAIL), "{bearer}");
    }

    const CLOUD_BEARER_BODY: &[u8] = br#"{"error": "Failed to parse Connect Session Auth Token"}"#;

    #[test]
    fn cloud_connect_token_error_is_unauthorized_not_unavailable() {
        let none = HeaderMap::new();

        assert!(matches!(
            status_error(403, &none, CLOUD_BEARER_BODY, false),
            Some(FetchError::Unauthorized {
                basic: false,
                cloud: true,
                scope: false,
            })
        ));
        assert!(matches!(
            status_error(403, &none, br#"{"errorMessages":["no permission"]}"#, false),
            Some(FetchError::Unavailable { status: 403 })
        ));
        assert!(matches!(
            status_error(401, &none, CLOUD_BEARER_BODY, false),
            Some(FetchError::Unauthorized {
                basic: false,
                cloud: true,
                scope: false,
            })
        ));
    }

    const SCOPE_MISMATCH_BODY: &[u8] =
        br#"{"code":401,"message":"Unauthorized; scope does not match"}"#;

    #[test]
    fn scope_mismatch_401_sets_scope_on_fetches_and_the_auth_check() {
        let none = HeaderMap::new();

        for map in [status_error, auth_check_error] {
            assert!(matches!(
                map(401, &none, SCOPE_MISMATCH_BODY, true),
                Some(FetchError::Unauthorized {
                    basic: true,
                    cloud: false,
                    scope: true,
                })
            ));
            assert!(matches!(
                map(401, &none, br#"{"message":"Unauthorized"}"#, true),
                Some(FetchError::Unauthorized { scope: false, .. })
            ));
        }
        assert!(matches!(
            auth_check_error(403, &none, SCOPE_MISMATCH_BODY, true),
            Some(FetchError::Unauthorized { scope: false, .. })
        ));
    }

    #[test]
    fn scope_mismatch_message_names_the_scope() {
        for basic in [true, false] {
            let message = FetchError::Unauthorized {
                basic,
                cloud: false,
                scope: true,
            }
            .to_string();
            assert!(message.contains("lacks the scope"), "{message}");
            assert!(message.contains("read:jira-work"), "{message}");
        }
    }

    #[test]
    fn auth_check_treats_any_401_or_403_as_unauthorized() {
        let none = HeaderMap::new();

        assert!(auth_check_error(200, &none, b"{}", false).is_none());
        for status in [401, 403] {
            assert!(matches!(
                auth_check_error(status, &none, b"", true),
                Some(FetchError::Unauthorized {
                    basic: true,
                    cloud: false,
                    scope: false,
                })
            ));
        }
        assert!(matches!(
            auth_check_error(403, &none, CLOUD_BEARER_BODY, false),
            Some(FetchError::Unauthorized {
                basic: false,
                cloud: true,
                scope: false,
            })
        ));
        assert!(matches!(
            auth_check_error(404, &none, b"", false),
            Some(FetchError::Unavailable { status: 404 })
        ));
        assert!(matches!(
            auth_check_error(503, &none, b"", false),
            Some(FetchError::Status { status: 503 })
        ));
        assert!(matches!(
            auth_check_error(429, &none, b"", false),
            Some(FetchError::RateLimited { .. })
        ));
    }

    #[test]
    fn cloud_bearer_rejection_hints_at_the_email() {
        let message = FetchError::Unauthorized {
            basic: false,
            cloud: true,
            scope: false,
        }
        .to_string();

        assert!(message.contains("Jira Cloud"), "{message}");
        assert!(message.contains(ENV_JIRA_EMAIL), "{message}");
        assert!(message.contains(ENV_JIRA_TOKEN), "{message}");
        assert!(
            message.contains("https://api.atlassian.com/ex/jira/<cloudId>"),
            "{message}"
        );
        assert!(message.contains("read:jira-work"), "{message}");
    }

    #[test]
    fn issue_url_requests_rendered_fields() {
        assert_eq!(
            issue_url("https://jira.example.com/", "GRLD-1"),
            "https://jira.example.com/rest/api/2/issue/GRLD-1?expand=renderedFields"
        );
        assert_eq!(
            issue_url("https://example.com/jira", "GRLD-1"),
            "https://example.com/jira/rest/api/2/issue/GRLD-1?expand=renderedFields"
        );
    }

    #[test]
    fn request_carries_url_accept_and_auth() {
        let mut config = jira(Some("t"), Some("a@example.com"));
        config.base_url = " https://jira.example.com/ ".to_string();
        let request = JiraClient::new(&config).unwrap().request("GRLD-1").unwrap();

        assert_eq!(request.method(), reqwest::Method::GET);
        assert_eq!(
            request.url().as_str(),
            "https://jira.example.com/rest/api/2/issue/GRLD-1?expand=renderedFields"
        );
        assert_eq!(request.headers()[ACCEPT], "application/json");
        assert_eq!(
            request.headers()[reqwest::header::AUTHORIZATION],
            "Basic YUBleGFtcGxlLmNvbTp0"
        );

        let request = JiraClient::new(&jira(Some("t"), None))
            .unwrap()
            .request("GRLD-1")
            .unwrap();
        assert_eq!(
            request.headers()[reqwest::header::AUTHORIZATION],
            "Bearer t"
        );
    }

    #[test]
    fn auth_check_requests_myself_with_the_auth_header() {
        let mut config = jira(Some("secret"), None);
        config.base_url = "https://example.com/jira/".to_string();
        let request = JiraClient::new(&config).unwrap().myself_request().unwrap();

        assert_eq!(request.method(), reqwest::Method::GET);
        assert_eq!(
            request.url().as_str(),
            "https://example.com/jira/rest/api/2/myself"
        );
        assert_eq!(request.headers()[ACCEPT], "application/json");
        assert_eq!(
            request.headers()[reqwest::header::AUTHORIZATION],
            "Bearer secret"
        );

        let request = JiraClient::new(&jira(Some("t"), Some("a@example.com")))
            .unwrap()
            .myself_request()
            .unwrap();
        assert_eq!(
            request.url().as_str(),
            "https://jira.example.com/rest/api/2/myself"
        );
        assert_eq!(
            request.headers()[reqwest::header::AUTHORIZATION],
            "Basic YUBleGFtcGxlLmNvbTp0"
        );
    }

    #[test]
    fn client_validates_base_url() {
        let mut config = jira(Some("t"), None);
        for base_url in ["", "  ", "jira.example.com", "ftp://jira.example.com"] {
            config.base_url = base_url.to_string();
            assert!(JiraClient::new(&config).is_err(), "{base_url:?}");
        }

        config.base_url = "  https://jira.example.com/jira  ".to_string();
        let client = JiraClient::new(&config).unwrap();
        assert_eq!(client.base_url, "https://jira.example.com/jira");
    }

    #[tokio::test]
    async fn client_requires_a_token_and_rejects_odd_keys() {
        assert!(JiraClient::new(&jira(None, None)).is_err());

        let client = JiraClient::new(&jira(Some("t"), None)).unwrap();
        for key in ["", "GRLD-1/../2", "GRLD-1?x=1"] {
            let err = client.fetch(key).await.unwrap_err();
            assert!(matches!(err, FetchError::InvalidKey(_)), "{key}: {err}");
        }
    }

    /// Manual end-to-end check against a real Jira (audit §3.1). Needs
    /// `ANNATAR_JIRA_TOKEN` (plus `ANNATAR_JIRA_EMAIL` on Cloud),
    /// `ANNATAR_JIRA_TEST_KEY`, and a base URL from `ANNATAR_JIRA_URL` or the
    /// `[jira]` section of `annatar.toml`.
    #[tokio::test]
    #[ignore = "needs a real Jira and a token"]
    async fn fetches_real_ticket() {
        let key = std::env::var("ANNATAR_JIRA_TEST_KEY").expect("set ANNATAR_JIRA_TEST_KEY");
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("annatar.toml");
        let file_jira = Config::load(&path).ok().and_then(|config| config.jira);
        let base_url = std::env::var("ANNATAR_JIRA_URL")
            .ok()
            .or_else(|| file_jira.as_ref().map(|jira| jira.base_url.clone()))
            .expect("set ANNATAR_JIRA_URL or [jira] base_url in annatar.toml");
        let config = JiraConfig {
            base_url,
            token: std::env::var(ENV_JIRA_TOKEN).ok(),
            email: std::env::var(ENV_JIRA_EMAIL).ok(),
            epic_link_field: file_jira.and_then(|jira| jira.epic_link_field),
            concurrency: crate::config::DEFAULT_JIRA_CONCURRENCY,
        };

        let client = JiraClient::new(&config).unwrap();
        let mode = match client.auth {
            Auth::Basic { .. } => "basic (Cloud)",
            Auth::Bearer { .. } => "bearer (Server/DC)",
        };
        match client.check_auth().await {
            Ok(()) | Err(FetchError::Unauthorized { scope: true, .. }) => {}
            Err(error) => panic!("{error}"),
        }
        let ticket = client.fetch(&key).await.unwrap();

        assert!(!ticket.key.is_empty());
        if ticket.key != key {
            println!(
                "requested {key}, Jira returned {} (moved issue?)",
                ticket.key
            );
        }
        assert!(!ticket.issue_type.is_empty());
        assert!(!ticket.summary.trim().is_empty());
        if let Some(description) = &ticket.description {
            assert!(!description.contains("</"), "description still has HTML");
        }
        println!(
            "auth: {mode}; key: {}; type: {}; parent: {:?}; summary: {} chars; description: {} chars",
            ticket.key,
            ticket.issue_type,
            ticket.parent_key,
            ticket.summary.chars().count(),
            ticket
                .description
                .as_deref()
                .map_or(0, |text| text.chars().count()),
        );
    }
}
