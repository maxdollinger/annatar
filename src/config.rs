use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use regex::Regex;
use serde::{Deserialize, Deserializer};

/// Default regex used to find Jira ticket keys in commit messages.
pub const DEFAULT_TICKET_REGEX: &str = r"\bGRLD-\d+";

/// Default URL of the local Ollama server.
pub const DEFAULT_OLLAMA_URL: &str = "http://localhost:11434";

/// Default reasoning effort sent with chat requests: thinking off.
pub const DEFAULT_REASONING_EFFORT: &str = "none";

/// Accepted `ollama.reasoning_effort` values; empty means the field is not
/// sent and the model decides.
pub const REASONING_EFFORTS: &[&str] = &["", "none", "low", "medium", "high"];

/// Default sampling temperature sent with chat requests: greedy, so a
/// regenerated record matches the cached one.
pub const DEFAULT_TEMPERATURE: f64 = 0.0;

/// Highest accepted `ollama.temperature`.
pub const MAX_TEMPERATURE: f64 = 2.0;

/// Default `max_tokens` sent with chat requests: a safety cap on runaway
/// generation, far above any real reply (the longest description so far is
/// about 130 tokens), not a length limit.
pub const DEFAULT_MAX_TOKENS: u32 = 1024;

/// Default number of Jira requests in flight during the ticket stage.
pub const DEFAULT_JIRA_CONCURRENCY: usize = 4;

/// Default number of most recent tickets (besides the first one) in a
/// member's description prompt.
pub const DEFAULT_RECENT_TICKETS: usize = 3;

/// Default number of commit subjects in a member's or type's description
/// prompt.
pub const DEFAULT_COMMIT_SUBJECTS: usize = 5;

/// Default number of characters of a member's source in its description
/// prompt.
pub const DEFAULT_BODY_CHARS: usize = 1500;

/// Default number of members and nested types listed in a type's
/// description prompt (public ones first).
pub const DEFAULT_TYPE_MEMBERS: usize = 30;

/// Default number of most recent tickets (besides the first one) in a type's
/// description prompt.
pub const DEFAULT_TYPE_RECENT_TICKETS: usize = 3;

/// Environment variable holding the Jira API token.
pub const ENV_JIRA_TOKEN: &str = "ANNATAR_JIRA_TOKEN";

/// Environment variable holding the Jira account email (for basic auth).
pub const ENV_JIRA_EMAIL: &str = "ANNATAR_JIRA_EMAIL";

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Root of the repository to index.
    pub repo: PathBuf,
    /// Directory holding the libSQL files `index.db` and `cache.db`.
    pub data_dir: PathBuf,
    /// Regex used to extract ticket keys from history, compiled once while the
    /// config is parsed so a bad pattern fails before any work starts.
    #[serde(
        default = "default_ticket_regex",
        deserialize_with = "deserialize_regex"
    )]
    pub ticket_regex: Regex,
    /// Jira connection. Optional: without it `index` uses cached tickets only.
    #[serde(default)]
    pub jira: Option<JiraConfig>,
    /// Ollama connection. Optional for now; a command that calls the LLM will
    /// require it once Phase 4 lands.
    #[serde(default)]
    pub ollama: Option<OllamaConfig>,
    /// What goes into the method, constructor and type description prompts.
    #[serde(default)]
    pub describe: DescribeConfig,
}

/// How much context a method, constructor or type description prompt carries.
/// Every value changes the prompts, so changing one misses the LLM cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct DescribeConfig {
    /// Most recent tickets shown besides the first (oldest) one.
    pub recent_tickets: usize,
    /// Most recent commit subjects shown (always, besides the tickets).
    pub commit_subjects: usize,
    /// Characters of the member's source (declaration and body), or of a
    /// type's declaration with its members cut out, shown; `0` leaves the
    /// source out (signature and Javadoc only).
    pub body_chars: usize,
    /// Members and nested types listed with their description in a type's
    /// prompt, public ones first; the rest are only counted.
    pub type_members: usize,
    /// Most recent tickets shown besides the first one in a type's prompt (a
    /// type's history spans its whole body, so its tickets are about the
    /// file's).
    pub type_recent_tickets: usize,
}

