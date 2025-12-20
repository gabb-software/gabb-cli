use crate::languages::{cpp, kotlin, rust, typescript, ImportBindingInfo};
use crate::store::{normalize_path, now_unix, FileRecord, IndexStore, ReferenceRecord};
use anyhow::{bail, Context, Result};
use blake3::Hasher;
use log::{debug, info, warn};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use walkdir::{DirEntry, WalkDir};

const SKIP_DIRS: &[&str] = &[".git", ".gabb", "target", "node_modules"];

/// Progress information during indexing
#[derive(Debug, Clone)]
pub struct IndexProgress {
    /// Number of files indexed so far
    pub files_done: usize,
    /// Total number of files to index
    pub files_total: usize,
    /// Number of symbols found so far
    pub symbols_found: usize,
    /// Elapsed time in seconds
    pub elapsed_secs: f64,
    /// Files indexed per second (rolling average)
    pub files_per_sec: f64,
    /// Estimated seconds remaining (None if not enough data)
    pub eta_secs: Option<f64>,
    /// Current file being indexed (if any)
    pub current_file: Option<String>,
    /// Phase of indexing (scanning, parsing, resolving)
    pub phase: IndexPhase,
}

/// Phase of the indexing process
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexPhase {
    /// Scanning directory for files to index
    Scanning,
    /// Parsing files and extracting symbols (phase 1)
    Parsing,
    /// Resolving cross-file references (phase 2)
    Resolving,
    /// Finalizing (analyze, cleanup)
    Finalizing,
}

impl std::fmt::Display for IndexPhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IndexPhase::Scanning => write!(f, "Scanning"),
            IndexPhase::Parsing => write!(f, "Parsing"),
            IndexPhase::Resolving => write!(f, "Resolving"),
            IndexPhase::Finalizing => write!(f, "Finalizing"),
        }
    }
}

/// Summary of indexing results
#[derive(Debug, Clone)]
pub struct IndexSummary {
    /// Total files indexed
    pub files_indexed: usize,
    /// Total symbols found
    pub symbols_found: usize,
    /// Total duration in seconds
    pub duration_secs: f64,
    /// Average files per second
    pub files_per_sec: f64,
}

/// Collected data from first pass of indexing, before reference resolution
struct FirstPassData {
    file_path: String,
    references: Vec<ReferenceRecord>,
    import_bindings: Vec<ImportBindingInfo>,
}

