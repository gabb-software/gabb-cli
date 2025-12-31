//! Query commands: symbols, symbol, definition, implementation, usages.

use anyhow::{anyhow, Result};
use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use gabb_cli::daemon;
use gabb_cli::store::{normalize_path, SymbolQuery, SymbolRecord};
use gabb_cli::ExitCode;
use gabb_cli::OutputFormat;

use crate::output::{
    self, DefinitionOutput, EdgeOutput, Position, ReferenceOutput, SourceDisplayOptions,
    SymbolDetailOutput, SymbolOutput,
};
use crate::util::{
    self, dedup_symbols, line_char_to_offset, open_store_for_query, parse_file_position,
    resolve_symbol_at, search_usages_by_name,
};

#[allow(clippy::too_many_arguments)]
pub fn list_symbols(
    db: &Path,
    file: Option<&PathBuf>,
    kind: Option<&str>,
    name: Option<&str>,
    fuzzy: bool,
    limit: Option<usize>,
    offset: Option<usize>,
    format: OutputFormat,
    source_opts: SourceDisplayOptions,
    _quiet: bool,
) -> Result<ExitCode> {
    let store = open_store_for_query(db)?;
    let file_str = file.map(|p| p.to_string_lossy().to_string());

    let mut symbols: Vec<SymbolRecord> = match (fuzzy, name) {
        (true, Some(query)) => {
            // Use FTS5 search for fuzzy matching
            let mut results = store.search_symbols_fts(query)?;

            // Apply additional filters
            if let Some(f) = &file_str {
                results.retain(|s| s.file == *f);
            }
            if let Some(k) = kind {
                results.retain(|s| s.kind == k);
            }
            // Apply offset
            if let Some(off) = offset {
                if off < results.len() {
                    results = results.into_iter().skip(off).collect();
                } else {
                    results.clear();
                }
            }
            if let Some(l) = limit {
                results.truncate(l);
            }
            results
        }
        _ => {
            let query = SymbolQuery {
                file: file_str.as_deref(),
                kind,
                name,
                limit,
                offset,
                ..Default::default()
            };
            store.list_symbols_filtered(&query)?
        }
    };

    // Sort by name for consistent output
    if fuzzy {
        symbols.sort_by(|a, b| a.name.cmp(&b.name));
    }

    let found = !symbols.is_empty();
    output::output_symbols(&symbols, format, source_opts)?;
    Ok(if found {
        ExitCode::Success
    } else {
        ExitCode::NotFound
    })
}

#[allow(clippy::too_many_arguments)]
pub fn find_implementation(
    db: &Path,
    file: &Path,
    line: Option<usize>,
    character: Option<usize>,
    limit: Option<usize>,
    kind: Option<&str>,
    format: OutputFormat,
    source_opts: SourceDisplayOptions,
    _quiet: bool,
) -> Result<ExitCode> {
    let store = open_store_for_query(db)?;
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

    let found = !impl_symbols.is_empty();
    output::output_implementations(&target, &impl_symbols, format, source_opts)?;
    Ok(if found {
        ExitCode::Success
    } else {
        ExitCode::NotFound
    })
}

