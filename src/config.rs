use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

/// Default regex used to find Jira ticket keys in commit messages.
pub const DEFAULT_TICKET_REGEX: &str = r"\bGRLD-\d+\b";

/// Default URL of the local Ollama server.
pub const DEFAULT_OLLAMA_URL: &str = "http://localhost:11434";

/// Environment variable holding the Jira API token.
pub const ENV_JIRA_TOKEN: &str = "ANNATAR_JIRA_TOKEN";

/// Environment variable holding the Jira account email (for basic auth).
pub const ENV_JIRA_EMAIL: &str = "ANNATAR_JIRA_EMAIL";

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Root of the repository to index.
    pub repo: PathBuf,
    /// Path to the libSQL database file.
    pub database: PathBuf,
    /// Regex used to extract ticket keys from history.
    #[serde(default = "default_ticket_regex")]
    pub ticket_regex: String,
    pub jira: JiraConfig,
    pub ollama: OllamaConfig,
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
    #[serde(skip)]
    pub email: Option<String>,
}

impl std::fmt::Debug for JiraConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JiraConfig")
            .field("base_url", &self.base_url)
            .field("token", &self.token.as_ref().map(|_| "***"))
            .field("email", &self.email)
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
    /// Dimension of the embedding vectors, must match the model.
    pub embedding_dim: usize,
}

impl Config {
    /// Parse a configuration from TOML text. Pure: never touches the
    /// environment, so tests are deterministic.
    pub fn parse(text: &str) -> Result<Self> {
        let config: Config = toml::from_str(text).context("parsing configuration TOML")?;
        config.validate()?;
        Ok(config)
    }

    /// Read, parse and validate a configuration file, then load secrets
    /// from the environment.
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading config file {}", path.display()))?;
        let mut config =
            Self::parse(&text).with_context(|| format!("in config file {}", path.display()))?;
        config.apply_env();
        Ok(config)
    }

    /// Populate secret fields from environment variables. Missing variables
    /// leave the field empty; a command that needs Jira auth fails later.
    pub fn apply_env(&mut self) {
        if let Ok(token) = std::env::var(ENV_JIRA_TOKEN) {
            self.jira.token = Some(token);
        }
        if let Ok(email) = std::env::var(ENV_JIRA_EMAIL) {
            self.jira.email = Some(email);
        }
    }

    fn validate(&self) -> Result<()> {
        regex::Regex::new(&self.ticket_regex)
            .with_context(|| format!("invalid ticket_regex {:?}", self.ticket_regex))?;
        if self.ollama.embedding_dim == 0 {
            bail!("ollama.embedding_dim must be greater than zero");
        }
        Ok(())
    }
}

fn default_ticket_regex() -> String {
    DEFAULT_TICKET_REGEX.to_string()
}

fn default_ollama_url() -> String {
    DEFAULT_OLLAMA_URL.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL: &str = r#"
repo = "/tmp/repo"
database = "annatar.db"
ticket_regex = ""

[jira]
base_url = "https://example.atlassian.net"

[ollama]
url = ""
chat_model = "qwen2.5-coder"
embedding_model = "nomic-embed-text"
embedding_dim = 768
"#;

    fn minimal_without(lines: &[&str]) -> String {
        let mut text = MINIMAL.to_string();
        for line in lines {
            text = text.replace(line, "");
        }
        text
    }

    #[test]
    fn defaults_are_applied_when_omitted() {
        let text = minimal_without(&[r#"ticket_regex = """#, "url = \"\""]);
        let config = Config::parse(&text).expect("config should parse");

        assert_eq!(config.ticket_regex, DEFAULT_TICKET_REGEX);
        assert_eq!(config.ollama.url, DEFAULT_OLLAMA_URL);
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
            err.to_string().contains("ticket_regex"),
            "error should mention ticket_regex, got: {err}"
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
    fn load_reads_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("annatar.toml");
        std::fs::write(&path, MINIMAL).unwrap();

        let config = Config::load(&path).expect("config should load");
        assert_eq!(config.repo, PathBuf::from("/tmp/repo"));
        assert_eq!(config.ollama.embedding_dim, 768);
    }
}
