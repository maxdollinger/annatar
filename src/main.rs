use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::builder::PossibleValuesParser;
use clap::{ArgAction, Parser, Subcommand};

use annatar::config::Config;
use annatar::search::{self, Filter, QueryEmbedder};
use annatar::store::{IndexReader, Store};
use annatar::summaries::Summarizer;
use annatar::symbols::{Role, SymbolKind};
use annatar::tickets::TicketFetch;
use annatar::{indexer, show, walk};

/// Index a codebase by intent: what each symbol does and why it exists.
#[derive(Debug, Parser)]
#[command(name = "annatar", version, about)]
struct Cli {
    /// Limit the run to paths under this prefix, for fast iteration. `index`
    /// still replaces the whole index.db, which then holds only this prefix;
    /// `search` returns only symbols in files under it.
    #[arg(long, global = true, value_name = "PREFIX")]
    path: Option<PathBuf>,

    /// Increase log verbosity (-v info, -vv debug, -vvv trace).
    #[arg(short, long, global = true, action = ArgAction::Count)]
    verbose: u8,

    /// Path to the configuration file.
    #[arg(
        long,
        global = true,
        value_name = "FILE",
        default_value = "annatar.toml"
    )]
    config: PathBuf,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Rebuild the index from the repository.
    Index {
        /// Make no Jira requests, even when Jira is configured; tickets
        /// already in cache.db are still used.
        #[arg(long)]
        offline: bool,
        /// Make no chat or embedding calls; summaries, descriptions and
        /// embeddings already in cache.db are still used, the rest get none.
        #[arg(long)]
        no_llm: bool,
    },
    /// Print a symbol and its children.
    Show {
        /// Fully qualified name, e.g. `com.acme.user.UserRepository`.
        fqn: String,
    },
    /// Search symbol descriptions by meaning: the most similar symbols, each
    /// as `rank. score fqn [kind] role=… file:start-end` with its description
    /// on the next line.
    Search {
        /// Plain-language query.
        query: String,
        /// Only symbols of this kind (repeatable).
        #[arg(long, value_name = "KIND", value_parser = PossibleValuesParser::new(SymbolKind::ALL.map(|kind| kind.as_str())))]
        kind: Vec<String>,
        /// Only types with this Spring role and their methods and
        /// constructors (repeatable).
        #[arg(long, value_name = "ROLE", value_parser = PossibleValuesParser::new(Role::ALL.map(|role| role.as_str())))]
        role: Vec<String>,
        /// Number of results.
        #[arg(short = 'k', long, value_name = "N", default_value_t = search::DEFAULT_LIMIT, value_parser = parse_limit)]
        limit: usize,
    },
}

