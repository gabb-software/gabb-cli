mod daemon;
mod indexer;
mod rust_lang;
mod store;
mod ts;

use anyhow::{Result, anyhow};
use clap::{Parser, Subcommand};
use std::fs;
use std::path::{Path, PathBuf};
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
        /// Only show symbols with this exact name
        #[arg(long)]
        name: Option<String>,
        /// Limit the number of results
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Find implementations for symbol at a source position
    Implementation {
        /// Path to the SQLite index database
        #[arg(long, default_value = ".gabb/index.db")]
        db: PathBuf,
        /// Source file containing the reference
        #[arg(long)]
        file: PathBuf,
        /// 1-based line number within the file
        #[arg(long)]
        line: usize,
        /// 1-based character offset within the line
        #[arg(long, alias = "col")]
        character: usize,
        /// Limit the number of results
        #[arg(long)]
        limit: Option<usize>,
        /// Only show symbols of this kind (function, class, interface, method, struct, enum, trait)
        #[arg(long)]
        kind: Option<String>,
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
            name,
            limit,
        } => list_symbols(&db, file.as_ref(), kind.as_deref(), name.as_deref(), limit),
        Commands::Implementation {
            db,
            file,
            line,
            character,
            limit,
            kind,
        } => find_implementation(&db, &file, line, character, limit, kind.as_deref()),
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
    db: &Path,
    file: Option<&PathBuf>,
    kind: Option<&str>,
    name: Option<&str>,
    limit: Option<usize>,
) -> Result<()> {
    let store = store::IndexStore::open(db)?;
    let file_str = file.map(|p| p.to_string_lossy().to_string());
    let symbols: Vec<SymbolRecord> = store.list_symbols(file_str.as_deref(), kind, name, limit)?;

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

fn find_implementation(
    db: &Path,
    file: &PathBuf,
    line: usize,
    character: usize,
    limit: Option<usize>,
    kind: Option<&str>,
) -> Result<()> {
    let store = store::IndexStore::open(db)?;
    let target = resolve_symbol_at(&store, file, line, character)?;

    let mut impl_edges = store.edges_to(&target.id)?;
    let impl_ids: Vec<String> = impl_edges.drain(..).map(|e| e.src).collect();
    let mut impl_symbols = store.symbols_by_ids(&impl_ids)?;

    if impl_symbols.is_empty() {
        impl_symbols = store.list_symbols(None, kind, Some(&target.name), limit)?;
    }

    if let Some(k) = kind {
        impl_symbols.retain(|s| s.kind == k);
    }
    if let Some(lim) = limit {
        impl_symbols.truncate(lim);
    }

    println!(
        "Target: {} {} {} [{}-{}]",
        target.kind, target.name, target.file, target.start, target.end
    );
    for sym in impl_symbols {
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

fn resolve_symbol_at(
    store: &store::IndexStore,
    file: &PathBuf,
    line: usize,
    character: usize,
) -> Result<SymbolRecord> {
    let file_str = file.to_string_lossy().to_string();
    let symbols = store.list_symbols(Some(&file_str), None, None, None)?;
    let contents = fs::read(file)?;
    let offset = line_char_to_offset(&contents, line, character)
        .ok_or_else(|| anyhow!("could not map line/character to byte offset"))?
        as i64;

    let mut best: Option<SymbolRecord> = None;
    for sym in symbols {
        if sym.start <= offset && offset < sym.end {
            let span = sym.end - sym.start;
            if best
                .as_ref()
                .map(|b| span < (b.end - b.start))
                .unwrap_or(true)
            {
                best = Some(sym);
            }
        }
    }

    best.ok_or_else(|| anyhow!("no symbol found at {}:{}", file.display(), line))
}

fn line_char_to_offset(buf: &[u8], line: usize, character: usize) -> Option<usize> {
    if line == 0 || character == 0 {
        return None;
    }
    let mut idx = 0;
    let mut current_line = 1usize;
    while current_line < line {
        if let Some(pos) = buf[idx..].iter().position(|b| *b == b'\n') {
            idx += pos + 1;
            current_line += 1;
        } else {
            return None;
        }
    }
    let line_end = buf[idx..]
        .iter()
        .position(|b| *b == b'\n')
        .map(|p| idx + p)
        .unwrap_or(buf.len());
    let line_len = line_end - idx;
    let col = character.saturating_sub(1).min(line_len);
    Some(idx + col)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indexer;
    use crate::store::IndexStore;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn resolves_symbol_at_position() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let file_path = root.join("sample.ts");
        fs::write(&file_path, "function foo() {}\nfunction bar() {}\n").unwrap();
        let file_path = file_path.canonicalize().unwrap();

        let db_path = root.join(".gabb/index.db");
        let store = IndexStore::open(&db_path).unwrap();
        indexer::build_full_index(root, &store).unwrap();

        let symbol = resolve_symbol_at(&store, &file_path, 1, 10).unwrap();
        assert_eq!(symbol.name, "foo");
        assert_eq!(symbol.kind, "function");
    }
}