impl Default for DescribeConfig {
    fn default() -> Self {
        Self {
            recent_tickets: DEFAULT_RECENT_TICKETS,
            commit_subjects: DEFAULT_COMMIT_SUBJECTS,
            body_chars: DEFAULT_BODY_CHARS,
            type_members: DEFAULT_TYPE_MEMBERS,
            type_recent_tickets: DEFAULT_TYPE_RECENT_TICKETS,
        }
    }
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JiraConfig {
    /// Base URL of the Jira instance, e.g. `https://acme.atlassian.net`.
    pub base_url: String,
    /// API token, never read from the config file. See [`ENV_JIRA_TOKEN`].
    #[serde(skip)]
    pub token: Option<String>,
    /// Account email, never read from the config file. See [`ENV_JIRA_EMAIL`].
    /// When set, requests use basic auth (Cloud); otherwise a bearer token
    /// (Server/DC personal access token).
    #[serde(skip)]
    pub email: Option<String>,
    /// Custom field holding the epic link (e.g. `customfield_10100` on
    /// Server/DC), used when an issue has no `parent`. Unset = ignored.
    #[serde(default)]
    pub epic_link_field: Option<String>,
    /// Maximum Jira requests in flight during the ticket stage; at least 1.
    #[serde(default = "default_jira_concurrency")]
    pub concurrency: usize,
}

impl std::fmt::Debug for JiraConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JiraConfig")
            .field("base_url", &self.base_url)
            .field("token", &self.token.as_ref().map(|_| "***"))
            .field("email", &self.email)
            .field("epic_link_field", &self.epic_link_field)
            .field("concurrency", &self.concurrency)
            .finish()
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OllamaConfig {
    /// OpenAI-compatible endpoint of the Ollama server.
    #[serde(default = "default_ollama_url")]
    pub url: String,
    /// Chat model used for ticket and symbol summaries.
    pub chat_model: String,
    /// Embedding model used for semantic search.
    pub embedding_model: String,
    /// `reasoning_effort` sent with every chat request (one of
    /// [`REASONING_EFFORTS`]). `none` turns a thinking model's reasoning off;
    /// empty leaves it to the model.
    #[serde(
        default = "default_reasoning_effort",
        deserialize_with = "deserialize_reasoning_effort"
    )]
    pub reasoning_effort: String,
    /// Sampling `temperature` sent with every chat request, `0.0` to
    /// [`MAX_TEMPERATURE`]. Part of the LLM cache key.
    #[serde(
        default = "default_temperature",
        deserialize_with = "deserialize_temperature"
    )]
    pub temperature: f64,
    /// `max_tokens` sent with every chat request: a safety cap that stops a
    /// looping reply (cut off at the cap, so the reply is invalid). `0` does
    /// not send it. With a `reasoning_effort` other than `none`, the
    /// reasoning counts against it too. Not part of the LLM cache key: only
    /// complete replies are cached, and a cap does not change them.
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
}

impl Config {
    /// Parse a configuration from TOML text. Pure: never touches the
    /// environment, so tests are deterministic.
    pub fn parse(text: &str) -> Result<Self> {
        toml::from_str(text).context("parsing configuration TOML")
    }

    /// Read, parse and validate a configuration file, then resolve relative
    /// paths against the file's own directory and load secrets from the
    /// environment.
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading config file {}", path.display()))?;
        let mut config =
            Self::parse(&text).with_context(|| format!("in config file {}", path.display()))?;
        let base = config_dir(path);
        config.repo = resolve_path(&base, config.repo.clone());
        config.data_dir = resolve_path(&base, config.data_dir.clone());
        config.apply_env();
        Ok(config)
    }

    /// Populate secret fields from environment variables. Missing variables
    /// leave the field empty; a command that needs Jira auth fails later.
    pub fn apply_env(&mut self) {
        if let Some(jira) = self.jira.as_mut() {
            if let Ok(token) = std::env::var(ENV_JIRA_TOKEN) {
                jira.token = Some(token);
            }
            if let Ok(email) = std::env::var(ENV_JIRA_EMAIL) {
                jira.email = Some(email);
            }
        }
    }
}

fn default_ticket_regex() -> Regex {
    Regex::new(DEFAULT_TICKET_REGEX).expect("the default ticket regex is valid")
}

fn deserialize_regex<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Regex, D::Error> {
    let pattern = String::deserialize(deserializer)?;
    Regex::new(&pattern)
        .map_err(|err| serde::de::Error::custom(format!("invalid ticket_regex {pattern:?}: {err}")))
}

fn default_reasoning_effort() -> String {
    DEFAULT_REASONING_EFFORT.to_string()
}

