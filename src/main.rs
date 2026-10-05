use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::builder::PossibleValuesParser;
use clap::error::ErrorKind;
use clap::{ArgAction, CommandFactory, Parser, Subcommand};

use annatar::config::Config;
use annatar::search::{self, Filter, QueryEmbedder};
use annatar::store::{IndexReader, Store};
use annatar::summaries::Summarizer;
use annatar::symbols::{Role, SymbolKind};
use annatar::tickets::TicketFetch;
use annatar::{indexer, show, trace};

/// Index a codebase by intent: what each symbol does and why it exists.
#[derive(Debug, Parser)]
#[command(name = "annatar", version, about, disable_help_subcommand = true)]
struct Cli {
    /// Limit `index` and `search` to paths under this prefix.
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
    /// Build the index for the current repository.
    Index {
        /// Make no Jira requests; cached tickets are still used.
        #[arg(long)]
        offline: bool,
        /// Make no chat or embedding calls; cached results are still used.
        #[arg(long)]
        no_llm: bool,
    },
    /// Print the information about a symbol: its members and usages
    /// `--history` adds its git and ticket history
    #[command(verbatim_doc_comment)]
    Show {
        /// Fully qualified name, e.g. `com.acme.user.UserRepository`.
        fqn: String,
        /// Entries per usage list (default 20, at most 100).
        #[arg(short = 'k', long, value_name = "N", default_value_t = show::DEFAULT_LIMIT, value_parser = parse_show_limit)]
        limit: usize,
        /// Add the git and ticket history.
        #[arg(long)]
        history: bool,
    },
    /// Print everything that calls a symbol, up to the entry points.
    Trace {
        /// Fully qualified name, e.g. `com.acme.user.UserRepository#find(Long)`.
        fqn: String,
        /// Caller levels below the symbol (default 6, at most 10).
        #[arg(long, value_name = "N", default_value_t = trace::DEFAULT_DEPTH, value_parser = parse_depth)]
        depth: usize,
        /// Callers per symbol (default 10, at most 100).
        #[arg(short = 'k', long, value_name = "N", default_value_t = trace::DEFAULT_LIMIT, value_parser = parse_show_limit)]
        limit: usize,
    },
    /// Find the code that matches a plain-language query.
    Search {
        /// Plain-language query.
        query: String,
        /// Only symbols of this kind can match (repeatable).
        #[arg(long, value_name = "KIND", value_parser = PossibleValuesParser::new(SymbolKind::ALL.map(|kind| kind.as_str())))]
        kind: Vec<String>,
        /// Only types with this Spring role and their members can match (repeatable).
        #[arg(long, value_name = "ROLE", value_parser = PossibleValuesParser::new(Role::ALL.map(|role| role.as_str())))]
        role: Vec<String>,
        /// Number of files (default 5, at most 20), or symbols with
        /// `--symbols` (default 10, at most 100).
        #[arg(short = 'k', long, value_name = "N", value_parser = parse_limit)]
        limit: Option<usize>,
        /// Print the matching symbols instead of files.
        #[arg(long)]
        symbols: bool,
    },
    #[command(hide = true)]
    Help { command: Option<String> },
}

fn parse_limit(text: &str) -> Result<usize, String> {
    parse_limit_from(text, 1).map_err(|_| format!("expected {}", search_limits()))
}

fn search_limits() -> String {
    format!(
        "files: 1 to {}; with --symbols: 1 to {}",
        search::MAX_FILES,
        search::MAX_LIMIT
    )
}

fn parse_show_limit(text: &str) -> Result<usize, String> {
    parse_limit_from(text, 1)
}

fn parse_depth(text: &str) -> Result<usize, String> {
    match text.parse::<usize>() {
        Ok(depth) if (1..=trace::MAX_DEPTH).contains(&depth) => Ok(depth),
        _ => Err(format!("expected a number from 1 to {}", trace::MAX_DEPTH)),
    }
}

