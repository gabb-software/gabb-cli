//! Output formatting for command results.
//!
//! This module handles serialization and formatting of query results
//! across all supported output formats (JSON, CSV, TSV, text).

use anyhow::{anyhow, Result};

use gabb_cli::is_test_file;
use gabb_cli::mcp;
use gabb_cli::store::{self, ReferenceRecord, SymbolRecord};
use gabb_cli::OutputFormat;

use crate::util::offset_to_line_char_in_file;

// ==================== Output Structs ====================

/// Options for displaying source code in output
#[derive(Clone, Copy, Default)]
pub struct SourceDisplayOptions {
    pub include_source: bool,
    pub context_lines: Option<usize>,
}

/// A symbol with resolved line/column positions for output
#[derive(serde::Serialize)]
pub struct SymbolOutput {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub context: String, // "test" or "prod"
    pub file: String,
    pub start: Position,
    pub end: Position,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visibility: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub container: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub qualifier: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

#[derive(serde::Serialize, Clone)]
pub struct Position {
    pub line: usize,
    pub character: usize,
}

/// Output for file structure command showing hierarchical symbols
#[derive(serde::Serialize)]
pub struct FileStructure {
    pub file: String,
    pub context: String, // "test" or "prod"
    pub symbols: Vec<SymbolNode>,
}

/// A symbol node in the file structure hierarchy
#[derive(serde::Serialize, Clone)]
pub struct SymbolNode {
    pub name: String,
    pub kind: String,
    pub context: String, // "test" or "prod"
    pub start: Position,
    pub end: Position,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visibility: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<SymbolNode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

impl SymbolOutput {
    pub fn from_record(sym: &SymbolRecord) -> Option<Self> {
        Self::from_record_with_source(sym, SourceDisplayOptions::default())
    }

    pub fn from_record_with_source(sym: &SymbolRecord, opts: SourceDisplayOptions) -> Option<Self> {
        let (line, col) = offset_to_line_char_in_file(&sym.file, sym.start).ok()?;
        let (end_line, end_col) = offset_to_line_char_in_file(&sym.file, sym.end).ok()?;

        let source = if opts.include_source {
            mcp::extract_source(&sym.file, sym.start, sym.end, opts.context_lines)
        } else {
            None
        };

        // Determine context from file path OR inline test markers (#[cfg(test)], #[test])
        let context = if sym.is_test || is_test_file(&sym.file) {
            "test"
        } else {
            "prod"
        };

        Some(Self {
            id: sym.id.clone(),
            name: sym.name.clone(),
            kind: sym.kind.clone(),
            context: context.to_string(),
            file: sym.file.clone(),
            start: Position {
                line,
                character: col,
            },
            end: Position {
                line: end_line,
                character: end_col,
            },
            visibility: sym.visibility.clone(),
            container: sym.container.clone(),
            qualifier: sym.qualifier.clone(),
            source,
        })
    }

    /// Compact file:line:col format for text output
    pub fn location(&self) -> String {
        format!("{}:{}:{}", self.file, self.start.line, self.start.character)
    }

