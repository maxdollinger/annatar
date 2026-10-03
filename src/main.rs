use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{ArgAction, Parser, Subcommand};

use annatar::config::Config;
use annatar::store::{IndexReader, Store};
use annatar::{indexer, show};

/// Index a codebase by intent: what each symbol does and why it exists.
#[derive(Debug, Parser)]
#[command(name = "annatar", version, about)]
struct Cli {
    /// Limit the run to paths under this prefix.
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
    Index,
    /// Print a symbol and its children.
    Show {
        /// Fully qualified name, e.g. `com.acme.user.UserRepository`.
        fqn: String,
    },
    /// Search what/why records by meaning.
    Search {
        /// Plain-language query.
        query: String,
    },
    /// Run the MCP server over stdio.
    Serve,
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
        Command::Index => {
            let ticket_regex = regex::Regex::new(&config.ticket_regex)
                .with_context(|| format!("invalid ticket_regex {:?}", config.ticket_regex))?;
            let store = Store::open(&config.data_dir).await?;
            let stats =
                indexer::build_index(&store, &config.repo, cli.path.as_deref(), &ticket_regex)
                    .await?;
            println!(
                "indexed {} files, {} symbols, {} commits, {} tickets, {} empty, {} parse errors, {} unreadable",
                stats.files,
                stats.symbols,
                stats.commits,
                stats.tickets,
                stats.empty,
                stats.parse_errors,
                stats.unreadable
            );
        }
        Command::Show { fqn } => {
            let reader = IndexReader::open(&config.data_dir).await?;
            print!("{}", show::render(reader.connection(), fqn).await?);
        }
        Command::Search { query } => not_implemented(&format!("search {query}")),
        Command::Serve => not_implemented("serve"),
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
