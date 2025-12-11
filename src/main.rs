mod daemon;
mod indexer;
mod rust_lang;
mod store;
mod ts;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use store::SymbolRecord;

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
    /// List symbols from an existing index
    Symbols {
        /// Path to the SQLite index database
        #[arg(long, default_value = ".gabb/index.db")]
        db: PathBuf,
        /// Only show symbols from this file
        #[arg(long)]
        file: Option<PathBuf>,
        /// Only show symbols of this kind (function, class, interface, method, struct, enum, trait)
        #[arg(long)]
        kind: Option<String>,
        /// Limit the number of results
        #[arg(long)]
        limit: Option<usize>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    init_logging(cli.verbose);

    match cli.command {
        Commands::Daemon { root, db } => daemon::run(&root, &db),
        Commands::Symbols {
            db,
            file,
            kind,
            limit,
        } => list_symbols(&db, file.as_ref(), kind.as_deref(), limit),
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

fn list_symbols(
    db: &PathBuf,
    file: Option<&PathBuf>,
    kind: Option<&str>,
    limit: Option<usize>,
) -> Result<()> {
    let store = store::IndexStore::open(db)?;
    let file_str = file.map(|p| p.to_string_lossy().to_string());
    let symbols: Vec<SymbolRecord> = store.list_symbols(file_str.as_deref(), kind, limit)?;

    for sym in symbols {
        let container = sym
            .container
            .as_deref()
            .map(|c| format!(" in {c}"))
            .unwrap_or_default();
        println!(
            "{:<10} {:<30} {} [{}-{}]{container}",
            sym.kind, sym.name, sym.file, sym.start, sym.end
        );
    }

    Ok(())
}
