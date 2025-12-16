mod daemon;
mod indexer;
mod languages;
mod store;

use anyhow::{anyhow, bail, Context, Result};
use clap::{Parser, Subcommand};
use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use store::{normalize_path, SymbolRecord};

#[derive(Parser, Debug)]
#[command(name = "gabb", about = "Gabb CLI indexing daemon")]
struct Cli {
    /// Increase output verbosity (-v, -vv)
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    verbose: u8,

    /// Output results as JSON (for agent integration)
    #[arg(long, global = true)]
    json: bool,

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
        /// If set, delete any existing DB and rebuild the index from scratch before watching
        #[arg(long)]
        rebuild: bool,
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
        /// Source file containing the reference. You can optionally append :line:character (1-based), e.g. ./src/daemon.rs:18:5
        #[arg(long)]
        file: PathBuf,
        /// 1-based line number within the file (optional if provided in --file)
        #[arg(long)]
        line: Option<usize>,
        /// 1-based character offset within the line (optional if provided in --file)
        #[arg(long, alias = "col")]
        character: Option<usize>,
        /// Limit the number of results
        #[arg(long)]
        limit: Option<usize>,
        /// Only show symbols of this kind (function, class, interface, method, struct, enum, trait)
        #[arg(long)]
        kind: Option<String>,
    },
    /// Find usages of the symbol at a source position
    Usages {
        /// Path to the SQLite index database
        #[arg(long, default_value = ".gabb/index.db")]
        db: PathBuf,
        /// Source file containing the reference. You can optionally append :line:character (1-based), e.g. ./src/daemon.rs:18:5
        #[arg(long)]
        file: PathBuf,
        /// 1-based line number within the file (optional if provided in --file)
        #[arg(long)]
        line: Option<usize>,
        /// 1-based character offset within the line (optional if provided in --file)
        #[arg(long, alias = "col")]
        character: Option<usize>,
        /// Limit the number of results
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Show details for symbols with a given name
    Symbol {
        /// Path to the SQLite index database
        #[arg(long, default_value = ".gabb/index.db")]
        db: PathBuf,
        /// Symbol name to look up
        #[arg(long)]
        name: String,
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
    /// Go to definition: find where a symbol is declared
    Definition {
        /// Path to the SQLite index database
        #[arg(long, default_value = ".gabb/index.db")]
        db: PathBuf,
        /// Source file containing the reference. You can optionally append :line:character (1-based), e.g. ./src/daemon.rs:18:5
        #[arg(long)]
        file: PathBuf,
        /// 1-based line number within the file (optional if provided in --file)
        #[arg(long)]
        line: Option<usize>,
        /// 1-based character offset within the line (optional if provided in --file)
        #[arg(long, alias = "col")]
        character: Option<usize>,
    },
    /// Find duplicate code in the codebase
    Duplicates {
        /// Path to the SQLite index database
        #[arg(long, default_value = ".gabb/index.db")]
        db: PathBuf,
        /// Only analyze files with uncommitted changes (git working tree)
        #[arg(long)]
        uncommitted: bool,
        /// Only analyze files in git staging area
        #[arg(long)]
        staged: bool,
        /// Only check specific symbol kinds (function, method, class, etc.)
        #[arg(long)]
        kind: Option<String>,
        /// Minimum number of duplicates to report (default: 2)
        #[arg(long, default_value = "2")]
        min_count: usize,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    init_logging(cli.verbose);
    let json_output = cli.json;

    match cli.command {
        Commands::Daemon { root, db, rebuild } => daemon::run(&root, &db, rebuild),
        Commands::Symbols {
            db,
            file,
            kind,
            name,
            limit,
        } => list_symbols(
            &db,
            file.as_ref(),
            kind.as_deref(),
            name.as_deref(),
            limit,
            json_output,
        ),
        Commands::Implementation {
            db,
            file,
            line,
            character,
            limit,
            kind,
        } => find_implementation(
            &db,
            &file,
            line,
            character,
            limit,
            kind.as_deref(),
            json_output,
        ),
        Commands::Usages {
            db,
            file,
            line,
            character,
            limit,
        } => find_usages(&db, &file, line, character, limit, json_output),
        Commands::Symbol {
            db,
            name,
            file,
            kind,
            limit,
        } => show_symbol(
            &db,
            &name,
            file.as_ref(),
            kind.as_deref(),
            limit,
            json_output,
        ),
        Commands::Definition {
            db,
            file,
            line,
            character,
        } => find_definition(&db, &file, line, character, json_output),
        Commands::Duplicates {
            db,
            uncommitted,
            staged,
            kind,
            min_count,
        } => find_duplicates(
            &db,
            uncommitted,
            staged,
            kind.as_deref(),
            min_count,
            json_output,
        ),
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
    json_output: bool,
) -> Result<()> {
    let store = store::IndexStore::open(db)?;
    let file_str = file.map(|p| p.to_string_lossy().to_string());
    let symbols: Vec<SymbolRecord> = store.list_symbols(file_str.as_deref(), kind, name, limit)?;

    if json_output {
        let json_symbols: Vec<serde_json::Value> = symbols
            .iter()
            .filter_map(|sym| {
                let (line, col) = offset_to_line_char_in_file(&sym.file, sym.start).ok()?;
                let (end_line, end_col) = offset_to_line_char_in_file(&sym.file, sym.end).ok()?;
                Some(serde_json::json!({
                    "id": sym.id,
                    "name": sym.name,
                    "kind": sym.kind,
                    "file": sym.file,
                    "start": { "line": line, "character": col },
                    "end": { "line": end_line, "character": end_col },
                    "visibility": sym.visibility,
                    "container": sym.container,
                    "qualifier": sym.qualifier
                }))
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&json_symbols)?);
        return Ok(());
    }

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
    file: &Path,
    line: Option<usize>,
    character: Option<usize>,
    limit: Option<usize>,
    kind: Option<&str>,
    json_output: bool,
) -> Result<()> {
    let store = store::IndexStore::open(db)?;
    let (file, line, character) = parse_file_position(file, line, character)?;
    let target = resolve_symbol_at(&store, &file, line, character)?;

    let mut impl_edges = store.edges_to(&target.id)?;
    let impl_ids: Vec<String> = impl_edges.drain(..).map(|e| e.src).collect();
    let mut impl_symbols = store.symbols_by_ids(&impl_ids)?;

    if impl_symbols.is_empty() {
        // Use dependency graph: implementations would be in files that depend on the target's file
        let dependents = store.get_dependents(&target.file)?;
        if dependents.is_empty() {
            // No dependency info - fall back to searching all files
            impl_symbols = store.list_symbols(None, kind, Some(&target.name), limit)?;
        } else {
            // Search only in dependent files plus the target's own file
            for dep_file in dependents.iter().chain(std::iter::once(&target.file)) {
                let file_symbols =
                    store.list_symbols(Some(dep_file), kind, Some(&target.name), limit)?;
                impl_symbols.extend(file_symbols);
            }
        }
    }

    if let Some(k) = kind {
        impl_symbols.retain(|s| s.kind == k);
    }
    impl_symbols.retain(|s| s.id != target.id);
    dedup_symbols(&mut impl_symbols);
    if let Some(lim) = limit {
        impl_symbols.truncate(lim);
    }

    if json_output {
        let (t_line, t_col) = offset_to_line_char_in_file(&target.file, target.start)?;
        let (t_end_line, t_end_col) = offset_to_line_char_in_file(&target.file, target.end)?;
        let json_implementations: Vec<serde_json::Value> = impl_symbols
            .iter()
            .filter_map(|sym| {
                let (line, col) = offset_to_line_char_in_file(&sym.file, sym.start).ok()?;
                let (end_line, end_col) = offset_to_line_char_in_file(&sym.file, sym.end).ok()?;
                Some(serde_json::json!({
                    "id": sym.id,
                    "name": sym.name,
                    "kind": sym.kind,
                    "file": sym.file,
                    "start": { "line": line, "character": col },
                    "end": { "line": end_line, "character": end_col },
                    "visibility": sym.visibility,
                    "container": sym.container
                }))
            })
            .collect();
        let output = serde_json::json!({
            "target": {
                "id": target.id,
                "name": target.name,
                "kind": target.kind,
                "file": target.file,
                "start": { "line": t_line, "character": t_col },
                "end": { "line": t_end_line, "character": t_end_col }
            },
            "implementations": json_implementations
        });
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
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

fn find_usages(
    db: &Path,
    file: &Path,
    line: Option<usize>,
    character: Option<usize>,
    limit: Option<usize>,
    json_output: bool,
) -> Result<()> {
    let store = store::IndexStore::open(db)?;
    let (file, line, character) = parse_file_position(file, line, character)?;
    let target = resolve_symbol_at(&store, &file, line, character)?;
    let workspace_root = workspace_root_from_db(db).unwrap_or_else(|_| {
        file.parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
    });

    let mut refs = store.references_for_symbol(&target.id)?;
    let mut seen = HashSet::new();
    // Filter out the definition span and deduplicate
    refs.retain(|r| {
        if r.file == target.file && r.start >= target.start && r.end <= target.end {
            return false;
        }
        seen.insert((r.file.clone(), r.start, r.end))
    });
    // If no useful references found, try name-based search
    if refs.is_empty() {
        refs = search_usages_by_name(&store, &target, &workspace_root)?;
        // Filter out definition span for fallback results too
        refs.retain(|r| {
            if r.file == target.file && r.start >= target.start && r.end <= target.end {
                return false;
            }
            seen.insert((r.file.clone(), r.start, r.end))
        });
    }
    if let Some(lim) = limit {
        refs.truncate(lim);
    }

    // Group references by file
    let mut by_file: std::collections::BTreeMap<String, Vec<&store::ReferenceRecord>> =
        std::collections::BTreeMap::new();
    for r in &refs {
        by_file.entry(r.file.clone()).or_default().push(r);
    }

    if json_output {
        let (t_line, t_col) = offset_to_line_char_in_file(&target.file, target.start)?;
        let (t_end_line, t_end_col) = offset_to_line_char_in_file(&target.file, target.end)?;

        let mut test_count = 0;
        let mut prod_count = 0;
        let mut total_usages = 0;

        let json_files: Vec<serde_json::Value> = by_file
            .iter()
            .filter_map(|(file, file_refs)| {
                let is_test = is_test_file(file);
                let usages: Vec<serde_json::Value> = file_refs
                    .iter()
                    .filter_map(|r| {
                        let (line, col) = offset_to_line_char_in_file(&r.file, r.start).ok()?;
                        let (end_line, end_col) =
                            offset_to_line_char_in_file(&r.file, r.end).ok()?;
                        Some(serde_json::json!({
                            "start": { "line": line, "character": col },
                            "end": { "line": end_line, "character": end_col }
                        }))
                    })
                    .collect();

                if is_test {
                    test_count += usages.len();
                } else {
                    prod_count += usages.len();
                }
                total_usages += usages.len();

                Some(serde_json::json!({
                    "file": file,
                    "context": if is_test { "test" } else { "prod" },
                    "count": usages.len(),
                    "usages": usages
                }))
            })
            .collect();

        let output = serde_json::json!({
            "target": {
                "id": target.id,
                "name": target.name,
                "kind": target.kind,
                "file": target.file,
                "start": { "line": t_line, "character": t_col },
                "end": { "line": t_end_line, "character": t_end_col }
            },
            "files": json_files,
            "summary": {
                "total": total_usages,
                "files": by_file.len(),
                "prod": prod_count,
                "test": test_count
            }
        });
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }

    let (t_line, t_col) = offset_to_line_char_in_file(&target.file, target.start)?;
    let (t_end_line, t_end_col) = offset_to_line_char_in_file(&target.file, target.end)?;
    println!(
        "Target: {} {} {} [{}:{}-{}:{}]",
        target.kind, target.name, target.file, t_line, t_col, t_end_line, t_end_col
    );
    if refs.is_empty() {
        println!("No usages found.");
    } else {
        let mut test_count = 0;
        let mut prod_count = 0;

        for (file, file_refs) in &by_file {
            let is_test = is_test_file(file);
            let context = if is_test { "[test]" } else { "[prod]" };
            if is_test {
                test_count += file_refs.len();
            } else {
                prod_count += file_refs.len();
            }

            println!("\n{} {} ({} usages)", context, file, file_refs.len());
            for r in file_refs {
                let (line, col) = offset_to_line_char_in_file(&r.file, r.start)?;
                let (end_line, end_col) = offset_to_line_char_in_file(&r.file, r.end)?;
                println!("  {}:{}-{}:{}", line, col, end_line, end_col);
            }
        }
        println!(
            "\nSummary: {} usages in {} files ({} prod, {} test)",
            refs.len(),
            by_file.len(),
            prod_count,
            test_count
        );
    }

    Ok(())
}

fn show_symbol(
    db: &Path,
    name: &str,
    file: Option<&PathBuf>,
    kind: Option<&str>,
    limit: Option<usize>,
    json_output: bool,
) -> Result<()> {
    let store = store::IndexStore::open(db)?;
    let file_str = file.map(|p| p.to_string_lossy().to_string());
    let symbols = store.list_symbols(file_str.as_deref(), kind, Some(name), limit)?;

    if symbols.is_empty() {
        if json_output {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({ "symbols": [] }))?
            );
        } else {
            println!("No symbols found for name '{}'.", name);
        }
        return Ok(());
    }

    let workspace_root = workspace_root_from_db(db)
        .unwrap_or_else(|_| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    if json_output {
        let json_symbols: Vec<serde_json::Value> = symbols
            .iter()
            .filter_map(|sym| {
                let (line, col) = offset_to_line_char_in_file(&sym.file, sym.start).ok()?;
                let (end_line, end_col) = offset_to_line_char_in_file(&sym.file, sym.end).ok()?;
                let outgoing = store.edges_from(&sym.id).ok()?;
                let incoming = store.edges_to(&sym.id).ok()?;
                let mut refs = store.references_for_symbol(&sym.id).ok()?;
                if refs.is_empty() {
                    refs = search_usages_by_name(&store, sym, &workspace_root).ok()?;
                }

                let json_refs: Vec<serde_json::Value> = refs
                    .iter()
                    .filter_map(|r| {
                        let (r_line, r_col) = offset_to_line_char_in_file(&r.file, r.start).ok()?;
                        let (r_end_line, r_end_col) =
                            offset_to_line_char_in_file(&r.file, r.end).ok()?;
                        Some(serde_json::json!({
                            "file": r.file,
                            "start": { "line": r_line, "character": r_col },
                            "end": { "line": r_end_line, "character": r_end_col }
                        }))
                    })
                    .collect();

                Some(serde_json::json!({
                    "id": sym.id,
                    "name": sym.name,
                    "kind": sym.kind,
                    "file": sym.file,
                    "start": { "line": line, "character": col },
                    "end": { "line": end_line, "character": end_col },
                    "visibility": sym.visibility,
                    "container": sym.container,
                    "qualifier": sym.qualifier,
                    "outgoing_edges": outgoing.iter().map(|e| serde_json::json!({
                        "src": e.src,
                        "dst": e.dst,
                        "kind": e.kind
                    })).collect::<Vec<_>>(),
                    "incoming_edges": incoming.iter().map(|e| serde_json::json!({
                        "src": e.src,
                        "dst": e.dst,
                        "kind": e.kind
                    })).collect::<Vec<_>>(),
                    "references": json_refs
                }))
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({ "symbols": json_symbols }))?
        );
        return Ok(());
    }

    for sym in symbols.iter() {
        let (line, col) = offset_to_line_char_in_file(&sym.file, sym.start)?;
        let (end_line, end_col) = offset_to_line_char_in_file(&sym.file, sym.end)?;
        let qualifier = sym.qualifier.as_deref().unwrap_or("");
        let visibility = sym.visibility.as_deref().unwrap_or("");
        let container = sym.container.as_deref().unwrap_or("");
        println!(
            "Symbol: {} {} {} [{}:{}-{}:{}] vis={} container={}",
            sym.kind, sym.name, sym.file, line, col, end_line, end_col, visibility, container
        );
        if !qualifier.is_empty() {
            println!("  qualifier: {}", qualifier);
        }
        let outgoing = store.edges_from(&sym.id)?;
        if !outgoing.is_empty() {
            println!("  outgoing edges:");
            for e in outgoing {
                println!("    {} -> {} ({})", e.src, e.dst, e.kind);
            }
        }
        let incoming = store.edges_to(&sym.id)?;
        if !incoming.is_empty() {
            println!("  incoming edges:");
            for e in incoming {
                println!("    {} -> {} ({})", e.src, e.dst, e.kind);
            }
        }
        let mut refs = store.references_for_symbol(&sym.id)?;
        if refs.is_empty() {
            refs = search_usages_by_name(&store, sym, &workspace_root)?;
        }
        if !refs.is_empty() {
            println!("  references:");
            for r in refs {
                let (r_line, r_col) = offset_to_line_char_in_file(&r.file, r.start)?;
                let (r_end_line, r_end_col) = offset_to_line_char_in_file(&r.file, r.end)?;
                println!(
                    "    {} [{}:{}-{}:{}]",
                    r.file, r_line, r_col, r_end_line, r_end_col
                );
            }
        }
    }

    Ok(())
}

fn find_definition(
    db: &Path,
    file: &Path,
    line: Option<usize>,
    character: Option<usize>,
    json_output: bool,
) -> Result<()> {
    let store = store::IndexStore::open(db)?;
    let (file, line, character) = parse_file_position(file, line, character)?;
    let canonical_file = file.canonicalize().unwrap_or_else(|_| file.to_path_buf());
    let file_str = normalize_path(&canonical_file);
    let contents = fs::read(&canonical_file)?;
    let offset = line_char_to_offset(&contents, line, character)
        .ok_or_else(|| anyhow!("could not map line/character to byte offset"))?
        as i64;

    // First, check if cursor is on a recorded reference - if so, look up its target symbol
    let definition = if let Some(ref_record) = store.reference_at_position(&file_str, offset)? {
        // Found a reference - look up the symbol it points to
        let symbols = store.symbols_by_ids(&[ref_record.symbol_id.clone()])?;
        if let Some(sym) = symbols.into_iter().next() {
            sym
        } else {
            // Reference exists but symbol not found - fall back to resolve_symbol_at
            resolve_symbol_at(&store, &file, line, character)?
        }
    } else {
        // No reference at position - use standard resolution
        resolve_symbol_at(&store, &file, line, character)?
    };

    if json_output {
        let (d_line, d_col) = offset_to_line_char_in_file(&definition.file, definition.start)?;
        let (d_end_line, d_end_col) =
            offset_to_line_char_in_file(&definition.file, definition.end)?;
        let output = serde_json::json!({
            "definition": {
                "id": definition.id,
                "name": definition.name,
                "kind": definition.kind,
                "file": definition.file,
                "start": { "line": d_line, "character": d_col },
                "end": { "line": d_end_line, "character": d_end_col },
                "visibility": definition.visibility,
                "container": definition.container,
                "qualifier": definition.qualifier
            }
        });
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }

    let (d_line, d_col) = offset_to_line_char_in_file(&definition.file, definition.start)?;
    let (d_end_line, d_end_col) = offset_to_line_char_in_file(&definition.file, definition.end)?;
    let container = definition
        .container
        .as_deref()
        .map(|c| format!(" in {c}"))
        .unwrap_or_default();
    println!(
        "Definition: {} {} {} [{}:{}-{}:{}]{}",
        definition.kind,
        definition.name,
        definition.file,
        d_line,
        d_col,
        d_end_line,
        d_end_col,
        container
    );

    Ok(())
}

fn find_duplicates(
    db: &Path,
    uncommitted: bool,
    staged: bool,
    kind: Option<&str>,
    min_count: usize,
    json_output: bool,
) -> Result<()> {
    let store = store::IndexStore::open(db)?;
    let workspace_root = workspace_root_from_db(db)?;

    // Get file filter based on git flags
    let file_filter: Option<Vec<String>> = if uncommitted || staged {
        let files = get_git_changed_files(&workspace_root, uncommitted, staged)?;
        if files.is_empty() {
            if json_output {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "groups": [],
                        "summary": { "total_groups": 0, "total_duplicates": 0 }
                    }))?
                );
            } else {
                println!("No changed files found.");
            }
            return Ok(());
        }
        Some(files)
    } else {
        None
    };

    let groups = store.find_duplicate_groups(min_count, kind, file_filter.as_deref())?;

    if json_output {
        let json_groups: Vec<serde_json::Value> = groups
            .iter()
            .map(|group| {
                let symbols: Vec<serde_json::Value> = group
                    .symbols
                    .iter()
                    .filter_map(|sym| {
                        let (line, col) = offset_to_line_char_in_file(&sym.file, sym.start).ok()?;
                        let (end_line, end_col) =
                            offset_to_line_char_in_file(&sym.file, sym.end).ok()?;
                        Some(serde_json::json!({
                            "id": sym.id,
                            "name": sym.name,
                            "kind": sym.kind,
                            "file": sym.file,
                            "start": { "line": line, "character": col },
                            "end": { "line": end_line, "character": end_col },
                            "container": sym.container
                        }))
                    })
                    .collect();
                serde_json::json!({
                    "content_hash": group.content_hash,
                    "count": group.symbols.len(),
                    "symbols": symbols
                })
            })
            .collect();

        let total_duplicates: usize = groups.iter().map(|g| g.symbols.len()).sum();
        let output = serde_json::json!({
            "groups": json_groups,
            "summary": {
                "total_groups": groups.len(),
                "total_duplicates": total_duplicates
            }
        });
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }

    if groups.is_empty() {
        println!("No duplicates found.");
        return Ok(());
    }

    let total_duplicates: usize = groups.iter().map(|g| g.symbols.len()).sum();
    println!(
        "Found {} duplicate groups ({} total symbols)\n",
        groups.len(),
        total_duplicates
    );

    for (i, group) in groups.iter().enumerate() {
        println!(
            "Group {} ({} duplicates, hash: {}):",
            i + 1,
            group.symbols.len(),
            &group.content_hash[..8]
        );
        for sym in &group.symbols {
            let (line, col) = offset_to_line_char_in_file(&sym.file, sym.start)?;
            let (end_line, end_col) = offset_to_line_char_in_file(&sym.file, sym.end)?;
            let container = sym
                .container
                .as_deref()
                .map(|c| format!(" in {c}"))
                .unwrap_or_default();
            println!(
                "  {:<10} {:<30} {} [{}:{}-{}:{}]{container}",
                sym.kind, sym.name, sym.file, line, col, end_line, end_col
            );
        }
        println!();
    }

    Ok(())
}

