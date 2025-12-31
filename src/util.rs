//! Shared utility functions for the CLI.
//!
//! This module contains helper functions used across multiple commands.

use anyhow::{anyhow, bail, Context, Result};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use gabb_cli::daemon;
use gabb_cli::store::{normalize_path, DbOpenResult, IndexStore, ReferenceRecord, SymbolRecord};

// ==================== Store Helpers ====================

/// Open index store for query commands with version checking.
/// Returns a helpful error if the database needs regeneration.
pub fn open_store_for_query(db: &Path) -> Result<IndexStore> {
    match IndexStore::try_open(db)? {
        DbOpenResult::Ready(store) => Ok(store),
        DbOpenResult::NeedsRegeneration { reason, .. } => {
            bail!(
                "{}\n\nRun `gabb daemon start --db {} --rebuild` to regenerate the index.",
                reason.message(),
                db.display()
            )
        }
    }
}

/// Ensure the index is available using the shared daemon logic.
pub fn ensure_index_available(
    db: &Path,
    no_start_daemon: bool,
    no_daemon: bool,
) -> Result<()> {
    // Derive workspace root from db path
    let workspace_root = daemon::workspace_root_from_db(db)
        .unwrap_or_else(|_| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    let daemon_opts = daemon::EnsureIndexOptions {
        no_start_daemon,
        timeout: std::time::Duration::from_secs(60),
        no_daemon_warnings: no_daemon,
        auto_restart_on_version_mismatch: false, // CLI shows warning, user decides
    };

    daemon::ensure_index_available(&workspace_root, db, &daemon_opts)
}

// ==================== Symbol Resolution ====================

/// Resolve a symbol at a given file position.
pub fn resolve_symbol_at(
    store: &IndexStore,
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

/// Find the narrowest symbol that covers a given offset.
pub fn narrowest_symbol_covering(
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

/// Find the identifier at a given byte offset in a buffer.
pub fn find_identifier_at_offset(buf: &[u8], offset: usize) -> Option<String> {
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

/// Check if an identifier matches a symbol name or is a keyword.
pub fn identifier_matches_symbol(ident: &str, sym: &SymbolRecord) -> bool {
    if ident == sym.name {
        return true;
    }
    matches!(
        ident,
        "fn" | "function" | "class" | "interface" | "enum" | "struct" | "impl"
    )
}

/// Remove duplicate symbols by ID.
pub fn dedup_symbols(symbols: &mut Vec<SymbolRecord>) {
    let mut seen = HashSet::new();
    symbols.retain(|s| seen.insert(s.id.clone()));
}

// ==================== Usage Search ====================

/// Search for usages of a symbol by name (fallback when references aren't indexed).
pub fn search_usages_by_name(
    store: &IndexStore,
    target: &SymbolRecord,
    _workspace_root: &Path,
) -> Result<Vec<ReferenceRecord>> {
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
            refs.push(ReferenceRecord {
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

/// Check if a match is at a word boundary.
pub fn is_word_boundary(buf: &[u8], start: usize, len: usize) -> bool {
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

// ==================== Position Conversion ====================

/// Convert 1-based line/character to byte offset.
pub fn line_char_to_offset(buf: &[u8], line: usize, character: usize) -> Option<usize> {
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

/// Convert byte offset to 1-based line/character in a file.
pub fn offset_to_line_char_in_file(path: &str, offset: i64) -> Result<(usize, usize)> {
    gabb_cli::offset_to_line_col_in_file(Path::new(path), offset as usize)
        .with_context(|| format!("failed to convert offset for {path}"))
}

// ==================== File Position Parsing ====================

/// Parse file path with optional embedded :line:character position.
pub fn parse_file_position(
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

/// Split a file path into base path and optional embedded position.
pub fn split_file_and_embedded_position(file: &Path) -> (PathBuf, Option<(usize, usize)>) {
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

// ==================== Git Utilities ====================

/// Get list of changed files from git.
/// If `uncommitted` is true, includes working tree changes (unstaged + staged).
/// If `staged` is true, includes only staged changes.
pub fn get_git_changed_files(
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
