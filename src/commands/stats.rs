//! Stats command: show index statistics.

use anyhow::Result;
use std::path::Path;

use gabb_cli::OutputFormat;

use crate::util::open_store_for_query;

pub fn show_stats(db: &Path, format: OutputFormat) -> Result<()> {
    let store = open_store_for_query(db)?;
    let stats = store.get_index_stats()?;

    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&stats)?);
        }
        OutputFormat::Jsonl => {
            println!("{}", serde_json::to_string(&stats)?);
        }
        OutputFormat::Csv | OutputFormat::Tsv => {
            // CSV/TSV format doesn't make sense for nested stats, output as JSON
            println!("{}", serde_json::to_string_pretty(&stats)?);
        }
        OutputFormat::Text => {
            println!("Index Statistics");
            println!("================");
            println!();

            println!("Files:");
            println!("  Total: {}", stats.files.total);
            if !stats.files.by_language.is_empty() {
                println!("  By language:");
                let mut langs: Vec<_> = stats.files.by_language.iter().collect();
                langs.sort_by(|a, b| b.1.cmp(a.1)); // Sort by count descending
                for (lang, count) in langs {
                    println!("    {}: {}", lang, count);
                }
            }
            println!();

            println!("Symbols:");
            println!("  Total: {}", stats.symbols.total);
            if !stats.symbols.by_kind.is_empty() {
                println!("  By kind:");
                let mut kinds: Vec<_> = stats.symbols.by_kind.iter().collect();
                kinds.sort_by(|a, b| b.1.cmp(a.1)); // Sort by count descending
                for (kind, count) in kinds {
                    println!("    {}: {}", kind, count);
                }
            }
            println!();

            println!("Index:");
            println!("  Size: {} bytes", stats.index.size_bytes);
            if let Some(updated) = &stats.index.last_updated {
                println!("  Last updated: {}", updated);
            }
            println!("  Schema version: {}", stats.index.schema_version);
            println!();

            if stats.errors.parse_failures > 0 {
                println!("Errors:");
                println!("  Parse failures: {}", stats.errors.parse_failures);
                if !stats.errors.failed_files.is_empty() {
                    println!("  Failed files:");
                    for file in &stats.errors.failed_files {
                        println!("    {}", file);
                    }
                }
            }
        }
    }

    Ok(())
}
