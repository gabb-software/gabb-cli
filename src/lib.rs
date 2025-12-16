pub mod daemon;
pub mod indexer;
pub mod languages;
pub mod store;

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
