use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::builder::PossibleValuesParser;
use clap::error::ErrorKind;
use clap::{ArgAction, CommandFactory, Parser, Subcommand};

use annatar::config::Config;
use annatar::golden::GoldenSet;
use annatar::search::{self, Filter, QueryEmbedder};
use annatar::store::{IndexReader, Store};
use annatar::summaries::Summarizer;
use annatar::symbols::{Role, SymbolKind};
use annatar::tickets::TicketFetch;
use annatar::usage_eval::{self, UsageSet};
use annatar::{eval, indexer, show};

/// Index a codebase by intent: what each symbol does and why it exists.
#[derive(Debug, Parser)]
#[command(name = "annatar", version, about)]
struct Cli {
    /// Limit the run to paths under this prefix, for fast iteration. `index`
    /// still replaces the whole index.db, which then holds only this prefix;
    /// `search` matches only symbols in files under it.
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
    /// Print a symbol and its children, with its entry-point annotations
    /// and its direct usages: `used by` (for a type also its members' and
    /// nested types' users, for a method also the callers of the methods it
    /// overrides, `via`) and `uses`, grouped by file as `fqn [kind] :lines`.
    Show {
        /// Fully qualified name, e.g. `com.acme.user.UserRepository`.
        fqn: String,
        /// Entries listed per usage list (default 20, at most 100); the
        /// rest are counted as `… N more`.
        #[arg(short = 'k', long, value_name = "N", default_value_t = show::DEFAULT_LIMIT, value_parser = parse_show_limit)]
        limit: usize,
    },
    /// Search symbol descriptions by meaning and print the files of the
    /// best matches: per file `rank. score path`, its top-level type as
    /// `kind fqn [role] :start-end` with its description, then its members
    /// and nested types with their lines; the best matching symbols end with
    /// `*score`. `--symbols` prints the matching symbols instead.
    Search {
        /// Plain-language query.
        query: String,
        /// Only symbols of this kind can match (repeatable); the file output
        /// still prints each file of a match whole.
        #[arg(long, value_name = "KIND", value_parser = PossibleValuesParser::new(SymbolKind::ALL.map(|kind| kind.as_str())))]
        kind: Vec<String>,
        /// Only types with this Spring role and their methods and
        /// constructors can match (repeatable); the file output still prints
        /// each file of a match whole.
        #[arg(long, value_name = "ROLE", value_parser = PossibleValuesParser::new(Role::ALL.map(|role| role.as_str())))]
        role: Vec<String>,
        /// Number of results: files (default 5, at most 20), or symbols
        /// with `--symbols` (default 10, at most 100).
        #[arg(short = 'k', long, value_name = "N", value_parser = parse_limit)]
        limit: Option<usize>,
        /// Print the most similar symbols, each as `rank. score fqn [kind]
        /// role=… file:start-end` with its description on the next line.
        #[arg(long)]
        symbols: bool,
    },
    /// Score retrieval against a golden set: every question runs through
    /// `search --symbols` and `search`; prints per question the rank of its
    /// first expected fqn and the best rank of any (`-` = not in the top N),
    /// the same for their files, then top-1, top-5 and MRR for all
    /// questions, types and members, by symbol and by file.
    Eval {
        /// The golden set (TOML, see `golden`).
        golden: PathBuf,
        /// Hits searched per question; at least 5, as the report counts top-5.
        #[arg(short = 'k', long, value_name = "N", default_value_t = search::DEFAULT_LIMIT, value_parser = parse_eval_limit)]
        limit: usize,
    },
    /// Score the usages (`edges`) against a usage golden set: prints per
    /// symbol the precision and recall of its direct users with the missed
    /// and extra ones, the overriding methods when the set lists them, then
    /// precision and recall over all symbols, types and members.
    EvalUsages {
        /// The usage golden set (TOML, see `usage_eval`).
        set: PathBuf,
    },
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

fn parse_eval_limit(text: &str) -> Result<usize, String> {
    parse_limit_from(text, eval::MIN_LIMIT)
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

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
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
        Command::Show { fqn, limit } => {
            let reader = IndexReader::open(&config.data_dir).await?;
            print!(
                "{}",
                show::render_limited(reader.connection(), fqn, *limit).await?
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
        Command::Eval { golden, limit } => {
            if cli.path.is_some() {
                anyhow::bail!("eval scores the whole index and takes no --path");
            }
            let ollama = config.ollama.as_ref().context(
                "eval needs an [ollama] section with the embedding_model the index was built with",
            )?;
            let set = GoldenSet::load(golden)?;
            let reader = IndexReader::open(&config.data_dir).await?;
            let embedder = QueryEmbedder::from_config(ollama).await?;
            let outcomes = eval::evaluate(reader.connection(), &embedder, &set, *limit).await?;
            print!(
                "{}",
                eval::settings_line(reader.connection(), *limit).await?
            );
            print!("{}", eval::format_report(&outcomes));
        }
        Command::EvalUsages { set } => {
            if cli.path.is_some() {
                anyhow::bail!("eval-usages scores the whole index and takes no --path");
            }
            let set = UsageSet::load(set)?;
            let reader = IndexReader::open(&config.data_dir).await?;
            let outcomes = usage_eval::evaluate(reader.connection(), &set).await?;
            print!("{}", usage_eval::format_report(&outcomes));
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