    /// CSV/TSV row
    pub fn to_row(&self) -> Vec<String> {
        vec![
            self.name.clone(),
            self.kind.clone(),
            self.context.clone(),
            self.location(),
            self.visibility.clone().unwrap_or_default(),
            self.container.clone().unwrap_or_default(),
        ]
    }
}

// ==================== Targeted Results (implementations, etc.) ====================

/// Output for implementation/usages results with a target symbol
#[derive(serde::Serialize)]
pub struct TargetedResultOutput {
    pub target: SymbolOutput,
    pub results: Vec<SymbolOutput>,
}

// ==================== Usages Output ====================

/// Output for usages with file grouping
#[derive(serde::Serialize)]
pub struct UsagesOutput {
    pub target: SymbolOutput,
    pub files: Vec<FileUsages>,
    pub summary: UsagesSummary,
}

#[derive(serde::Serialize)]
pub struct FileUsages {
    pub file: String,
    pub context: String,
    pub count: usize,
    pub usages: Vec<UsageLocation>,
}

#[derive(serde::Serialize)]
pub struct UsageLocation {
    pub start: Position,
    pub end: Position,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Import statement that brought the symbol into scope (if from different file)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub import_via: Option<String>,
}

#[derive(serde::Serialize)]
pub struct UsagesSummary {
    pub total: usize,
    pub files: usize,
    pub prod: usize,
    pub test: usize,
}

// ==================== Symbol Detail Output ====================

/// Detailed symbol output with edges and references
#[derive(serde::Serialize)]
pub struct SymbolDetailOutput {
    #[serde(flatten)]
    pub base: SymbolOutput,
    pub outgoing_edges: Vec<EdgeOutput>,
    pub incoming_edges: Vec<EdgeOutput>,
    pub references: Vec<ReferenceOutput>,
}

#[derive(serde::Serialize, Clone)]
pub struct EdgeOutput {
    pub src: String,
    pub dst: String,
    pub kind: String,
    /// Count of duplicate edges (only present when > 1)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<usize>,
}

#[derive(serde::Serialize)]
pub struct ReferenceOutput {
    pub file: String,
    pub start: Position,
    pub end: Position,
}

impl EdgeOutput {
    /// Create an EdgeOutput from src, dst, kind without a count
    pub fn new(src: String, dst: String, kind: String) -> Self {
        Self {
            src,
            dst,
            kind,
            count: None,
        }
    }
}

/// Deduplicate edges by (src, dst, kind) and add count when > 1
///
/// This is useful for displaying edges in a human-readable format where
/// multiple calls to the same function are grouped together with a count.
pub fn deduplicate_edges(edges: Vec<EdgeOutput>) -> Vec<EdgeOutput> {
    use std::collections::HashMap;

    let mut counts: HashMap<(String, String, String), usize> = HashMap::new();
    for e in edges {
        *counts.entry((e.src, e.dst, e.kind)).or_insert(0) += 1;
    }

    let mut result: Vec<EdgeOutput> = counts
        .into_iter()
        .map(|((src, dst, kind), count)| EdgeOutput {
            src,
            dst,
            kind,
            count: if count > 1 { Some(count) } else { None },
        })
        .collect();

    // Sort for stable output: by kind, then dst name
    result.sort_by(|a, b| (&a.kind, &a.dst).cmp(&(&b.kind, &b.dst)));
    result
}

// ==================== Definition Output ====================

/// Definition output wrapper
#[derive(serde::Serialize)]
pub struct DefinitionOutput {
    pub definition: SymbolOutput,
}

// ==================== Duplicates Output ====================

/// Duplicates output structure
#[derive(serde::Serialize)]
pub struct DuplicatesOutput {
    pub groups: Vec<DuplicateGroupOutput>,
    pub summary: DuplicatesSummary,
}

#[derive(serde::Serialize)]
pub struct DuplicateGroupOutput {
    pub content_hash: String,
    pub count: usize,
    pub symbols: Vec<SymbolOutput>,
}

#[derive(serde::Serialize)]
pub struct DuplicatesSummary {
    pub total_groups: usize,
    pub total_duplicates: usize,
}

// ==================== Output Functions ====================

/// Format and output a list of symbols
pub fn output_symbols(
    symbols: &[SymbolRecord],
    format: OutputFormat,
    source_opts: SourceDisplayOptions,
) -> Result<()> {
    let outputs: Vec<SymbolOutput> = symbols
        .iter()
        .filter_map(|s| SymbolOutput::from_record_with_source(s, source_opts))
        .collect();

    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&outputs)?);
        }
        OutputFormat::Jsonl => {
            for sym in &outputs {
                println!("{}", serde_json::to_string(sym)?);
            }
        }
        OutputFormat::Csv => {
            let mut wtr = csv::Writer::from_writer(std::io::stdout());
            wtr.write_record([
                "name",
                "kind",
                "context",
                "location",
                "visibility",
                "container",
            ])?;
            for sym in &outputs {
                wtr.write_record(sym.to_row())?;
            }
            wtr.flush()?;
        }
        OutputFormat::Tsv => {
            println!("name\tkind\tcontext\tlocation\tvisibility\tcontainer");
            for sym in &outputs {
                let row = sym.to_row();
                println!("{}", row.join("\t"));
            }
        }
        OutputFormat::Text => {
            for sym in &outputs {
                let container = sym
                    .container
                    .as_deref()
                    .map(|c| format!(" in {c}"))
                    .unwrap_or_default();
                println!(
                    "{:<10} {:<30} [{}] {}{}",
                    sym.kind,
                    sym.name,
                    sym.context,
                    sym.location(),
                    container
                );
                if let Some(src) = &sym.source {
                    println!("{}\n", src);
                }
            }
        }
    }
    Ok(())
}

