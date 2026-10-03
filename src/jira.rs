//! Fetch one Jira issue and parse it into a [`Ticket`].
//!
//! The client calls `GET /rest/api/2/issue/{key}?expand=renderedFields`.
//! REST v2 works on both Cloud and Server/DC, and `renderedFields` carries the
//! description as HTML on both, so [`html_to_text`] turns it into plain text
//! without an Atlassian Document Format or wiki-markup parser.
//!
//! Auth: with an email configured ([`ENV_JIRA_EMAIL`]) requests use basic auth
//! `email:token` (Cloud API token); without one they send the token as a bearer
//! personal access token (Server/DC).
//!
//! The parent key comes from `fields.parent.key` (sub-tasks, and epic children
//! on Cloud). An optional `jira.epic_link_field` names a custom field that
//! holds the epic key on Server/DC; it is read only when there is no parent
//! (or its key is empty).
//!
//! [`FetchError`] keeps the HTTP outcomes 3.2 needs apart: bad credentials,
//! an unavailable issue, rate limiting and everything else. A 403 carrying
//! `X-Authentication-Denied-Reason` (Server/DC CAPTCHA after failed logins)
//! counts as bad credentials. [`TicketSource`] is the seam the ticket stage
//! fetches through; caching, retries and concurrency live in
//! [`crate::tickets`] and the index build, not here.

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

/// Wrap width handed to the HTML converter; wide enough that paragraphs stay
/// on one line.
const TEXT_WIDTH: usize = 10_000;

/// A Jira issue reduced to what the summaries need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ticket {
    /// Issue key as Jira reports it (a moved issue reports its new key).
    pub key: String,
    /// Issue type name, e.g. `Story`, `Bug`, `Sub-task`.
    pub issue_type: String,
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
    let summary =
        string_at(json, "/fields/summary").with_context(|| format!("{key}: no summary"))?;
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
    /// 401, or 403 with `X-Authentication-Denied-Reason`: credentials missing
    /// or wrong. Must never be cached. `basic` names the auth mode for the
    /// message.
    Unauthorized { basic: bool },
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
    /// A success response that is not a parseable issue.
    Parse(anyhow::Error),
}

impl fmt::Display for FetchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unauthorized { basic: true } => write!(
                f,
                "Jira rejected the credentials; check {ENV_JIRA_TOKEN} and {ENV_JIRA_EMAIL}"
            ),
            Self::Unauthorized { basic: false } => {
                write!(f, "Jira rejected the credentials; check {ENV_JIRA_TOKEN}")
            }
            Self::Unavailable { status } => write!(f, "issue unavailable (HTTP {status})"),
            Self::RateLimited {
                retry_after: Some(wait),
            } => write!(f, "rate limited (429), retry after {}s", wait.as_secs()),
            Self::RateLimited { retry_after: None } => write!(f, "rate limited (429)"),
            Self::Status { status } => write!(f, "Jira returned HTTP {status}"),
            Self::InvalidKey(key) => write!(f, "not a Jira key: {key:?}"),
            Self::Transport(err) => write!(f, "Jira request failed: {err}"),
            Self::Parse(err) => write!(f, "unexpected Jira response: {err:#}"),
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