#[allow(clippy::too_many_arguments)]
pub fn find_usages(
    db: &Path,
    file: &Path,
    line: Option<usize>,
    character: Option<usize>,
    limit: Option<usize>,
    format: OutputFormat,
    source_opts: SourceDisplayOptions,
    _quiet: bool,
) -> Result<ExitCode> {
    let store = open_store_for_query(db)?;
    let (file, line, character) = parse_file_position(file, line, character)?;
    let target = resolve_symbol_at(&store, &file, line, character)?;
    let workspace_root = daemon::workspace_root_from_db(db).unwrap_or_else(|_| {
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

    let found = !refs.is_empty();
    output::output_usages(&target, &refs, &store, format, source_opts)?;
    Ok(if found {
        ExitCode::Success
    } else {
        ExitCode::NotFound
    })
}

#[allow(clippy::too_many_arguments)]
pub fn show_symbol(
    db: &Path,
    name: &str,
    file: Option<&PathBuf>,
    kind: Option<&str>,
    fuzzy: bool,
    limit: Option<usize>,
    format: OutputFormat,
    source_opts: SourceDisplayOptions,
    quiet: bool,
) -> Result<ExitCode> {
    let store = open_store_for_query(db)?;
    let file_str = file.map(|p| p.to_string_lossy().to_string());

    let symbols = if fuzzy {
        // Use FTS5 search for fuzzy matching
        let mut results = store.search_symbols_fts(name)?;

        // Apply additional filters
        if let Some(f) = &file_str {
            results.retain(|s| s.file == *f);
        }
        if let Some(k) = kind {
            results.retain(|s| s.kind == k);
        }
        if let Some(l) = limit {
            results.truncate(l);
        }
        results
    } else {
        store.list_symbols(file_str.as_deref(), kind, Some(name), limit)?
    };

    if symbols.is_empty() {
        match format {
            OutputFormat::Json => {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({ "symbols": [] }))?
                );
            }
            OutputFormat::Jsonl => {} // No output for empty results
            OutputFormat::Csv | OutputFormat::Tsv => {
                // Just headers for empty results
                let sep = if matches!(format, OutputFormat::Csv) {
                    ","
                } else {
                    "\t"
                };
                println!(
                    "name{}kind{}location{}visibility{}container",
                    sep, sep, sep, sep
                );
            }
            OutputFormat::Text => {
                if !quiet {
                    println!("No symbols found for name '{}'.", name);
                }
            }
        }
        return Ok(ExitCode::NotFound);
    }

    let workspace_root = daemon::workspace_root_from_db(db)
        .unwrap_or_else(|_| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    // Build detailed output for each symbol
    let detailed_symbols: Vec<SymbolDetailOutput> = symbols
        .iter()
        .filter_map(|sym| {
            let base = SymbolOutput::from_record_with_source(sym, source_opts)?;
            let outgoing = store.edges_from(&sym.id).ok()?;
            let incoming = store.edges_to(&sym.id).ok()?;
            let mut refs = store.references_for_symbol(&sym.id).ok()?;
            if refs.is_empty() {
                refs = search_usages_by_name(&store, sym, &workspace_root).ok()?;
            }

            let outgoing_edges: Vec<EdgeOutput> = outgoing
                .iter()
                .map(|e| EdgeOutput {
                    src: e.src.clone(),
                    dst: e.dst.clone(),
                    kind: e.kind.clone(),
                })
                .collect();

            let incoming_edges: Vec<EdgeOutput> = incoming
                .iter()
                .map(|e| EdgeOutput {
                    src: e.src.clone(),
                    dst: e.dst.clone(),
                    kind: e.kind.clone(),
                })
                .collect();

            let references: Vec<ReferenceOutput> = refs
                .iter()
                .filter_map(|r| {
                    let (r_line, r_col) =
                        util::offset_to_line_char_in_file(&r.file, r.start).ok()?;
                    let (r_end_line, r_end_col) =
                        util::offset_to_line_char_in_file(&r.file, r.end).ok()?;
                    Some(ReferenceOutput {
                        file: r.file.clone(),
                        start: Position {
                            line: r_line,
                            character: r_col,
                        },
                        end: Position {
                            line: r_end_line,
                            character: r_end_col,
                        },
                    })
                })
                .collect();

            Some(SymbolDetailOutput {
                base,
                outgoing_edges,
                incoming_edges,
                references,
            })
        })
        .collect();

    match format {
        OutputFormat::Json => {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({ "symbols": detailed_symbols }))?
            );
        }
        OutputFormat::Jsonl => {
            for sym in &detailed_symbols {
                println!("{}", serde_json::to_string(sym)?);
            }
        }
        OutputFormat::Csv => {
            let mut wtr = csv::Writer::from_writer(std::io::stdout());
            wtr.write_record([
                "name",
                "kind",
                "location",
                "visibility",
                "container",
                "outgoing_edges",
                "incoming_edges",
                "references_count",
            ])?;
            for sym in &detailed_symbols {
                wtr.write_record([
                    sym.base.name.as_str(),
                    sym.base.kind.as_str(),
                    &sym.base.location(),
                    sym.base.visibility.as_deref().unwrap_or(""),
                    sym.base.container.as_deref().unwrap_or(""),
                    &sym.outgoing_edges.len().to_string(),
                    &sym.incoming_edges.len().to_string(),
                    &sym.references.len().to_string(),
                ])?;
            }
            wtr.flush()?;
        }
        OutputFormat::Tsv => {
            println!("name\tkind\tlocation\tvisibility\tcontainer\toutgoing_edges\tincoming_edges\treferences_count");
            for sym in &detailed_symbols {
                println!(
                    "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                    sym.base.name,
                    sym.base.kind,
                    sym.base.location(),
                    sym.base.visibility.as_deref().unwrap_or(""),
                    sym.base.container.as_deref().unwrap_or(""),
                    sym.outgoing_edges.len(),
                    sym.incoming_edges.len(),
                    sym.references.len()
                );
            }
        }
        OutputFormat::Text => {
            for sym in &detailed_symbols {
                let visibility = sym.base.visibility.as_deref().unwrap_or("");
                let container = sym.base.container.as_deref().unwrap_or("");
                println!(
                    "Symbol: {} {} {} vis={} container={}",
                    sym.base.kind,
                    sym.base.name,
                    sym.base.location(),
                    visibility,
                    container
                );
                if let Some(qualifier) = &sym.base.qualifier {
                    println!("  qualifier: {}", qualifier);
                }
                if let Some(src) = &sym.base.source {
                    println!("{}\n", src);
                }
                if !sym.outgoing_edges.is_empty() {
                    println!("  outgoing edges:");
                    for e in &sym.outgoing_edges {
                        println!("    {} -> {} ({})", e.src, e.dst, e.kind);
                    }
                }
                if !sym.incoming_edges.is_empty() {
                    println!("  incoming edges:");
                    for e in &sym.incoming_edges {
                        println!("    {} -> {} ({})", e.src, e.dst, e.kind);
                    }
                }
                if !sym.references.is_empty() {
                    println!("  references:");
                    for r in &sym.references {
                        println!(
                            "    {}:{}:{}-{}:{}",
                            r.file, r.start.line, r.start.character, r.end.line, r.end.character
                        );
                    }
                }
            }
        }
    }

    Ok(ExitCode::Success)
}