/// Format and output implementations for a target symbol
pub fn output_implementations(
    target: &SymbolRecord,
    implementations: &[SymbolRecord],
    format: OutputFormat,
    source_opts: SourceDisplayOptions,
) -> Result<()> {
    let target_out = SymbolOutput::from_record(target)
        .ok_or_else(|| anyhow!("Failed to resolve target position"))?;
    let impl_outputs: Vec<SymbolOutput> = implementations
        .iter()
        .filter_map(|s| SymbolOutput::from_record_with_source(s, source_opts))
        .collect();

    match format {
        OutputFormat::Json => {
            let output = TargetedResultOutput {
                target: target_out,
                results: impl_outputs,
            };
            println!("{}", serde_json::to_string_pretty(&output)?);
        }
        OutputFormat::Jsonl => {
            // First line is target, rest are implementations
            println!("{}", serde_json::to_string(&target_out)?);
            for sym in &impl_outputs {
                println!("{}", serde_json::to_string(sym)?);
            }
        }
        OutputFormat::Csv => {
            let mut wtr = csv::Writer::from_writer(std::io::stdout());
            wtr.write_record(["name", "kind", "location", "visibility", "container"])?;
            for sym in &impl_outputs {
                wtr.write_record(sym.to_row())?;
            }
            wtr.flush()?;
        }
        OutputFormat::Tsv => {
            println!("name\tkind\tlocation\tvisibility\tcontainer");
            for sym in &impl_outputs {
                println!("{}", sym.to_row().join("\t"));
            }
        }
        OutputFormat::Text => {
            println!(
                "Target: {} {} {}",
                target_out.kind,
                target_out.name,
                target_out.location()
            );
            for sym in &impl_outputs {
                let container = sym
                    .container
                    .as_deref()
                    .map(|c| format!(" in {c}"))
                    .unwrap_or_default();
                println!(
                    "{:<10} {:<30} {}{}",
                    sym.kind,
                    sym.name,
                    sym.location(),
                    container
                );
                if let Some(src) = &sym.source {
                    println!("{}\n", src);
                }
            }
        }
    }
    Ok(())
}