/// Map a non-success status and its headers to a [`FetchError`]; `None` for
/// 2xx. `basic` is the auth mode, carried into [`FetchError::Unauthorized`].
fn status_error(status: u16, headers: &HeaderMap, basic: bool) -> Option<FetchError> {
    match status {
        200..=299 => None,
        401 => Some(FetchError::Unauthorized { basic }),
        403 if headers.contains_key(AUTH_DENIED_REASON) => Some(FetchError::Unauthorized { basic }),
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

/// The issue URL for `key` under `base_url`.
fn issue_url(base_url: &str, key: &str) -> String {
    format!(
        "{}/rest/api/2/issue/{key}?expand=renderedFields",
        base_url.trim_end_matches('/')
    )
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
        let request = self.request(key)?;
        let response = self
            .http
            .execute(request)
            .await
            .map_err(FetchError::Transport)?;
        let basic = matches!(self.auth, Auth::Basic { .. });
        if let Some(err) = status_error(response.status().as_u16(), response.headers(), basic) {
            return Err(err);
        }
        let body = response.bytes().await.map_err(FetchError::Transport)?;
        let json: Value =
            serde_json::from_slice(&body).map_err(|err| FetchError::Parse(anyhow!(err)))?;
        parse_issue(&json, self.epic_link_field.as_deref()).map_err(FetchError::Parse)
    }

    /// The authenticated issue request for `key`, built but not sent.
    fn request(&self, key: &str) -> Result<reqwest::Request, FetchError> {
        if !is_plain_key(key) {
            return Err(FetchError::InvalidKey(key.to_string()));
        }
        let request = self
            .http
            .get(issue_url(&self.base_url, key))
            .header(ACCEPT, "application/json");
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

/// Where the ticket stage gets issues from: [`JiraClient`] in production, a
/// scripted fake in tests. One call is one Jira request.
pub trait TicketSource: Send + Sync {
    /// Fetch and parse the issue `key`.
    fn fetch<'a>(&'a self, key: &'a str) -> FetchFuture<'a>;
}

impl TicketSource for JiraClient {
    fn fetch<'a>(&'a self, key: &'a str) -> FetchFuture<'a> {
        Box::pin(JiraClient::fetch(self, key))
    }
}

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
    fn parses_a_story_with_lists_and_tables() {
        let ticket = parse_issue(&fixture("story"), None).unwrap();

        assert_eq!(ticket.key, "GRLD-101");
        assert_eq!(ticket.issue_type, "Story");
        assert_eq!(ticket.summary, "Users can reset their password by email");
        assert_eq!(ticket.parent_key, None);
        let description = ticket.description.expect("story has a description");
        assert!(
            description.starts_with("As a user I want to reset my password"),
            "{description}"
        );
        assert!(
            description.contains("* A reset link is sent to the registered address"),
            "{description}"
        );
        assert!(description.contains("single use"), "{description}");
        assert!(!description.contains('<'), "HTML left in: {description}");
        assert!(!description.contains("**"), "{description}");
    }

    #[test]
    fn parses_a_bug_with_code_and_links() {
        let ticket = parse_issue(&fixture("bug"), None).unwrap();

        assert_eq!(ticket.key, "GRLD-214");
        assert_eq!(ticket.issue_type, "Bug");
        let description = ticket.description.unwrap();
        assert!(
            description.contains(
                "java.lang.NullPointerException: address is null\n    at com.acme.order.OrderExporter.export"
            ),
            "{description}"
        );
        assert!(description.contains("the export spec"), "{description}");
        assert!(!description.contains("<span"), "{description}");
        assert!(!description.contains("wiki.example.com"), "{description}");
    }

    #[test]
    fn sub_task_takes_its_parent_key() {
        let ticket = parse_issue(&fixture("subtask"), None).unwrap();

        assert_eq!(ticket.issue_type, "Sub-task");
        assert_eq!(ticket.parent_key.as_deref(), Some("GRLD-101"));
        assert_eq!(
            ticket.description.as_deref(),
            Some("Store expires_at next to the token and reject expired tokens.")
        );
    }

    #[test]
    fn epic_link_field_is_used_only_when_configured() {
        let json = fixture("epic_child");

        let ticket = parse_issue(&json, None).unwrap();
        assert_eq!(ticket.parent_key, None);

        let ticket = parse_issue(&json, Some("customfield_10100")).unwrap();
        assert_eq!(ticket.parent_key.as_deref(), Some("GRLD-300"));

        let ticket = parse_issue(&json, Some("customfield_99999")).unwrap();
        assert_eq!(ticket.parent_key, None);
    }

    #[test]
    fn epic_link_field_may_hold_an_object_with_a_key() {
        let mut json = fixture("epic_child");
        json["fields"]["customfield_10100"] = serde_json::json!({ "key": "GRLD-301" });

        let ticket = parse_issue(&json, Some("customfield_10100")).unwrap();
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

        let ticket = parse_issue(&json, Some("customfield_10100")).unwrap();
        assert_eq!(ticket.parent_key.as_deref(), Some("GRLD-300"));
    }

    #[test]
    fn parent_wins_over_epic_link_field() {
        let mut json = fixture("subtask");
        json["fields"]["customfield_10100"] = Value::from("GRLD-300");

        let ticket = parse_issue(&json, Some("customfield_10100")).unwrap();
        assert_eq!(ticket.parent_key.as_deref(), Some("GRLD-101"));
    }

    #[test]
    fn null_or_blank_description_is_none() {
        let mut json = fixture("empty_description");
        let ticket = parse_issue(&json, None).unwrap();
        assert_eq!(ticket.key, "GRLD-412");
        assert_eq!(ticket.issue_type, "Task");
        assert_eq!(ticket.description, None);

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
            format!("{err:#}").contains("GRLD-101: no summary"),
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
        assert!(status_error(200, &none, false).is_none());
        assert!(matches!(
            status_error(401, &none, true),
            Some(FetchError::Unauthorized { basic: true })
        ));
        assert!(matches!(
            status_error(403, &none, false),
            Some(FetchError::Unavailable { status: 403 })
        ));
        assert!(matches!(
            status_error(404, &none, false),
            Some(FetchError::Unavailable { status: 404 })
        ));
        assert!(matches!(
            status_error(429, &headers(&[("retry-after", " 17 ")]), false),
            Some(FetchError::RateLimited { retry_after: Some(wait) }) if wait == Duration::from_secs(17)
        ));
        assert!(matches!(
            status_error(
                429,
                &headers(&[("retry-after", "Wed, 21 Oct 2026 07:28:00 GMT")]),
                false
            ),
            Some(FetchError::RateLimited { retry_after: None })
        ));
        assert!(matches!(
            status_error(500, &none, false),
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
            status_error(403, &denied, false),
            Some(FetchError::Unauthorized { basic: false })
        ));
    }

    #[test]
    fn unauthorized_names_the_email_only_in_basic_mode() {
        let basic = FetchError::Unauthorized { basic: true }.to_string();
        assert!(basic.contains(ENV_JIRA_TOKEN), "{basic}");
        assert!(basic.contains(ENV_JIRA_EMAIL), "{basic}");

        let bearer = FetchError::Unauthorized { basic: false }.to_string();
        assert!(bearer.contains(ENV_JIRA_TOKEN), "{bearer}");
        assert!(!bearer.contains(ENV_JIRA_EMAIL), "{bearer}");
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