fn deserialize_reasoning_effort<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<String, D::Error> {
    let effort = String::deserialize(deserializer)?;
    if REASONING_EFFORTS.contains(&effort.as_str()) {
        Ok(effort)
    } else {
        Err(serde::de::Error::custom(format!(
            "invalid ollama.reasoning_effort {effort:?}: expected one of {REASONING_EFFORTS:?}"
        )))
    }
}

fn default_temperature() -> f64 {
    DEFAULT_TEMPERATURE
}

fn deserialize_temperature<'de, D: Deserializer<'de>>(deserializer: D) -> Result<f64, D::Error> {
    let temperature = f64::deserialize(deserializer)?;
    if (0.0..=MAX_TEMPERATURE).contains(&temperature) {
        Ok(temperature)
    } else {
        Err(serde::de::Error::custom(format!(
            "invalid ollama.temperature {temperature}: expected 0.0 to {MAX_TEMPERATURE}"
        )))
    }
}

fn default_max_tokens() -> u32 {
    DEFAULT_MAX_TOKENS
}

fn default_jira_concurrency() -> usize {
    DEFAULT_JIRA_CONCURRENCY
}

fn default_ollama_url() -> String {
    DEFAULT_OLLAMA_URL.to_string()
}

/// The directory a config file lives in, used as the base for relative paths.
/// A bare filename with no parent resolves to the current directory.
fn config_dir(path: &Path) -> PathBuf {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

/// Resolve a possibly-relative config path against the config file's
/// directory. Absolute paths are returned unchanged.
fn resolve_path(base: &Path, path: PathBuf) -> PathBuf {
    if path.is_relative() {
        base.join(path)
    } else {
        path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL: &str = r#"
repo = "/tmp/repo"
data_dir = "data"
ticket_regex = ""

[jira]
base_url = "https://example.atlassian.net"

[ollama]
url = ""
chat_model = "qwen2.5-coder"
embedding_model = "nomic-embed-text"
"#;

    fn minimal_without(lines: &[&str]) -> String {
        let mut text = MINIMAL.to_string();
        for line in lines {
            assert!(
                text.contains(line),
                "MINIMAL no longer contains {line:?}; the test would be vacuous"
            );
            text = text.replace(line, "");
            assert!(
                !text.contains(line),
                "{line:?} is still present after removal; the test would be vacuous"
            );
        }
        text
    }

    #[test]
    fn defaults_are_applied_when_omitted() {
        let text = minimal_without(&[r#"ticket_regex = """#, "url = \"\""]);
        let config = Config::parse(&text).expect("config should parse");

        assert_eq!(config.ticket_regex.as_str(), DEFAULT_TICKET_REGEX);
        let ollama = config.ollama.expect("section present");
        assert_eq!(ollama.url, DEFAULT_OLLAMA_URL);
        assert_eq!(ollama.reasoning_effort, DEFAULT_REASONING_EFFORT);
        assert_eq!(ollama.temperature, DEFAULT_TEMPERATURE);
        assert_eq!(ollama.max_tokens, DEFAULT_MAX_TOKENS);
        assert_eq!(config.describe, DescribeConfig::default());
        assert_eq!(config.describe.recent_tickets, DEFAULT_RECENT_TICKETS);
        assert_eq!(config.describe.commit_subjects, DEFAULT_COMMIT_SUBJECTS);
        assert_eq!(config.describe.body_chars, DEFAULT_BODY_CHARS);
        assert_eq!(config.describe.type_members, DEFAULT_TYPE_MEMBERS);
        assert_eq!(
            config.describe.type_recent_tickets,
            DEFAULT_TYPE_RECENT_TICKETS
        );
    }

    #[test]
    fn describe_settings_are_optional_per_key_and_strict() {
        let config = Config::parse(&format!(
            "{MINIMAL}\n[describe]\nbody_chars = 0\ntype_members = 5\n"
        ))
        .expect("config should parse");
        assert_eq!(
            config.describe,
            DescribeConfig {
                body_chars: 0,
                type_members: 5,
                ..DescribeConfig::default()
            }
        );
        let err = Config::parse(&format!("{MINIMAL}\n[describe]\nrecent = 2\n"))
            .expect_err("unknown key");
        assert!(format!("{err:#}").contains("recent"), "{err:#}");
    }

    #[test]
    fn reasoning_effort_is_validated() {
        let with = |effort: &str| {
            MINIMAL.replace(
                "embedding_model = \"nomic-embed-text\"",
                &format!("embedding_model = \"nomic-embed-text\"\nreasoning_effort = \"{effort}\""),
            )
        };
        for effort in REASONING_EFFORTS {
            let config = Config::parse(&with(effort)).expect("valid effort should parse");
            assert_eq!(config.ollama.unwrap().reasoning_effort, *effort);
        }
        let err = Config::parse(&with("max")).expect_err("unknown effort should fail");
        assert!(
            format!("{err:#}").contains("invalid ollama.reasoning_effort \"max\""),
            "error should name the bad value, got: {err:#}"
        );
    }

    #[test]
    fn temperature_is_validated() {
        let with = |temperature: &str| {
            MINIMAL.replace(
                "embedding_model = \"nomic-embed-text\"",
                &format!("embedding_model = \"nomic-embed-text\"\ntemperature = {temperature}"),
            )
        };
        for (text, value) in [("0.0", 0.0), ("0.7", 0.7), ("2", 2.0)] {
            let config = Config::parse(&with(text)).expect("valid temperature should parse");
            assert_eq!(config.ollama.unwrap().temperature, value);
        }
        for bad in ["-0.1", "2.5"] {
            let err = Config::parse(&with(bad)).expect_err("out-of-range temperature should fail");
            assert!(
                format!("{err:#}").contains("invalid ollama.temperature"),
                "error should name the field, got: {err:#}"
            );
        }
    }

    #[test]
    fn jira_and_ollama_are_optional() {
        let text = r#"
repo = "/tmp/repo"
data_dir = "data"
"#;
        let config = Config::parse(text).expect("config without jira/ollama should parse");

        assert!(config.jira.is_none(), "jira should default to None");
        assert!(config.ollama.is_none(), "ollama should default to None");
        assert_eq!(config.ticket_regex.as_str(), DEFAULT_TICKET_REGEX);
    }

    #[test]
    fn shipped_config_uses_the_default_ticket_regex() {
        let config = Config::parse(include_str!("../annatar.toml")).expect("annatar.toml parses");
        assert_eq!(config.ticket_regex.as_str(), DEFAULT_TICKET_REGEX);
    }

    #[test]
    fn epic_link_field_is_optional() {
        let config = Config::parse(MINIMAL).expect("config should parse");
        let jira = config.jira.expect("section present");
        assert_eq!(jira.epic_link_field, None);
        assert_eq!(jira.concurrency, DEFAULT_JIRA_CONCURRENCY);

        let text = MINIMAL.replace(
            r#"base_url = "https://example.atlassian.net""#,
            "base_url = \"https://example.atlassian.net\"\nepic_link_field = \"customfield_10100\"",
        );
        let config = Config::parse(&text).expect("config should parse");
        assert_eq!(
            config
                .jira
                .expect("section present")
                .epic_link_field
                .as_deref(),
            Some("customfield_10100")
        );
    }

    #[test]
    fn missing_required_value_is_an_error() {
        let text = minimal_without(&[r#"repo = "/tmp/repo""#]);
        let err = Config::parse(&text).expect_err("missing repo should fail");

        assert!(
            format!("{err:#}").contains("repo"),
            "error should mention the missing field, got: {err:#}"
        );
    }

    #[test]
    fn invalid_ticket_regex_is_an_error() {
        let text = MINIMAL.replace(r#"ticket_regex = """#, r#"ticket_regex = "(""#);
        let err = Config::parse(&text).expect_err("invalid regex should fail");

        assert!(
            format!("{err:#}").contains("invalid ticket_regex \"(\""),
            "error should name the bad pattern, got: {err:#}"
        );
    }

    #[test]
    fn secrets_are_not_read_from_the_config_file() {
        let text = MINIMAL.replace(
            r#"base_url = "https://example.atlassian.net""#,
            "base_url = \"https://example.atlassian.net\"\ntoken = \"nope\"",
        );
        let err = Config::parse(&text).expect_err("token in file should be rejected");

        assert!(
            format!("{err:#}").contains("token"),
            "error should mention the unknown field, got: {err:#}"
        );
    }

    #[test]
    fn load_reads_the_file_and_resolves_relative_paths_against_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("annatar.toml");
        std::fs::write(&path, MINIMAL).unwrap();

        let config = Config::load(&path).expect("config should load");
        // `repo` is absolute and stays as written; `data_dir` is relative and
        // resolves next to the config file, not the current directory.
        assert_eq!(config.repo, PathBuf::from("/tmp/repo"));
        assert_eq!(config.data_dir, dir.path().join("data"));
    }
}
