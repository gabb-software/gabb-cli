pub mod daemon;
pub mod indexer;
pub mod languages;
pub mod mcp;
pub mod store;
pub mod workspace;

use clap::ValueEnum;

/// Output format for command results
#[derive(Debug, Clone, Copy, Default, ValueEnum)]
pub enum OutputFormat {
    /// Human-readable text output (default)
    #[default]
    Text,
    /// JSON array output
    Json,
    /// JSON Lines (one JSON object per line)
    Jsonl,
    /// Comma-separated values
    Csv,
    /// Tab-separated values
    Tsv,
}

/// Semantic exit codes for script-friendly operation.
///
/// These exit codes allow scripts to distinguish between different
/// outcomes without parsing output:
/// - 0: Success - operation completed and found results
/// - 1: Not found - operation completed but no results matched
/// - 2: Error - operation failed due to an error
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitCode {
    /// Operation succeeded with results (exit code 0)
    Success = 0,
    /// Operation succeeded but nothing was found (exit code 1)
    NotFound = 1,
    /// Operation failed with an error (exit code 2)
    Error = 2,
}

impl ExitCode {
    /// Convert to process exit code
    pub fn code(self) -> i32 {
        self as i32
    }
}

impl From<ExitCode> for std::process::ExitCode {
    fn from(code: ExitCode) -> Self {
        std::process::ExitCode::from(code.code() as u8)
    }
}

/// Convert a byte offset to 1-based (line, column) position in a buffer.
///
/// Returns `None` if the offset is beyond the buffer length.
/// Useful for converting byte positions from parsers to editor-friendly coordinates.
pub fn offset_to_line_col(buf: &[u8], offset: usize) -> Option<(usize, usize)> {
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
    // Handle offset at exact end of buffer
    if offset == buf.len() {
        Some((line, col))
    } else {
        None
    }
}

/// Convert a byte offset to 1-based (line, column) position in a file.
///
/// Reads the file and delegates to [`offset_to_line_col`].
/// Returns an error if the file cannot be read or the offset is out of bounds.
pub fn offset_to_line_col_in_file(
    path: &std::path::Path,
    offset: usize,
) -> std::io::Result<(usize, usize)> {
    let buf = std::fs::read(path)?;
    offset_to_line_col(&buf, offset).ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "offset out of bounds")
    })
}

/// Check if a file path appears to be a test file based on common naming conventions.
///
/// Returns true if the path matches any of these patterns:
/// - Directory: `/tests/`, `/__tests__/`, `/test/`, `/spec/`
/// - Rust: `*_test.rs`, `*_spec.rs`
/// - TypeScript/JavaScript: `*.test.ts`, `*.spec.ts`, `*.test.tsx`, `*.spec.tsx`,
///   `*.test.js`, `*.spec.js`, `*.test.jsx`, `*.spec.jsx`
pub fn is_test_file(path: &str) -> bool {
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
