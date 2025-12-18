use crate::languages::{cpp, kotlin, rust, typescript, ImportBindingInfo};
use crate::store::{normalize_path, now_unix, FileRecord, IndexStore, ReferenceRecord};
use anyhow::{bail, Context, Result};
use blake3::Hasher;
use log::{debug, info, warn};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
use walkdir::{DirEntry, WalkDir};

const SKIP_DIRS: &[&str] = &[".git", ".gabb", "target", "node_modules"];

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
pub fn build_full_index(root: &Path, store: &IndexStore) -> Result<()> {
    let root = root
        .canonicalize()
        .with_context(|| format!("failed to canonicalize root {}", root.display()))?;
    info!("Starting full index at {}", root.display());
    let mut seen = HashSet::new();
    let mut first_pass_data: Vec<FirstPassData> = Vec::new();

    // Phase 1: Parse all files, store symbols/edges/deps, collect references for later resolution
    for entry in WalkDir::new(&root)
        .into_iter()
        .filter_entry(|e| should_descend(e, &root))
    {
        let entry = match entry {
            Ok(e) => e,
            Err(err) => {
                warn!("walk error: {}", err);
                continue;
            }
        };
        if !entry.file_type().is_file() || !is_indexed_file(entry.path()) {
            continue;
        }
        match index_first_pass(entry.path(), store) {
            Ok((path, refs, imports)) => {
                seen.insert(path.clone());
                first_pass_data.push(FirstPassData {
                    file_path: path,
                    references: refs,
                    import_bindings: imports,
                });
            }
            Err(err) => warn!("indexing failed for {}: {err}", entry.path().display()),
        }
    }

    prune_deleted(store, &seen)?;

    // Phase 2: Build global symbol table and resolve references
    let symbol_table = build_global_symbol_table(store)?;
    resolve_and_store_references(store, &first_pass_data, &symbol_table)?;

    // Update query optimizer statistics for optimal index usage
    store.analyze()?;

    info!("Full index complete. DB at {}", store.db_path().display());
    Ok(())
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

/// First pass of indexing: parse file, store symbols/edges/deps, return references for later resolution
fn index_first_pass(
    path: &Path,
    store: &IndexStore,
) -> Result<(String, Vec<ReferenceRecord>, Vec<ImportBindingInfo>)> {
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

    // Store symbols and edges in first pass (but NOT references - those come in phase 2)
    store.save_file_index_without_refs(&record, &symbols, &edges)?;
    store.save_file_dependencies(&record.path, &dependencies)?;

    debug!(
        "First pass indexed {} symbols={} edges={} refs={} deps={} imports={}",
        record.path,
        symbols.len(),
        edges.len(),
        references.len(),
        dependencies.len(),
        import_bindings.len()
    );

    Ok((record.path, references, import_bindings))
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
