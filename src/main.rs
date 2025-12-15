mod daemon;
mod indexer;
mod languages;
mod store;

use anyhow::{Context, Result, anyhow, bail};
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
        let (line, col) = offset_to_line_char_in_file(&sym.file, sym.start)?;
        let (end_line, end_col) = offset_to_line_char_in_file(&sym.file, sym.end)?;
        println!(
            "{:<10} {:<30} {} [{}:{}-{}:{}]{container}",
            sym.kind, sym.name, sym.file, line, col, end_line, end_col
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

    let (t_line, t_col) = offset_to_line_char_in_file(&target.file, target.start)?;
    let (t_end_line, t_end_col) = offset_to_line_char_in_file(&target.file, target.end)?;
    println!(
        "Target: {} {} {} [{}:{}-{}:{}]",
        target.kind, target.name, target.file, t_line, t_col, t_end_line, t_end_col
    );
    for sym in impl_symbols {
        let container = sym
            .container
            .as_deref()
            .map(|c| format!(" in {c}"))
            .unwrap_or_default();
        let (line, col) = offset_to_line_char_in_file(&sym.file, sym.start)?;
        let (end_line, end_col) = offset_to_line_char_in_file(&sym.file, sym.end)?;
        println!(
            "{:<10} {:<30} {} [{}:{}-{}:{}]{container}",
            sym.kind, sym.name, sym.file, line, col, end_line, end_col
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

    let ident = find_identifier_at_offset(&contents, offset as usize);

    if let Some(def) = narrowest_symbol_covering(&symbols, offset, ident.as_deref()) {
        return Ok(def);
    }

    let ident = ident.ok_or_else(|| anyhow!("no symbol found at {}:{}", file.display(), line))?;
    let mut candidates = store.list_symbols(None, None, Some(&ident), None)?;
    if candidates.is_empty() {
        bail!("no symbol found at {}:{}", file.display(), line);
    }
    candidates.sort_by(|a, b| {
        a.file
            .cmp(&b.file)
            .then_with(|| a.qualifier.cmp(&b.qualifier))
            .then_with(|| a.start.cmp(&b.start))
    });
    Ok(candidates.remove(0))
}

fn narrowest_symbol_covering(
    symbols: &[SymbolRecord],
    offset: i64,
    ident: Option<&str>,
) -> Option<SymbolRecord> {
    let mut best: Option<&SymbolRecord> = None;
    for sym in symbols {
        if sym.start <= offset && offset < sym.end {
            if let Some(id) = ident {
                if !identifier_matches_symbol(id, sym) {
                    continue;
                }
            }
            let span = sym.end - sym.start;
            if best.map(|b| span < (b.end - b.start)).unwrap_or(true) {
                best = Some(sym);
            }
        }
    }

    best.cloned()
}

fn find_identifier_at_offset(buf: &[u8], offset: usize) -> Option<String> {
    if offset >= buf.len() {
        return None;
    }
    let is_ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut start = offset;
    while start > 0 && is_ident(buf[start.saturating_sub(1)]) {
        start -= 1;
    }
    let mut end = offset;
    while end < buf.len() && is_ident(buf[end]) {
        end += 1;
    }
    if start == end {
        return None;
    }
    std::str::from_utf8(&buf[start..end])
        .ok()
        .map(|s| s.to_string())
}

fn identifier_matches_symbol(ident: &str, sym: &SymbolRecord) -> bool {
    if ident == sym.name {
        return true;
    }
    matches!(
        ident,
        "fn" | "function" | "class" | "interface" | "enum" | "struct" | "impl"
    )
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

fn offset_to_line_char_in_file(path: &str, offset: i64) -> Result<(usize, usize)> {
    let buf = fs::read(path).with_context(|| format!("failed to read {}", path))?;
    offset_to_line_char_in_buf(&buf, offset as usize)
        .ok_or_else(|| anyhow!("could not map byte offset for {path}"))
}

fn offset_to_line_char_in_buf(buf: &[u8], offset: usize) -> Option<(usize, usize)> {
    let mut line = 1usize;
    let mut col = 1usize;
    for (i, b) in buf.iter().enumerate() {
        if i == offset {
            return Some((line, col));
        }
        if *b == b'\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    if offset == buf.len() {
        Some((line, col))
    } else {
        None
    }
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

    #[test]
    fn resolves_reference_by_name_when_not_a_definition() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let impl_path = root.join("indexer.rs");
        let caller_path = root.join("daemon.rs");
        fs::write(
            &impl_path,
            r#"
                fn build_full_index() {}
            "#,
        )
        .unwrap();
        let call_src = r#"fn main() { build_full_index(); }"#;
        fs::write(&caller_path, call_src).unwrap();
        let caller_path = caller_path.canonicalize().unwrap();

        let db_path = root.join(".gabb/index.db");
        let store = IndexStore::open(&db_path).unwrap();
        indexer::build_full_index(root, &store).unwrap();

        let offset = call_src.find("build_full_index").unwrap();
        let (line, character) = offset_to_line_char(call_src.as_bytes(), offset).unwrap();

        let symbol = resolve_symbol_at(&store, &caller_path, line, character).unwrap();
        assert_eq!(symbol.name, "build_full_index");
        assert!(symbol.file.ends_with("indexer.rs"));
    }

    fn offset_to_line_char(buf: &[u8], offset: usize) -> Option<(usize, usize)> {
        let mut line = 1usize;
        let mut col = 1usize;
        for (i, b) in buf.iter().enumerate() {
            if i == offset {
                return Some((line, col));
            }
            if *b == b'\n' {
                line += 1;
                col = 1;
            } else {
                col += 1;
            }
        }
        if offset == buf.len() {
            Some((line, col))
        } else {
            None
        }
    }
}
