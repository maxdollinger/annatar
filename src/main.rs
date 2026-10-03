use std::path::PathBuf;

use anyhow::Result;
use clap::{ArgAction, Parser, Subcommand};

use annatar::config::Config;

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

fn main() -> Result<()> {
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
        Command::Index => not_implemented("index"),
        Command::Show { fqn } => not_implemented(&format!("show {fqn}")),
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
