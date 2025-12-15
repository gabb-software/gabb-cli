use crate::languages::{rust, typescript};
use crate::store::{FileRecord, IndexStore, normalize_path, now_unix};
use anyhow::{Context, Result, bail};
use blake3::Hasher;
use log::{debug, info, warn};
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
use walkdir::{DirEntry, WalkDir};

const SKIP_DIRS: &[&str] = &[".git", ".gabb", "target", "node_modules"];

/// Rebuild the index from scratch for a workspace root.
pub fn build_full_index(root: &Path, store: &IndexStore) -> Result<()> {
    let root = root
        .canonicalize()
        .with_context(|| format!("failed to canonicalize root {}", root.display()))?;
    info!("Starting full index at {}", root.display());
    let mut seen = HashSet::new();

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
        match index_one(entry.path(), store) {
            Ok(path) => {
                seen.insert(path);
            }
            Err(err) => warn!("indexing failed for {}: {err}", entry.path().display()),
        }
    }

    prune_deleted(store, &seen)?;

    // Update query optimizer statistics for optimal index usage
    store.analyze()?;

    info!("Full index complete. DB at {}", store.db_path().display());
    Ok(())
}

/// Index a single file, updating or inserting its record.
pub fn index_one(path: &Path, store: &IndexStore) -> Result<String> {
    let contents = fs::read(path)?;
    let source = String::from_utf8_lossy(&contents).to_string();
    let record = to_record(path, &contents)?;
    let (symbols, edges, references, dependencies) = if is_ts_file(path) {
        typescript::index_file(path, &source)?
    } else if is_rust_file(path) {
        rust::index_file(path, &source)?
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

pub fn is_indexed_file(path: &Path) -> bool {
    is_ts_file(path) || is_rust_file(path)
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