/// Get list of changed files from git.
/// If `uncommitted` is true, includes working tree changes (unstaged + staged).
/// If `staged` is true, includes only staged changes.
fn get_git_changed_files(
    workspace_root: &Path,
    uncommitted: bool,
    staged: bool,
) -> Result<Vec<String>> {
    use std::process::Command;

    let mut files = HashSet::new();

    if staged {
        // Get staged files only
        let output = Command::new("git")
            .args(["diff", "--cached", "--name-only"])
            .current_dir(workspace_root)
            .output()
            .context("failed to run git diff --cached")?;

        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                if !line.is_empty() {
                    let full_path = workspace_root.join(line);
                    if let Ok(canonical) = full_path.canonicalize() {
                        files.insert(normalize_path(&canonical));
                    }
                }
            }
        }
    }

    if uncommitted {
        // Get all uncommitted changes (staged + unstaged)
        let output = Command::new("git")
            .args(["diff", "--name-only", "HEAD"])
            .current_dir(workspace_root)
            .output()
            .context("failed to run git diff HEAD")?;

        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                if !line.is_empty() {
                    let full_path = workspace_root.join(line);
                    if let Ok(canonical) = full_path.canonicalize() {
                        files.insert(normalize_path(&canonical));
                    }
                }
            }
        }

        // Also get untracked files
        let output = Command::new("git")
            .args(["ls-files", "--others", "--exclude-standard"])
            .current_dir(workspace_root)
            .output()
            .context("failed to run git ls-files")?;

        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                if !line.is_empty() {
                    let full_path = workspace_root.join(line);
                    if let Ok(canonical) = full_path.canonicalize() {
                        files.insert(normalize_path(&canonical));
                    }
                }
            }
        }
    }

    Ok(files.into_iter().collect())
}

