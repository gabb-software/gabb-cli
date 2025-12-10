mod daemon;
mod indexer;
mod rust_lang;
mod store;
mod ts;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "gabb", about = "Gabb CLI indexing daemon")]
struct Cli {
    /// Increase output verbosity (-v, -vv)
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    verbose: u8,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Run the indexing daemon for a workspace
    Daemon {
        /// Workspace root to index
        #[arg(long, default_value = ".")]
        root: PathBuf,
        /// Path to the SQLite index database
        #[arg(long, default_value = ".gabb/index.db")]
        db: PathBuf,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    init_logging(cli.verbose);

    match cli.command {
        Commands::Daemon { root, db } => daemon::run(&root, &db),
    }
}

fn init_logging(verbosity: u8) {
    let level = match verbosity {
        0 => "info",
        1 => "debug",
        _ => "trace",
    };
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(level))
        .format_timestamp_secs()
        .init();
}