/// Rebuild the index from scratch for a workspace root.
/// Uses two-phase indexing:
/// 1. First pass: parse all files, store symbols/edges/deps, collect unresolved references
/// 2. Resolution pass: resolve references using global symbol table + import bindings
///
/// If `progress_callback` is provided, it will be called periodically with progress updates.
pub fn build_full_index<F>(
    root: &Path,
    store: &IndexStore,
    progress_callback: Option<F>,
) -> Result<IndexSummary>
where
    F: Fn(&IndexProgress),
{
    let root = root
        .canonicalize()
        .with_context(|| format!("failed to canonicalize root {}", root.display()))?;
    info!("Starting full index at {}", root.display());

    let start_time = Instant::now();
    let mut total_symbols = 0usize;

    // Report scanning phase
    if let Some(ref cb) = progress_callback {
        cb(&IndexProgress {
            files_done: 0,
            files_total: 0,
            symbols_found: 0,
            elapsed_secs: 0.0,
            files_per_sec: 0.0,
            eta_secs: None,
            current_file: None,
            phase: IndexPhase::Scanning,
        });
    }

    // First, collect all files to index (scanning phase)
    let files_to_index: Vec<_> = WalkDir::new(&root)
        .into_iter()
        .filter_entry(|e| should_descend(e, &root))
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file() && is_indexed_file(e.path()))
        .map(|e| e.path().to_path_buf())
        .collect();

    let files_total = files_to_index.len();
    info!("Found {} files to index", files_total);

    let mut seen = HashSet::new();
    let mut first_pass_data: Vec<FirstPassData> = Vec::new();

    // Phase 1: Parse all files, store symbols/edges/deps, collect references for later resolution
    for (i, path) in files_to_index.iter().enumerate() {
        let elapsed = start_time.elapsed().as_secs_f64();
        let files_per_sec = if elapsed > 0.0 {
            i as f64 / elapsed
        } else {
            0.0
        };
        let remaining = files_total.saturating_sub(i);
        let eta_secs = if files_per_sec > 0.0 {
            Some(remaining as f64 / files_per_sec)
        } else {
            None
        };

        // Report progress
        if let Some(ref cb) = progress_callback {
            cb(&IndexProgress {
                files_done: i,
                files_total,
                symbols_found: total_symbols,
                elapsed_secs: elapsed,
                files_per_sec,
                eta_secs,
                current_file: Some(path.to_string_lossy().to_string()),
                phase: IndexPhase::Parsing,
            });
        }

        match index_first_pass(path, store) {
            Ok((norm_path, refs, imports, sym_count)) => {
                seen.insert(norm_path.clone());
                total_symbols += sym_count;
                first_pass_data.push(FirstPassData {
                    file_path: norm_path,
                    references: refs,
                    import_bindings: imports,
                });
            }
            Err(err) => warn!("indexing failed for {}: {err}", path.display()),
        }
    }

    // Report resolving phase
    if let Some(ref cb) = progress_callback {
        cb(&IndexProgress {
            files_done: files_total,
            files_total,
            symbols_found: total_symbols,
            elapsed_secs: start_time.elapsed().as_secs_f64(),
            files_per_sec: files_total as f64 / start_time.elapsed().as_secs_f64().max(0.001),
            eta_secs: None,
            current_file: None,
            phase: IndexPhase::Resolving,
        });
    }

    prune_deleted(store, &seen)?;

    // Phase 2: Build global symbol table and resolve references + edges
    let symbol_table = build_global_symbol_table(store)?;
    resolve_and_store_references(store, &first_pass_data, &symbol_table)?;
    resolve_edge_destinations(store, &symbol_table)?;

    // Report finalizing phase
    if let Some(ref cb) = progress_callback {
        cb(&IndexProgress {
            files_done: files_total,
            files_total,
            symbols_found: total_symbols,
            elapsed_secs: start_time.elapsed().as_secs_f64(),
            files_per_sec: files_total as f64 / start_time.elapsed().as_secs_f64().max(0.001),
            eta_secs: None,
            current_file: None,
            phase: IndexPhase::Finalizing,
        });
    }

    // Update query optimizer statistics for optimal index usage
    store.analyze()?;

    let duration_secs = start_time.elapsed().as_secs_f64();
    let files_per_sec = files_total as f64 / duration_secs.max(0.001);

    info!(
        "Full index complete: {} files, {} symbols in {:.1}s ({:.1} files/sec). DB at {}",
        files_total,
        total_symbols,
        duration_secs,
        files_per_sec,
        store.db_path().display()
    );

    Ok(IndexSummary {
        files_indexed: files_total,
        symbols_found: total_symbols,
        duration_secs,
        files_per_sec,
    })
}

/// Build a global symbol table mapping (file, name) -> symbol_id
fn build_global_symbol_table(store: &IndexStore) -> Result<HashMap<(String, String), String>> {
    let mut table = HashMap::new();
    let symbols = store.list_symbols(None, None, None, None)?;
    for sym in symbols {
        // Map by (file, name) for cross-file resolution
        table.insert((sym.file.clone(), sym.name.clone()), sym.id.clone());
        // Also map by (file_without_ext, name) for import resolution
        let file_without_ext = strip_extension(&sym.file);
        table.insert((file_without_ext, sym.name.clone()), sym.id);
    }
    Ok(table)
}

/// Strip file extension for matching against import qualifiers
fn strip_extension(path: &str) -> String {
    if let Some(dot_pos) = path.rfind('.') {
        if let Some(slash_pos) = path.rfind('/') {
            if dot_pos > slash_pos {
                return path[..dot_pos].to_string();
            }
        } else if dot_pos > 0 {
            return path[..dot_pos].to_string();
        }
    }
    path.to_string()
}

