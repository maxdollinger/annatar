use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
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
    /// Directory holding the libSQL files `index.db` and `cache.db`.
    pub data_dir: PathBuf,
    /// Regex used to extract ticket keys from history.
    #[serde(default = "default_ticket_regex")]
    pub ticket_regex: String,
    /// Jira connection. Optional for now; a command that talks to Jira will
    /// require it once Phase 3 lands.
    #[serde(default)]
    pub jira: Option<JiraConfig>,
    /// Ollama connection. Optional for now; a command that calls the LLM will
    /// require it once Phase 4 lands.
    #[serde(default)]
    pub ollama: Option<OllamaConfig>,
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
}

impl Config {
    /// Parse a configuration from TOML text. Pure: never touches the
    /// environment, so tests are deterministic.
    pub fn parse(text: &str) -> Result<Self> {
        let config: Config = toml::from_str(text).context("parsing configuration TOML")?;
        config.validate()?;
        Ok(config)
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
        config.repo = resolve_path(&base, std::mem::take(&mut config.repo));
        config.data_dir = resolve_path(&base, std::mem::take(&mut config.data_dir));
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

    fn validate(&self) -> Result<()> {
        regex::Regex::new(&self.ticket_regex)
            .with_context(|| format!("invalid ticket_regex {:?}", self.ticket_regex))?;
        Ok(())
    }
}

fn default_ticket_regex() -> String {
    DEFAULT_TICKET_REGEX.to_string()
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
            text = text.replace(line, "");
        }
        text
    }

    #[test]
    fn defaults_are_applied_when_omitted() {
        let text = minimal_without(&[r#"ticket_regex = """#, "url = \"\""]);
        let config = Config::parse(&text).expect("config should parse");

        assert_eq!(config.ticket_regex, DEFAULT_TICKET_REGEX);
        assert_eq!(
            config.ollama.expect("section present").url,
            DEFAULT_OLLAMA_URL
        );
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
        assert_eq!(config.ticket_regex, DEFAULT_TICKET_REGEX);
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