pub fn find_definition(
    db: &Path,
    file: &Path,
    line: Option<usize>,
    character: Option<usize>,
    format: OutputFormat,
    source_opts: SourceDisplayOptions,
    _quiet: bool,
) -> Result<ExitCode> {
    let store = open_store_for_query(db)?;
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
        let symbols = store.symbols_by_ids(std::slice::from_ref(&ref_record.symbol_id))?;
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

    let def_out = SymbolOutput::from_record_with_source(&definition, source_opts)
        .ok_or_else(|| anyhow!("Failed to resolve definition position"))?;

    match format {
        OutputFormat::Json => {
            let output = DefinitionOutput {
                definition: def_out,
            };
            println!("{}", serde_json::to_string_pretty(&output)?);
        }
        OutputFormat::Jsonl => {
            println!("{}", serde_json::to_string(&def_out)?);
        }
        OutputFormat::Csv => {
            let mut wtr = csv::Writer::from_writer(std::io::stdout());
            wtr.write_record(["name", "kind", "location", "visibility", "container"])?;
            wtr.write_record(def_out.to_row())?;
            wtr.flush()?;
        }
        OutputFormat::Tsv => {
            println!("name\tkind\tlocation\tvisibility\tcontainer");
            println!("{}", def_out.to_row().join("\t"));
        }
        OutputFormat::Text => {
            let container = def_out
                .container
                .as_deref()
                .map(|c| format!(" in {c}"))
                .unwrap_or_default();
            println!(
                "Definition: {} {} {}{}",
                def_out.kind,
                def_out.name,
                def_out.location(),
                container
            );
            if let Some(src) = &def_out.source {
                println!("{}", src);
            }
        }
    }

    // Definition always found if we reach here (errors are returned early)
    Ok(ExitCode::Success)
}