/// Resolve references using import bindings and global symbol table, then store them
fn resolve_and_store_references(
    store: &IndexStore,
    first_pass_data: &[FirstPassData],
    symbol_table: &HashMap<(String, String), String>,
) -> Result<()> {
    for data in first_pass_data {
        // Build local resolution map from import bindings
        // Map both local_name and original_name to the resolved symbol_id
        let mut local_resolution: HashMap<String, String> = HashMap::new();
        for binding in &data.import_bindings {
            // Try to resolve the imported symbol
            let resolved_id = symbol_table
                .get(&(binding.source_file.clone(), binding.original_name.clone()))
                .or_else(|| {
                    // Try without extension
                    let source_without_ext = strip_extension(&binding.source_file);
                    symbol_table.get(&(source_without_ext, binding.original_name.clone()))
                });

            if let Some(symbol_id) = resolved_id {
                // Map local name (the alias) to resolved ID
                local_resolution.insert(binding.local_name.clone(), symbol_id.clone());
                // Also map original name for placeholder resolution
                // (references contain the original name in their symbol_id)
                local_resolution.insert(binding.original_name.clone(), symbol_id.clone());
            }
        }

        // Resolve each reference
        let resolved_refs: Vec<ReferenceRecord> = data
            .references
            .iter()
            .map(|r| {
                // Check if this reference's symbol_id is a placeholder that needs resolution
                // Placeholder IDs typically contain "::" (e.g., "./utils::helper")
                if r.symbol_id.contains("::") && !r.symbol_id.contains('#') {
                    // Extract the name from the placeholder (last segment after ::)
                    let name = r.symbol_id.rsplit("::").next().unwrap_or(&r.symbol_id);
                    // Try local resolution first (import bindings)
                    if let Some(resolved_id) = local_resolution.get(name) {
                        return ReferenceRecord {
                            file: r.file.clone(),
                            start: r.start,
                            end: r.end,
                            symbol_id: resolved_id.clone(),
                        };
                    }
                }
                // Keep original if can't resolve
                r.clone()
            })
            .collect();

        // Store the resolved references
        store.save_references(&data.file_path, &resolved_refs)?;
    }
    Ok(())
}

/// Resolve edge destinations that use placeholder format ({qualifier}::{name})
/// to actual symbol IDs ({file}#{start}-{end}).
fn resolve_edge_destinations(
    store: &IndexStore,
    symbol_table: &HashMap<(String, String), String>,
) -> Result<()> {
    let unresolved_edges = store.get_unresolved_edges()?;
    let mut resolved_count = 0;

    for edge in unresolved_edges {
        // Edge dst format is "{qualifier}::{name}" e.g., "/path/to/interface::Service"
        if let Some(double_colon_pos) = edge.dst.rfind("::") {
            let qualifier = &edge.dst[..double_colon_pos];
            let name = &edge.dst[double_colon_pos + 2..];

            // Try to resolve using the symbol table
            // First try exact qualifier match
            if let Some(resolved_id) = symbol_table.get(&(qualifier.to_string(), name.to_string()))
            {
                store.update_edge_destination(&edge.src, &edge.dst, resolved_id)?;
                resolved_count += 1;
                continue;
            }

            // Try with .ts extension (common for TypeScript imports)
            let qualifier_with_ts = format!("{}.ts", qualifier);
            if let Some(resolved_id) =
                symbol_table.get(&(qualifier_with_ts.clone(), name.to_string()))
            {
                store.update_edge_destination(&edge.src, &edge.dst, resolved_id)?;
                resolved_count += 1;
                continue;
            }

            // Try with .tsx extension
            let qualifier_with_tsx = format!("{}.tsx", qualifier);
            if let Some(resolved_id) = symbol_table.get(&(qualifier_with_tsx, name.to_string())) {
                store.update_edge_destination(&edge.src, &edge.dst, resolved_id)?;
                resolved_count += 1;
                continue;
            }

            // Try normalizing the qualifier path (remove ./ and ../)
            let normalized_qualifier = normalize_import_path(qualifier);
            if normalized_qualifier != qualifier {
                if let Some(resolved_id) =
                    symbol_table.get(&(normalized_qualifier.clone(), name.to_string()))
                {
                    store.update_edge_destination(&edge.src, &edge.dst, resolved_id)?;
                    resolved_count += 1;
                    continue;
                }

                // Try normalized with extensions
                let norm_with_ts = format!("{}.ts", normalized_qualifier);
                if let Some(resolved_id) = symbol_table.get(&(norm_with_ts, name.to_string())) {
                    store.update_edge_destination(&edge.src, &edge.dst, resolved_id)?;
                    resolved_count += 1;
                    continue;
                }

                let norm_with_tsx = format!("{}.tsx", normalized_qualifier);
                if let Some(resolved_id) = symbol_table.get(&(norm_with_tsx, name.to_string())) {
                    store.update_edge_destination(&edge.src, &edge.dst, resolved_id)?;
                    resolved_count += 1;
                }
            }
        }
    }

    if resolved_count > 0 {
        debug!("Resolved {} edge destinations", resolved_count);
    }

    Ok(())
}

/// Normalize an import path by resolving . and .. components
fn normalize_import_path(path: &str) -> String {
    let parts: Vec<&str> = path.split('/').collect();
    let mut result: Vec<&str> = Vec::new();

    for part in parts {
        match part {
            "." | "" => continue,
            ".." => {
                result.pop();
            }
            _ => result.push(part),
        }
    }

    if path.starts_with('/') {
        format!("/{}", result.join("/"))
    } else {
        result.join("/")
    }
}

