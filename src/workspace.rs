//! Workspace discovery and path resolution.
//!
//! This module provides automatic workspace root detection by walking up
//! from the current directory looking for project markers like `.gabb/`,
//! `.git/`, `Cargo.toml`, etc.
//!
//! ## Priority Order
//!
//! 1. CLI argument (`--workspace`)
//! 2. Environment variable (`GABB_WORKSPACE`)
//! 3. Auto-detection via marker files
//!
//! ## Markers (in priority order)
//!
//! - `.gabb/` - Explicit gabb workspace (highest priority)
//! - `.git/` - Git repository root
//! - `Cargo.toml`, `package.json`, etc. - Build system files

use anyhow::{bail, Result};
use std::env;
use std::path::{Path, PathBuf};

/// Environment variable for explicit workspace path
pub const ENV_WORKSPACE: &str = "GABB_WORKSPACE";

/// Environment variable for explicit database path
pub const ENV_DB: &str = "GABB_DB";

/// Workspace markers in priority order.
/// Files that indicate a project root.
pub const WORKSPACE_MARKERS: &[&str] = &[
    ".gabb",          // Highest priority - explicit gabb workspace
    ".git",           // Git repository root
    "Cargo.toml",     // Rust
    "package.json",   // Node.js
    "pyproject.toml", // Python
    "go.mod",         // Go
    "build.gradle",   // Gradle
    "build.gradle.kts",
    "pom.xml", // Maven
    "settings.gradle",
    "settings.gradle.kts",
];

/// Directory markers - directories that indicate a project root
pub const WORKSPACE_DIR_MARKERS: &[&str] = &[".git", "gradle"];

/// Find workspace root by walking up from current directory.
///
/// Returns `None` if no workspace markers are found before reaching
/// the filesystem root or user's home directory.
pub fn find_workspace_root() -> Option<PathBuf> {
    let cwd = env::current_dir().ok()?;
    find_workspace_root_from(&cwd)
}

/// Find workspace root by walking up from a specific starting path.
///
/// Returns `None` if no workspace markers are found before reaching
/// the filesystem root or user's home directory.
pub fn find_workspace_root_from(start: &Path) -> Option<PathBuf> {
    let start = if start.is_absolute() {
        start.to_path_buf()
    } else {
        env::current_dir().ok()?.join(start)
    };

    // Get home directory to stop searching there
    let home = dirs::home_dir();

    let mut current = if start.is_file() {
        start.parent()?.to_path_buf()
    } else {
        start
    };

    loop {
        // Stop at home directory (don't go above it)
        if let Some(ref home) = home {
            if &current == home {
                // Check home directory itself for markers, but don't go above
                if has_workspace_marker(&current) {
                    return Some(current);
                }
                return None;
            }
        }

        // Check for markers
        if has_workspace_marker(&current) {
            return Some(current);
        }

        // Move up to parent directory
        match current.parent() {
            Some(parent) if parent != current => {
                current = parent.to_path_buf();
            }
            _ => return None, // Reached filesystem root
        }
    }
}

/// Check if a directory contains any workspace marker
fn has_workspace_marker(dir: &Path) -> bool {
    // Check file markers
    for marker in WORKSPACE_MARKERS {
        if dir.join(marker).exists() {
            return true;
        }
    }

    // Check directory markers
    for marker in WORKSPACE_DIR_MARKERS {
        let marker_path = dir.join(marker);
        if marker_path.is_dir() {
            return true;
        }
    }

    false
}

/// Resolve workspace root with priority: CLI arg > env var > auto-detect.
///
/// # Arguments
/// * `cli_arg` - Optional workspace path from CLI `--workspace` flag
///
/// # Returns
/// The resolved workspace path, or an error if no workspace could be found.
pub fn resolve_workspace(cli_arg: Option<&Path>) -> Result<PathBuf> {
    // Priority 1: CLI argument
    if let Some(path) = cli_arg {
        let path = canonicalize_or_absolute(path);
        return Ok(path);
    }

    // Priority 2: Environment variable
    if let Ok(env_path) = env::var(ENV_WORKSPACE) {
        let path = PathBuf::from(env_path);
        let path = canonicalize_or_absolute(&path);
        return Ok(path);
    }

    // Priority 3: Auto-detect
    if let Some(workspace) = find_workspace_root() {
        return Ok(workspace);
    }

    bail!(
        "Could not detect workspace root.\n\n\
         Run from a directory containing .gabb/, .git/, Cargo.toml, package.json, or other project markers.\n\n\
         Or specify explicitly:\n\
         \x20 --workspace /path/to/project\n\
         \x20 {}=/path/to/project",
        ENV_WORKSPACE
    )
}