/// Format and output usages for a target symbol
pub fn output_usages(
    target: &SymbolRecord,
    refs: &[ReferenceRecord],
    store: &store::IndexStore,
    format: OutputFormat,
    source_opts: SourceDisplayOptions,
) -> Result<()> {
    let target_out = SymbolOutput::from_record(target)
        .ok_or_else(|| anyhow!("Failed to resolve target position"))?;

    // Group references by file
    let mut by_file: std::collections::BTreeMap<String, Vec<&ReferenceRecord>> =
        std::collections::BTreeMap::new();
    for r in refs {
        by_file.entry(r.file.clone()).or_default().push(r);
    }

    // Build structured output
    let mut test_count = 0;
    let mut prod_count = 0;
    let file_usages: Vec<FileUsages> = by_file
        .iter()
        .map(|(file, file_refs)| {
            let is_test = is_test_file(file);

            // Look up import binding if this file is different from the target file
            let import_via = if file != &target.file {
                store
                    .get_import_binding(file, &target.file, &target.name)
                    .ok()
                    .flatten()
                    .map(|b| b.import_text)
            } else {
                None
            };

            let usages: Vec<UsageLocation> = file_refs
                .iter()
                .filter_map(|r| {
                    let (line, col) = offset_to_line_char_in_file(&r.file, r.start).ok()?;
                    let (end_line, end_col) = offset_to_line_char_in_file(&r.file, r.end).ok()?;

                    let source = if source_opts.include_source {
                        mcp::extract_source(&r.file, r.start, r.end, source_opts.context_lines)
                    } else {
                        None
                    };

                    Some(UsageLocation {
                        start: Position {
                            line,
                            character: col,
                        },
                        end: Position {
                            line: end_line,
                            character: end_col,
                        },
                        source,
                        import_via: import_via.clone(),
                    })
                })
                .collect();

            if is_test {
                test_count += usages.len();
            } else {
                prod_count += usages.len();
            }

            FileUsages {
                file: file.clone(),
                context: if is_test { "test" } else { "prod" }.to_string(),
                count: usages.len(),
                usages,
            }
        })
        .collect();

    let total = test_count + prod_count;
    let summary = UsagesSummary {
        total,
        files: by_file.len(),
        prod: prod_count,
        test: test_count,
    };

    match format {
        OutputFormat::Json => {
            let output = UsagesOutput {
                target: target_out,
                files: file_usages,
                summary,
            };
            println!("{}", serde_json::to_string_pretty(&output)?);
        }
        OutputFormat::Jsonl => {
            // First line is target, then each file's usages
            println!("{}", serde_json::to_string(&target_out)?);
            for fu in &file_usages {
                println!("{}", serde_json::to_string(fu)?);
            }
        }
        OutputFormat::Csv => {
            let mut wtr = csv::Writer::from_writer(std::io::stdout());
            wtr.write_record([
                "file",
                "line",
                "character",
                "end_line",
                "end_character",
                "context",
            ])?;
            for fu in &file_usages {
                for u in &fu.usages {
                    wtr.write_record([
                        &fu.file,
                        &u.start.line.to_string(),
                        &u.start.character.to_string(),
                        &u.end.line.to_string(),
                        &u.end.character.to_string(),
                        &fu.context,
                    ])?;
                }
            }
            wtr.flush()?;
        }
        OutputFormat::Tsv => {
            println!("file\tline\tcharacter\tend_line\tend_character\tcontext");
            for fu in &file_usages {
                for u in &fu.usages {
                    println!(
                        "{}\t{}\t{}\t{}\t{}\t{}",
                        fu.file,
                        u.start.line,
                        u.start.character,
                        u.end.line,
                        u.end.character,
                        fu.context
                    );
                }
            }
        }
        OutputFormat::Text => {
            println!(
                "Target: {} {} {}",
                target_out.kind,
                target_out.name,
                target_out.location()
            );
            if refs.is_empty() {
                println!("No usages found.");
            } else {
                for fu in &file_usages {
                    let context = if fu.context == "test" {
                        "[test]"
                    } else {
                        "[prod]"
                    };
                    println!("\n{} {} ({} usages)", context, fu.file, fu.count);
                    // Show import statement if present (only once per file)
                    if let Some(first_usage) = fu.usages.first() {
                        if let Some(import_via) = &first_usage.import_via {
                            println!("  via: {}", import_via.trim());
                        }
                    }
                    for u in &fu.usages {
                        println!(
                            "  {}:{}-{}:{}",
                            u.start.line, u.start.character, u.end.line, u.end.character
                        );
                        if let Some(src) = &u.source {
                            println!("{}", src);
                        }
                    }
                }
                println!(
                    "\nSummary: {} usages in {} files ({} prod, {} test)",
                    summary.total, summary.files, summary.prod, summary.test
                );
            }
        }
    }

    Ok(())
}

/// Output a list of files in various formats.
pub fn output_file_list(
    files: &[String],
    source_file: &str,
    relation: &str,
    transitive: bool,
    format: OutputFormat,
    quiet: bool,
) -> Result<()> {
    #[derive(serde::Serialize)]
    struct FileListOutput {
        source_file: String,
        relation: String,
        transitive: bool,
        count: usize,
        files: Vec<String>,
    }

    let output = FileListOutput {
        source_file: source_file.to_string(),
        relation: relation.to_string(),
        transitive,
        count: files.len(),
        files: files.to_vec(),
    };

    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&output)?);
        }
        OutputFormat::Jsonl => {
            for file in files {
                println!("{}", serde_json::to_string(&file)?);
            }
        }
        OutputFormat::Csv => {
            let mut wtr = csv::Writer::from_writer(std::io::stdout());
            wtr.write_record(["file"])?;
            for file in files {
                wtr.write_record([file])?;
            }
            wtr.flush()?;
        }
        OutputFormat::Tsv => {
            println!("file");
            for file in files {
                println!("{file}");
            }
        }
        OutputFormat::Text => {
            let transitive_str = if transitive { " (transitive)" } else { "" };
            if files.is_empty() {
                if !quiet {
                    println!(
                        "No {} found for {}{}",
                        relation, source_file, transitive_str
                    );
                }
            } else {
                println!(
                    "Found {} {}{} for {}:\n",
                    files.len(),
                    relation,
                    transitive_str,
                    source_file
                );
                for file in files {
                    println!("  {file}");
                }
            }
        }
    }

    Ok(())
}
