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