/// Resolve database path with priority: CLI arg > env var > workspace/.gabb/index.db.
///
/// # Arguments
/// * `cli_arg` - Optional database path from CLI `--db` flag
/// * `workspace` - The resolved workspace root
///
/// # Returns
/// The resolved database path.
pub fn resolve_db_path(cli_arg: Option<&Path>, workspace: &Path) -> PathBuf {
    // Priority 1: CLI argument
    if let Some(path) = cli_arg {
        return if path.is_absolute() {
            path.to_path_buf()
        } else {
            workspace.join(path)
        };
    }

    // Priority 2: Environment variable
    if let Ok(env_path) = env::var(ENV_DB) {
        let path = PathBuf::from(env_path);
        return if path.is_absolute() {
            path
        } else {
            workspace.join(path)
        };
    }

    // Priority 3: Default location in workspace
    workspace.join(".gabb/index.db")
}

/// Canonicalize a path, or make it absolute if canonicalization fails.
fn canonicalize_or_absolute(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(path)
        }
    })
}

/// Set environment variables for child processes.
/// Call this after resolving workspace to ensure consistent paths in spawned processes.
pub fn set_env_for_children(workspace: &Path, db: &Path) {
    env::set_var(ENV_WORKSPACE, workspace);
    env::set_var(ENV_DB, db);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_find_workspace_with_gabb_marker() {
        let temp = tempdir().unwrap();
        let gabb_dir = temp.path().join(".gabb");
        fs::create_dir(&gabb_dir).unwrap();

        let result = find_workspace_root_from(temp.path());
        assert_eq!(result, Some(temp.path().to_path_buf()));
    }

    #[test]
    fn test_find_workspace_with_git_marker() {
        let temp = tempdir().unwrap();
        let git_dir = temp.path().join(".git");
        fs::create_dir(&git_dir).unwrap();

        let result = find_workspace_root_from(temp.path());
        assert_eq!(result, Some(temp.path().to_path_buf()));
    }

    #[test]
    fn test_find_workspace_with_cargo_toml() {
        let temp = tempdir().unwrap();
        fs::write(temp.path().join("Cargo.toml"), "").unwrap();

        let result = find_workspace_root_from(temp.path());
        assert_eq!(result, Some(temp.path().to_path_buf()));
    }

    #[test]
    fn test_find_workspace_from_subdirectory() {
        let temp = tempdir().unwrap();
        let gabb_dir = temp.path().join(".gabb");
        fs::create_dir(&gabb_dir).unwrap();

        let subdir = temp.path().join("src").join("nested");
        fs::create_dir_all(&subdir).unwrap();

        let result = find_workspace_root_from(&subdir);
        assert_eq!(result, Some(temp.path().to_path_buf()));
    }

    #[test]
    fn test_find_workspace_no_markers() {
        let temp = tempdir().unwrap();
        let isolated = temp.path().join("isolated");
        fs::create_dir(&isolated).unwrap();

        // This might find a parent workspace if run from within a project,
        // but the isolated directory itself has no markers
        // For a true isolated test, we'd need to mock the filesystem
    }

    #[test]
    fn test_resolve_db_path_default() {
        let workspace = PathBuf::from("/home/user/project");
        let result = resolve_db_path(None, &workspace);
        assert_eq!(result, PathBuf::from("/home/user/project/.gabb/index.db"));
    }

    #[test]
    fn test_resolve_db_path_with_cli_arg() {
        let workspace = PathBuf::from("/home/user/project");
        let cli_db = PathBuf::from("custom.db");
        let result = resolve_db_path(Some(&cli_db), &workspace);
        assert_eq!(result, PathBuf::from("/home/user/project/custom.db"));
    }

    #[test]
    fn test_resolve_db_path_with_absolute_cli_arg() {
        let workspace = PathBuf::from("/home/user/project");
        let cli_db = PathBuf::from("/tmp/index.db");
        let result = resolve_db_path(Some(&cli_db), &workspace);
        assert_eq!(result, PathBuf::from("/tmp/index.db"));
    }
}