/// `--path` for `search`: the prefix relative to the repository, with `/`
/// separators like `symbols.file`.
fn search_path(repo: &std::path::Path, prefix: &std::path::Path) -> Result<String> {
    let relative = walk::relative_prefix(repo, prefix).with_context(|| {
        format!(
            "--path {} is outside the repository {}",
            prefix.display(),
            repo.display()
        )
    })?;
    Ok(relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/"))
}

fn parse_limit(text: &str) -> Result<usize, String> {
    match text.parse::<usize>() {
        Ok(limit) if (1..=search::MAX_LIMIT).contains(&limit) => Ok(limit),
        _ => Err(format!("expected a number from 1 to {}", search::MAX_LIMIT)),
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    init_logging(cli.verbose);
    match run(&cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {}", format!("{err:#}").replace('\n', " "));
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: &Cli) -> Result<()> {
    let config = Config::load(&cli.config)?;
    tracing::debug!(
        repo = %config.repo.display(),
        data_dir = %config.data_dir.display(),
        path = ?cli.path,
        "loaded configuration"
    );

    match &cli.command {
        Command::Index { offline, no_llm } => {
            let jira = TicketFetch::from_config(config.jira.as_ref(), *offline)?;
            let store = Store::open(&config.data_dir).await?;
            let llm =
                Summarizer::from_config(config.ollama.as_ref(), config.describe, *no_llm, &store)?
                    .map(|llm| llm.with_embedding(config.embedding));
            let stats = indexer::build_index(
                &store,
                &config.repo,
                cli.path.as_deref(),
                &config.ticket_regex,
                &jira,
                llm.as_ref(),
            )
            .await?;
            println!(
                "indexed {} files, {} symbols, {} commits, {} tickets, {} history hits, {} history misses, {} history skipped, {} empty, {} parse errors, {} unreadable",
                stats.files,
                stats.symbols,
                stats.commits,
                stats.tickets,
                stats.history_hits,
                stats.history_misses,
                stats.history_skipped,
                stats.empty,
                stats.parse_errors,
                stats.unreadable
            );
            println!(
                "tickets: {} keys, {} cached, {} fetched, {} unavailable, {} failed, {} not fetched; {} Jira requests",
                stats.ticket_keys,
                stats.ticket_hits,
                stats.tickets_fetched,
                stats.tickets_unavailable,
                stats.tickets_failed,
                stats.tickets_not_fetched,
                stats.jira_requests
            );
            println!(
                "summaries: {} tickets, {} summarised, {} invalid, {} failed, {} skipped; {} chat calls, {} cache hits, {} retries; max {} prompt / {} completion tokens",
                stats.summary_tickets,
                stats.summaries,
                stats.summaries_invalid,
                stats.summaries_failed,
                stats.summaries_skipped,
                stats.summary_llm.chat_calls,
                stats.summary_llm.chat_hits,
                stats.summary_llm.chat_retries,
                stats.summary_llm.peak_prompt_tokens,
                stats.summary_llm.peak_completion_tokens
            );
            println!(
                "describe: {} methods, {} described ({} from cache), {} invalid, {} failed, {} incomplete, {} skipped; {} chat calls, {} cache hits, {} retries; max {} prompt / {} completion tokens",
                stats.describe_members,
                stats.described,
                stats.described_cached,
                stats.describe_invalid,
                stats.describe_failed,
                stats.describe_incomplete,
                stats.describe_skipped,
                stats.describe_llm.chat_calls,
                stats.describe_llm.chat_hits,
                stats.describe_llm.chat_retries,
                stats.describe_llm.peak_prompt_tokens,
                stats.describe_llm.peak_completion_tokens
            );
            println!(
                "types: {} types, {} described ({} from cache), {} invalid, {} failed, {} incomplete, {} skipped; {} chat calls, {} cache hits, {} retries; max {} prompt / {} completion tokens",
                stats.type_symbols,
                stats.types_described,
                stats.types_described_cached,
                stats.types_invalid,
                stats.types_failed,
                stats.types_incomplete,
                stats.types_skipped,
                stats.type_llm.chat_calls,
                stats.type_llm.chat_hits,
                stats.type_llm.chat_retries,
                stats.type_llm.peak_prompt_tokens,
                stats.type_llm.peak_completion_tokens
            );
            println!(
                "embeddings: {} symbols, {} embedded ({} from cache), {} failed, {} skipped; {} embedding calls, {} texts sent, dim {}",
                stats.embed_symbols,
                stats.embedded,
                stats.embedded_cached,
                stats.embed_failed,
                stats.embed_skipped,
                stats.embed_llm.embed_calls,
                stats.embed_llm.embed_texts,
                stats.embedding_dim
            );
        }
        Command::Show { fqn } => {
            let reader = IndexReader::open(&config.data_dir).await?;
            print!("{}", show::render(reader.connection(), fqn).await?);
        }
        Command::Search {
            query,
            kind,
            role,
            limit,
        } => {
            let ollama = config
                .ollama
                .as_ref()
                .context("search needs an [ollama] section with the embedding_model the index was built with")?;
            let filter = Filter {
                kinds: kind
                    .iter()
                    .filter_map(|kind| SymbolKind::parse(kind))
                    .collect(),
                roles: role.iter().filter_map(|role| Role::parse(role)).collect(),
                path: cli
                    .path
                    .as_deref()
                    .map(|prefix| search_path(&config.repo, prefix))
                    .transpose()?,
            };
            let reader = IndexReader::open(&config.data_dir).await?;
            let embedder = QueryEmbedder::from_config(ollama).await?;
            let hits =
                search::search(reader.connection(), &embedder, query, &filter, *limit).await?;
            if hits.is_empty() {
                eprintln!("no symbol matches the filter");
            }
            print!("{}", search::format_hits(&hits));
        }
    }
    Ok(())
}

fn init_logging(verbose: u8) {
    use tracing_subscriber::EnvFilter;

    let level = match verbose {
        0 => "warn",
        1 => "info",
        2 => "debug",
        _ => "trace",
    };
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(level));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(std::io::stderr().is_terminal())
        .init();
}