fn resolve_symbol_at(
    store: &store::IndexStore,
    file: &Path,
    line: usize,
    character: usize,
) -> Result<SymbolRecord> {
    let canonical_file = file.canonicalize().unwrap_or_else(|_| file.to_path_buf());
    let file_str = normalize_path(&canonical_file);
    let symbols = store.list_symbols(Some(&file_str), None, None, None)?;
    let contents = fs::read(&canonical_file)?;
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

fn dedup_symbols(symbols: &mut Vec<SymbolRecord>) {
    let mut seen = HashSet::new();
    symbols.retain(|s| seen.insert(s.id.clone()));
}

fn search_usages_by_name(
    store: &store::IndexStore,
    target: &SymbolRecord,
    _workspace_root: &Path,
) -> Result<Vec<store::ReferenceRecord>> {
    let mut refs = Vec::new();

    // Use dependency graph: only search files that depend on the target's file
    let dependents = store.get_dependents(&target.file)?;
    let paths: Vec<PathBuf> = if dependents.is_empty() {
        // No dependency info - fall back to all indexed files
        store.list_paths()?.into_iter().map(PathBuf::from).collect()
    } else {
        // Search dependents plus the target's own file
        let mut paths: Vec<PathBuf> = dependents.into_iter().map(PathBuf::from).collect();
        paths.push(PathBuf::from(&target.file));
        paths
    };

    for path in paths {
        let canonical = path.canonicalize().unwrap_or(path.clone());
        let path_str = normalize_path(&canonical);
        let buf = match fs::read(&canonical) {
            Ok(b) => b,
            Err(_) => continue,
        };
        let mut idx = 0usize;
        while let Some(pos) = buf[idx..]
            .windows(target.name.len())
            .position(|w| w == target.name.as_bytes())
        {
            let abs = idx + pos;
            if !is_word_boundary(&buf, abs, target.name.len()) {
                idx += pos + target.name.len();
                continue;
            }
            let abs = idx + pos;
            let start = abs as i64;
            let end = (abs + target.name.len()) as i64;
            if path_str == target.file && start >= target.start && end <= target.end {
                idx += pos + target.name.len();
                continue;
            }
            refs.push(store::ReferenceRecord {
                file: path_str.clone(),
                start,
                end,
                symbol_id: target.id.clone(),
            });
            idx += pos + target.name.len();
        }
    }
    Ok(refs)
}

fn is_word_boundary(buf: &[u8], start: usize, len: usize) -> bool {
    let is_ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let before_ok = if start == 0 {
        true
    } else {
        !is_ident(buf[start - 1])
    };
    let end = start + len;
    let after_ok = if end >= buf.len() {
        true
    } else {
        !is_ident(buf[end])
    };
    before_ok && after_ok
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

/// Check if a file path indicates a test file based on common conventions.
/// Returns true for:
/// - Files in `tests/`, `__tests__/`, `test/` directories
/// - Files matching `*_test.rs`, `*_spec.rs` (Rust)
/// - Files matching `*.test.ts`, `*.spec.ts`, `*.test.tsx`, `*.spec.tsx` (TypeScript)
fn is_test_file(path: &str) -> bool {
    let path_lower = path.to_lowercase();

    // Check directory patterns
    if path_lower.contains("/tests/")
        || path_lower.contains("/__tests__/")
        || path_lower.contains("/test/")
        || path_lower.contains("/spec/")
    {
        return true;
    }

    // Check file name patterns
    if let Some(file_name) = path.rsplit('/').next() {
        let name_lower = file_name.to_lowercase();
        // Rust patterns
        if name_lower.ends_with("_test.rs") || name_lower.ends_with("_spec.rs") {
            return true;
        }
        // TypeScript/JavaScript patterns
        if name_lower.ends_with(".test.ts")
            || name_lower.ends_with(".spec.ts")
            || name_lower.ends_with(".test.tsx")
            || name_lower.ends_with(".spec.tsx")
            || name_lower.ends_with(".test.js")
            || name_lower.ends_with(".spec.js")
            || name_lower.ends_with(".test.jsx")
            || name_lower.ends_with(".spec.jsx")
        {
            return true;
        }
    }

    false
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

fn workspace_root_from_db(db: &Path) -> Result<PathBuf> {
    let abs = if db.is_absolute() {
        db.to_path_buf()
    } else {
        env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(db)
    };
    let path = abs.canonicalize().unwrap_or(abs);
    if let Some(parent) = path.parent() {
        if parent.file_name().and_then(|n| n.to_str()) == Some(".gabb") {
            if let Some(root) = parent.parent() {
                return Ok(root.to_path_buf());
            }
        }
        return Ok(parent.to_path_buf());
    }
    Err(anyhow!("could not derive workspace root from db path"))
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

    #[test]
    fn usages_skip_definition_span() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let file_path = root.join("foo.ts");
        let source = "function foo() {}\nfoo();\n";
        fs::write(&file_path, source).unwrap();
        let file_path = file_path.canonicalize().unwrap();

        let db_path = root.join(".gabb/index.db");
        let store = IndexStore::open(&db_path).unwrap();
        indexer::build_full_index(root, &store).unwrap();

        let symbol = resolve_symbol_at(&store, &file_path, 1, 10).unwrap();
        let root = db_path.parent().and_then(|p| p.parent()).unwrap_or(root);
        let refs = super::search_usages_by_name(&store, &symbol, root).unwrap();
        assert!(
            refs.iter().all(|r| !(r.file == symbol.file
                && r.start >= symbol.start
                && r.end <= symbol.end)),
            "should not return reference within definition"
        );
    }

    #[test]
    fn finds_usages_across_files_via_fallback() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let def_path = root.join("lib.rs");
        let caller_path = root.join("main.rs");
        fs::write(
            &def_path,
            r#"
                pub fn build_full_index() {}
            "#,
        )
        .unwrap();
        fs::write(
            &caller_path,
            r#"
                fn main() {
                    build_full_index();
                }
            "#,
        )
        .unwrap();

        let db_path = root.join(".gabb/index.db");
        let store = IndexStore::open(&db_path).unwrap();
        indexer::build_full_index(root, &store).unwrap();

        let def_path = def_path.canonicalize().unwrap();
        let src = fs::read_to_string(&def_path).unwrap();
        let offset = src.find("build_full_index").unwrap();
        let (line, character) = offset_to_line_char(src.as_bytes(), offset).unwrap();
        let symbol = resolve_symbol_at(&store, &def_path, line, character).unwrap();
        let refs = super::search_usages_by_name(&store, &symbol, root).unwrap();
        assert!(
            refs.iter().any(|r| r.file.ends_with("main.rs")),
            "expected a usage in main.rs, got {:?}",
            refs
        );
    }

    #[test]
    fn parses_line_character_from_file_arg() {
        let file = PathBuf::from("src/daemon.rs:18:5");
        let (path, line, character) = parse_file_position(file.as_path(), None, None).unwrap();
        assert_eq!(path, PathBuf::from("src/daemon.rs"));
        assert_eq!(line, 18);
        assert_eq!(character, 5);

        // Explicit args override embedded position.
        let (path2, line2, character2) =
            parse_file_position(file.as_path(), Some(1), Some(2)).unwrap();
        assert_eq!(path2, PathBuf::from("src/daemon.rs"));
        assert_eq!(line2, 1);
        assert_eq!(character2, 2);
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

    #[test]
    fn detects_test_files_correctly() {
        // Directory-based patterns
        assert!(is_test_file("/project/tests/foo.rs"));
        assert!(is_test_file("/project/src/__tests__/component.test.ts"));
        assert!(is_test_file("/project/test/helper.ts"));
        assert!(is_test_file("/project/spec/models.spec.ts"));

        // Rust file patterns
        assert!(is_test_file("/project/src/indexer_test.rs"));
        assert!(is_test_file("/project/src/store_spec.rs"));

        // TypeScript/JavaScript file patterns
        assert!(is_test_file("/project/src/utils.test.ts"));
        assert!(is_test_file("/project/src/utils.spec.ts"));
        assert!(is_test_file("/project/src/component.test.tsx"));
        assert!(is_test_file("/project/src/component.spec.tsx"));
        assert!(is_test_file("/project/src/helper.test.js"));
        assert!(is_test_file("/project/src/helper.spec.jsx"));

        // Production files (should return false)
        assert!(!is_test_file("/project/src/main.rs"));
        assert!(!is_test_file("/project/src/lib.rs"));
        assert!(!is_test_file("/project/src/utils.ts"));
        assert!(!is_test_file("/project/src/component.tsx"));
        assert!(!is_test_file("/project/src/index.js"));

        // Edge cases
        assert!(!is_test_file("/project/src/testing.ts")); // "testing" != "test"
        assert!(!is_test_file("/project/src/contest.ts")); // contains "test" but not a test file
    }
}
fn parse_file_position(
    file: &Path,
    line: Option<usize>,
    character: Option<usize>,
) -> Result<(PathBuf, usize, usize)> {
    let (base, embedded) = split_file_and_embedded_position(file);
    if let (Some(l), Some(c)) = (line, character) {
        return Ok((base, l, c));
    }
    if let Some((l, c)) = embedded {
        return Ok((base, l, c));
    }

    Err(anyhow!(
        "must provide --line and --character or include :line:character in --file"
    ))
}

fn split_file_and_embedded_position(file: &Path) -> (PathBuf, Option<(usize, usize)>) {
    let raw = file.to_string_lossy();
    let parts: Vec<&str> = raw.split(':').collect();
    if parts.len() >= 3 {
        if let (Ok(line), Ok(character)) = (
            parts[parts.len() - 2].parse::<usize>(),
            parts[parts.len() - 1].parse::<usize>(),
        ) {
            let base = parts[..parts.len() - 2].join(":");
            return (PathBuf::from(base), Some((line, character)));
        }
    }
    (file.to_path_buf(), None)
}