/// First pass of indexing: parse file, store symbols/edges/deps, return references for later resolution
/// Returns (normalized_path, references, import_bindings, symbol_count)
fn index_first_pass(
    path: &Path,
    store: &IndexStore,
) -> Result<(String, Vec<ReferenceRecord>, Vec<ImportBindingInfo>, usize)> {
    let contents = fs::read(path)?;
    let source = String::from_utf8_lossy(&contents).to_string();
    let record = to_record(path, &contents)?;
    let (symbols, edges, references, dependencies, import_bindings) = if is_ts_file(path) {
        typescript::index_file(path, &source)?
    } else if is_rust_file(path) {
        rust::index_file(path, &source)?
    } else if is_kotlin_file(path) {
        kotlin::index_file(path, &source)?
    } else if is_cpp_file(path) {
        cpp::index_file(path, &source)?
    } else {
        bail!("unsupported file type: {}", path.display());
    };

    let symbol_count = symbols.len();

    // Store symbols and edges in first pass (but NOT references - those come in phase 2)
    store.save_file_index_without_refs(&record, &symbols, &edges)?;
    store.save_file_dependencies(&record.path, &dependencies)?;

    debug!(
        "First pass indexed {} symbols={} edges={} refs={} deps={} imports={}",
        record.path,
        symbol_count,
        edges.len(),
        references.len(),
        dependencies.len(),
        import_bindings.len()
    );

    Ok((record.path, references, import_bindings, symbol_count))
}

/// Index a single file, updating or inserting its record.
/// Note: For incremental updates, we still do single-pass indexing since we can't
/// easily rebuild the global symbol table for just one file. Cross-file reference
/// resolution may be incomplete until the next full index.
pub fn index_one(path: &Path, store: &IndexStore) -> Result<String> {
    let contents = fs::read(path)?;
    let source = String::from_utf8_lossy(&contents).to_string();
    let record = to_record(path, &contents)?;
    let (symbols, edges, references, dependencies, _import_bindings) = if is_ts_file(path) {
        typescript::index_file(path, &source)?
    } else if is_rust_file(path) {
        rust::index_file(path, &source)?
    } else if is_kotlin_file(path) {
        kotlin::index_file(path, &source)?
    } else if is_cpp_file(path) {
        cpp::index_file(path, &source)?
    } else {
        bail!("unsupported file type: {}", path.display());
    };
    store.save_file_index(&record, &symbols, &edges, &references)?;
    store.save_file_dependencies(&record.path, &dependencies)?;
    debug!(
        "Indexed {} symbols={} edges={} refs={} deps={}",
        record.path,
        symbols.len(),
        edges.len(),
        references.len(),
        dependencies.len()
    );
    Ok(record.path)
}

pub fn remove_if_tracked(path: &Path, store: &IndexStore) -> Result<()> {
    store.remove_file(path)?;
    debug!("Removed {} from index", path.display());
    Ok(())
}

fn prune_deleted(store: &IndexStore, seen: &HashSet<String>) -> Result<()> {
    let known = store.list_paths()?;
    for path in known.difference(seen) {
        store.remove_file(path)?;
        debug!("Pruned deleted file {path}");
    }
    Ok(())
}

fn should_descend(entry: &DirEntry, root: &Path) -> bool {
    let path = entry.path();
    if path == root {
        return true;
    }
    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        if entry.file_type().is_dir() && SKIP_DIRS.contains(&name) {
            return false;
        }
    }
    true
}

pub fn is_ts_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("ts" | "tsx")
    )
}

pub fn is_rust_file(path: &Path) -> bool {
    matches!(path.extension().and_then(|e| e.to_str()), Some("rs"))
}

pub fn is_kotlin_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("kt" | "kts")
    )
}

pub fn is_cpp_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("cpp" | "cc" | "cxx" | "c++" | "hpp" | "hh" | "hxx" | "h++")
    )
}

pub fn is_indexed_file(path: &Path) -> bool {
    is_ts_file(path) || is_rust_file(path) || is_kotlin_file(path) || is_cpp_file(path)
}

fn to_record(path: &Path, contents: &[u8]) -> Result<FileRecord> {
    let metadata = fs::metadata(path)?;
    let mtime = metadata
        .modified()
        .unwrap_or(SystemTime::UNIX_EPOCH)
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let mut hasher = Hasher::new();
    hasher.update(contents);
    let hash = hasher.finalize().to_hex().to_string();
    Ok(FileRecord {
        path: normalize_path(path),
        hash,
        mtime,
        indexed_at: now_unix(),
    })
}
