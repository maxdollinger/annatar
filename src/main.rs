use std::path::PathBuf;

use anyhow::Result;
use clap::{ArgAction, Parser, Subcommand};

use annatar::config::Config;
use annatar::store::{IndexReader, Store};
use annatar::summaries::Summarizer;
use annatar::tickets::TicketFetch;
use annatar::{indexer, show};

/// Index a codebase by intent: what each symbol does and why it exists.
#[derive(Debug, Parser)]
#[command(name = "annatar", version, about)]
struct Cli {
    /// Limit the run to paths under this prefix, for fast iteration. `index`
    /// still replaces the whole index.db, which then holds only this prefix.
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
        /// Make no chat-model calls; summaries and descriptions already in
        /// cache.db are still used, the rest get none.
        #[arg(long)]
        no_llm: bool,
    },
    /// Print a symbol and its children.
    Show {
        /// Fully qualified name, e.g. `com.acme.user.UserRepository`.
        fqn: String,
    },
    /// Search symbol descriptions by meaning.
    Search {
        /// Plain-language query.
        query: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    init_logging(cli.verbose);

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
                Summarizer::from_config(config.ollama.as_ref(), config.describe, *no_llm, &store)?;
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
        }
        Command::Show { fqn } => {
            let reader = IndexReader::open(&config.data_dir).await?;
            print!("{}", show::render(reader.connection(), fqn).await?);
        }
        Command::Search { query } => not_implemented(&format!("search {query}")),
    }
    Ok(())
}

fn not_implemented(command: &str) {
    tracing::warn!(command, "not implemented yet");
    println!("{command}: not implemented yet");
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
    tracing_subscriber::fmt().with_env_filter(filter).init();
}
