//! Dependency lookup commands: includers, includes.

use anyhow::Result;
use std::path::Path;

use gabb_cli::store::normalize_path;
use gabb_cli::ExitCode;
use gabb_cli::OutputFormat;

use crate::output::output_file_list;
use crate::util::open_store_for_query;

/// Find all files that include/import the given file (reverse dependency lookup).
pub fn find_includers(
    db: &Path,
    file: &Path,
    transitive: bool,
    limit: Option<usize>,
    format: OutputFormat,
    quiet: bool,
) -> Result<ExitCode> {
    let store = open_store_for_query(db)?;
    let file_str = normalize_path(file);

    let mut files: Vec<String> = if transitive {
        // Use get_invalidation_set which returns transitive reverse dependencies
        store.get_invalidation_set(&file_str)?
    } else {
        // Just direct dependents
        store.get_dependents(&file_str)?
    };

    // Remove the original file from transitive results
    files.retain(|f| f != &file_str);

    if let Some(lim) = limit {
        files.truncate(lim);
    }

    let found = !files.is_empty();
    output_file_list(&files, &file_str, "includers", transitive, format, quiet)?;
    Ok(if found {
        ExitCode::Success
    } else {
        ExitCode::NotFound
    })
}

/// Find all files that the given file includes/imports (forward dependency lookup).
pub fn find_includes(
    db: &Path,
    file: &Path,
    transitive: bool,
    limit: Option<usize>,
    format: OutputFormat,
    quiet: bool,
) -> Result<ExitCode> {
    let store = open_store_for_query(db)?;
    let file_str = normalize_path(file);

    let mut files: Vec<String> = if transitive {
        store.get_transitive_dependencies(&file_str)?
    } else {
        store
            .get_file_dependencies(&file_str)?
            .into_iter()
            .map(|d| d.to_file)
            .collect()
    };

    if let Some(lim) = limit {
        files.truncate(lim);
    }

    let found = !files.is_empty();
    output_file_list(&files, &file_str, "includes", transitive, format, quiet)?;
    Ok(if found {
        ExitCode::Success
    } else {
        ExitCode::NotFound
    })
}
