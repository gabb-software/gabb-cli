//! Duplicate detection command.

use anyhow::Result;
use std::path::Path;

use gabb_cli::daemon;
use gabb_cli::ExitCode;
use gabb_cli::OutputFormat;

use crate::output::{
    DuplicateGroupOutput, DuplicatesOutput, DuplicatesSummary, SymbolOutput,
};
use crate::util::{get_git_changed_files, open_store_for_query};

pub fn find_duplicates(
    db: &Path,
    uncommitted: bool,
    staged: bool,
    kind: Option<&str>,
    min_count: usize,
    format: OutputFormat,
    quiet: bool,
) -> Result<ExitCode> {
    let store = open_store_for_query(db)?;
    let workspace_root = daemon::workspace_root_from_db(db)?;

    // Get file filter based on git flags
    let file_filter: Option<Vec<String>> = if uncommitted || staged {
        let files = get_git_changed_files(&workspace_root, uncommitted, staged)?;
        if files.is_empty() {
            output_empty_duplicates(format, quiet)?;
            return Ok(ExitCode::NotFound);
        }
        Some(files)
    } else {
        None
    };

    let groups = store.find_duplicate_groups(min_count, kind, file_filter.as_deref())?;

    // Build structured output
    let group_outputs: Vec<DuplicateGroupOutput> = groups
        .iter()
        .map(|group| {
            let symbols: Vec<SymbolOutput> = group
                .symbols
                .iter()
                .filter_map(SymbolOutput::from_record)
                .collect();
            DuplicateGroupOutput {
                content_hash: group.content_hash.clone(),
                count: symbols.len(),
                symbols,
            }
        })
        .collect();

    let total_duplicates: usize = group_outputs.iter().map(|g| g.count).sum();
    let summary = DuplicatesSummary {
        total_groups: group_outputs.len(),
        total_duplicates,
    };

    let found = !group_outputs.is_empty();

    match format {
        OutputFormat::Json => {
            let output = DuplicatesOutput {
                groups: group_outputs,
                summary,
            };
            println!("{}", serde_json::to_string_pretty(&output)?);
        }
        OutputFormat::Jsonl => {
            for group in &group_outputs {
                println!("{}", serde_json::to_string(group)?);
            }
        }
        OutputFormat::Csv => {
            let mut wtr = csv::Writer::from_writer(std::io::stdout());
            wtr.write_record(["group_hash", "name", "kind", "location", "container"])?;
            for group in &group_outputs {
                let short_hash = &group.content_hash[..8.min(group.content_hash.len())];
                for sym in &group.symbols {
                    wtr.write_record([
                        short_hash,
                        &sym.name,
                        &sym.kind,
                        &sym.location(),
                        sym.container.as_deref().unwrap_or(""),
                    ])?;
                }
            }
            wtr.flush()?;
        }
        OutputFormat::Tsv => {
            println!("group_hash\tname\tkind\tlocation\tcontainer");
            for group in &group_outputs {
                let short_hash = &group.content_hash[..8.min(group.content_hash.len())];
                for sym in &group.symbols {
                    println!(
                        "{}\t{}\t{}\t{}\t{}",
                        short_hash,
                        sym.name,
                        sym.kind,
                        sym.location(),
                        sym.container.as_deref().unwrap_or("")
                    );
                }
            }
        }
        OutputFormat::Text => {
            if group_outputs.is_empty() {
                if !quiet {
                    println!("No duplicates found.");
                }
                return Ok(ExitCode::NotFound);
            }

            println!(
                "Found {} duplicate groups ({} total symbols)\n",
                summary.total_groups, summary.total_duplicates
            );

            for (i, group) in group_outputs.iter().enumerate() {
                let short_hash = &group.content_hash[..8.min(group.content_hash.len())];
                println!(
                    "Group {} ({} duplicates, hash: {}):",
                    i + 1,
                    group.count,
                    short_hash
                );
                for sym in &group.symbols {
                    let container = sym
                        .container
                        .as_deref()
                        .map(|c| format!(" in {c}"))
                        .unwrap_or_default();
                    println!(
                        "  {:<10} {:<30} {}{}",
                        sym.kind,
                        sym.name,
                        sym.location(),
                        container
                    );
                }
                println!();
            }
        }
    }

    Ok(if found {
        ExitCode::Success
    } else {
        ExitCode::NotFound
    })
}

fn output_empty_duplicates(format: OutputFormat, quiet: bool) -> Result<()> {
    match format {
        OutputFormat::Json => {
            let output = DuplicatesOutput {
                groups: vec![],
                summary: DuplicatesSummary {
                    total_groups: 0,
                    total_duplicates: 0,
                },
            };
            println!("{}", serde_json::to_string_pretty(&output)?);
        }
        OutputFormat::Jsonl => {} // No output for empty results
        OutputFormat::Csv => {
            let mut wtr = csv::Writer::from_writer(std::io::stdout());
            wtr.write_record(["group_hash", "name", "kind", "location", "container"])?;
            wtr.flush()?;
        }
        OutputFormat::Tsv => {
            println!("group_hash\tname\tkind\tlocation\tcontainer");
        }
        OutputFormat::Text => {
            if !quiet {
                println!("No changed files found.");
            }
        }
    }
    Ok(())
}