fn parse_limit_from(text: &str, min: usize) -> Result<usize, String> {
    match text.parse::<usize>() {
        Ok(limit) if (min..=search::MAX_LIMIT).contains(&limit) => Ok(limit),
        _ => Err(format!(
            "expected a number from {min} to {}",
            search::MAX_LIMIT
        )),
    }
}

fn print_help(name: Option<&str>) {
    let mut command = Cli::command();
    command.build();
    let _ = match name {
        None => command.print_help(),
        Some(name) => match command.find_subcommand_mut(name) {
            Some(subcommand) if !subcommand.is_hide_set() => subcommand.print_help(),
            _ => command
                .error(
                    ErrorKind::InvalidSubcommand,
                    format!("unrecognized subcommand '{name}'"),
                )
                .exit(),
        },
    };
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    if let Command::Help { command } = &cli.command {
        print_help(command.as_deref());
        return ExitCode::SUCCESS;
    }
    if let Command::Search {
        limit: Some(limit),
        symbols: false,
        ..
    } = cli.command
        && limit > search::MAX_FILES
    {
        let mut command = Cli::command();
        command.build();
        command
            .find_subcommand_mut("search")
            .expect("search is a subcommand")
            .error(
                ErrorKind::ValueValidation,
                format!(
                    "-k {limit}: search prints at most {} files; use --symbols for more ({})",
                    search::MAX_FILES,
                    search_limits()
                ),
            )
            .exit();
    }
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
                "edges: {} ({} extends, {} implements, {} instantiate, {} reference, {} call, {} overrides); {} unresolved type mentions ({} names); {} ms",
                stats.edges(),
                stats.edges_extends,
                stats.edges_implements,
                stats.edges_instantiate,
                stats.edges_reference,
                stats.edges_call,
                stats.edges_overrides,
                stats.unresolved_types,
                stats.unresolved_type_names,
                stats.edges_time.as_millis()
            );
            println!(
                "call sites: {} ({} resolved to a member, {} implicit, {} ambiguous, {} unresolved)",
                stats.calls_resolved
                    + stats.calls_implicit
                    + stats.calls_ambiguous
                    + stats.calls_unresolved,
                stats.calls_resolved,
                stats.calls_implicit,
                stats.calls_ambiguous,
                stats.calls_unresolved
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
        Command::Show {
            fqn,
            limit,
            history,
        } => {
            let reader = IndexReader::open(&config.data_dir).await?;
            print!(
                "{}",
                show::render_with(reader.connection(), fqn, *limit, *history).await?
            );
        }
        Command::Trace { fqn, depth, limit } => {
            let reader = IndexReader::open(&config.data_dir).await?;
            print!(
                "{}",
                trace::render(reader.connection(), fqn, *depth, *limit).await?
            );
        }
        Command::Search {
            query,
            kind,
            role,
            limit,
            symbols,
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
                path: match cli.path.as_deref() {
                    Some(prefix) => search::path_filter(&config.repo, prefix)?,
                    None => None,
                },
            };
            let reader = IndexReader::open(&config.data_dir).await?;
            let embedder = QueryEmbedder::from_config(ollama).await?;
            if *symbols {
                let limit = limit.unwrap_or(search::DEFAULT_LIMIT);
                let hits =
                    search::search(reader.connection(), &embedder, query, &filter, limit).await?;
                if hits.is_empty() {
                    eprintln!("no symbol matches the filter");
                }
                print!("{}", search::format_hits(&hits));
            } else {
                let limit = limit.unwrap_or(search::DEFAULT_FILES);
                let found =
                    search::search_files(reader.connection(), &embedder, query, &filter, limit)
                        .await?;
                let symbols = search::group_symbols(reader.connection(), &found.groups).await?;
                if let Some(note) = found.note() {
                    eprintln!("{note}");
                }
                print!(
                    "{}",
                    search::format_files(&found.groups, &symbols, search::MEMBER_LINES)
                );
            }
        }
        Command::Help { .. } => unreachable!("help is handled before the config is loaded"),
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
