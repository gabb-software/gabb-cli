//! File structure command: show hierarchical view of symbols in a file.

use anyhow::{bail, Result};
use std::collections::HashMap;
use std::path::Path;

use gabb_cli::is_test_file;
use gabb_cli::store::{normalize_path, SymbolQuery, SymbolRecord};
use gabb_cli::ExitCode;
use gabb_cli::OutputFormat;

use crate::output::{FileStructure, Position, SymbolNode};
use crate::util::{offset_to_line_char_in_file, open_store_for_query};

/// Summary information for a file's structure
struct FileSummary {
    /// Symbol counts by kind (e.g., "function" -> 45)
    counts_by_kind: Vec<(String, usize)>,
    /// Total line count in the file
    line_count: usize,
    /// Key types: public types with many methods, sorted by method count
    key_types: Vec<String>,
}

/// Compute summary statistics for a file's symbols
fn compute_file_summary(symbols: &[SymbolRecord], file_path: &Path) -> FileSummary {
    // Count symbols by kind
    let mut kind_counts: HashMap<String, usize> = HashMap::new();
    for sym in symbols {
        *kind_counts.entry(sym.kind.clone()).or_default() += 1;
    }

    // Sort by count descending, then by kind name
    let mut counts_by_kind: Vec<(String, usize)> = kind_counts.into_iter().collect();
    counts_by_kind.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    // Count lines in the file
    let line_count = std::fs::read_to_string(file_path)
        .map(|content| content.lines().count())
        .unwrap_or(0);

    // Find key types: public structs/classes/traits/interfaces with methods
    // Count methods per container
    let mut methods_per_container: HashMap<String, usize> = HashMap::new();
    for sym in symbols {
        if let Some(ref container) = sym.container {
            if sym.kind == "function" || sym.kind == "method" {
                *methods_per_container.entry(container.clone()).or_default() += 1;
            }
        }
    }

    // Filter to public types and sort by method count
    let type_kinds = ["struct", "class", "trait", "interface", "enum", "type"];
    let mut type_symbols: Vec<(&SymbolRecord, usize)> = symbols
        .iter()
        .filter(|s| {
            type_kinds.contains(&s.kind.as_str())
                && s.visibility.as_ref().map(|v| v == "pub").unwrap_or(false)
        })
        .map(|s| {
            let method_count = methods_per_container.get(&s.name).copied().unwrap_or(0);
            (s, method_count)
        })
        .collect();

    // Sort by method count descending
    type_symbols.sort_by(|a, b| b.1.cmp(&a.1));

    // Take top 5 key types with 3+ methods
    let key_types: Vec<String> = type_symbols
        .into_iter()
        .filter(|(_, count)| *count >= 3)
        .take(5)
        .map(|(sym, count)| {
            if count > 0 {
                format!("{} ({} methods)", sym.name, count)
            } else {
                sym.name.clone()
            }
        })
        .collect();

    FileSummary {
        counts_by_kind,
        line_count,
        key_types,
    }
}

/// Format the file summary as a compact string
fn format_file_summary(summary: &FileSummary) -> String {
    let mut parts = Vec::new();

    // Format counts by kind
    for (kind, count) in &summary.counts_by_kind {
        let plural = if *count == 1 { "" } else { "s" };
        parts.push(format!("{} {}{}", count, kind, plural));
    }

    let counts_str = parts.join(", ");
    let lines_str = format!("{} lines", summary.line_count);

    let mut result = format!("Summary: {} | {}", counts_str, lines_str);

    if !summary.key_types.is_empty() {
        result.push_str(&format!("\nKey types: {}", summary.key_types.join(", ")));
    }

    result
}

/// Build a hierarchical tree of symbols from flat records
fn build_symbol_tree(symbols: Vec<SymbolRecord>, file_path: &str) -> Result<Vec<SymbolNode>> {
    // Convert each symbol to a node with resolved positions
    let mut nodes: Vec<(Option<String>, SymbolNode)> = Vec::new();

    for sym in &symbols {
        let (start_line, start_col) = offset_to_line_char_in_file(&sym.file, sym.start)?;
        let (end_line, end_col) = offset_to_line_char_in_file(&sym.file, sym.end)?;

        // Determine context from file path OR inline test markers (#[cfg(test)], #[test])
        let context = if sym.is_test || is_test_file(file_path) {
            "test"
        } else {
            "prod"
        };

        let node = SymbolNode {
            name: sym.name.clone(),
            kind: sym.kind.clone(),
            context: context.to_string(),
            start: Position {
                line: start_line,
                character: start_col,
            },
            end: Position {
                line: end_line,
                character: end_col,
            },
            visibility: sym.visibility.clone(),
            children: Vec::new(),
            source: None,
        };

        nodes.push((sym.container.clone(), node));
    }

    // Group children by their container name
    let mut children_by_container: HashMap<String, Vec<SymbolNode>> = HashMap::new();
    let mut roots: Vec<SymbolNode> = Vec::new();

    for (container, node) in nodes {
        if let Some(container_name) = container {
            children_by_container
                .entry(container_name)
                .or_default()
                .push(node);
        } else {
            roots.push(node);
        }
    }

    // Recursively attach children to their parents
    fn attach_children(node: &mut SymbolNode, children_map: &mut HashMap<String, Vec<SymbolNode>>) {
        if let Some(children) = children_map.remove(&node.name) {
            node.children = children;
            for child in &mut node.children {
                attach_children(child, children_map);
            }
        }
    }

    for root in &mut roots {
        attach_children(root, &mut children_by_container);
    }

    // Any remaining orphans (container exists but parent wasn't found) become roots
    for (_, orphans) in children_by_container {
        roots.extend(orphans);
    }

    // Sort by start position
    roots.sort_by(|a, b| {
        a.start
            .line
            .cmp(&b.start.line)
            .then(a.start.character.cmp(&b.start.character))
    });

    Ok(roots)
}

/// Show the structure of a file (symbols with hierarchy and positions)
pub fn file_structure(
    db: &Path,
    workspace: &Path,
    file: &Path,
    format: OutputFormat,
    _quiet: bool,
) -> Result<ExitCode> {
    let store = open_store_for_query(db)?;

    // Resolve the file path
    let file_path = if file.is_absolute() {
        file.to_path_buf()
    } else {
        workspace.join(file)
    };
    let file_path = file_path
        .canonicalize()
        .unwrap_or_else(|_| file_path.clone());
    let file_str = normalize_path(&file_path);

    // Check if file exists
    if !file_path.exists() {
        bail!("File not found: {}", file_path.display());
    }

    // Query symbols for this file
    let query = SymbolQuery {
        file: Some(&file_str),
        ..Default::default()
    };
    let symbols: Vec<SymbolRecord> = store.list_symbols_filtered(&query)?;

    if symbols.is_empty() {
        bail!(
            "No symbols found in {}. Is it indexed? Run `gabb daemon start` to index.",
            file_str
        );
    }

    // Determine if this is a test file
    let context = if is_test_file(&file_str) {
        "test"
    } else {
        "prod"
    };

    // Compute summary (counts by kind, line count, key types)
    let summary = compute_file_summary(&symbols, &file_path);

    // Build the hierarchical tree
    let tree = build_symbol_tree(symbols, &file_str)?;

    let structure = FileStructure {
        file: file_str.clone(),
        context: context.to_string(),
        symbols: tree,
    };

    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&structure)?);
        }
        OutputFormat::Jsonl => {
            println!("{}", serde_json::to_string(&structure)?);
        }
        OutputFormat::Csv | OutputFormat::Tsv => {
            // Flatten the tree for CSV/TSV output
            fn flatten_nodes(nodes: &[SymbolNode], depth: usize, rows: &mut Vec<Vec<String>>) {
                for node in nodes {
                    let indent = "  ".repeat(depth);
                    rows.push(vec![
                        format!("{}{}", indent, node.name),
                        node.kind.clone(),
                        node.context.clone(),
                        format!("{}:{}", node.start.line, node.start.character),
                        format!("{}:{}", node.end.line, node.end.character),
                        node.visibility.clone().unwrap_or_default(),
                    ]);
                    flatten_nodes(&node.children, depth + 1, rows);
                }
            }

            let mut rows = Vec::new();
            flatten_nodes(&structure.symbols, 0, &mut rows);

            if matches!(format, OutputFormat::Csv) {
                let mut wtr = csv::Writer::from_writer(std::io::stdout());
                wtr.write_record(["name", "kind", "context", "start", "end", "visibility"])?;
                for row in rows {
                    wtr.write_record(&row)?;
                }
                wtr.flush()?;
            } else {
                println!("name\tkind\tcontext\tstart\tend\tvisibility");
                for row in rows {
                    println!("{}", row.join("\t"));
                }
            }
        }
        OutputFormat::Text => {
            println!("{} ({})", structure.file, structure.context);
            println!("{}", format_file_summary(&summary));
            print_tree(&structure.symbols, "");
        }
    }

    Ok(ExitCode::Success)
}

/// Print symbol tree with ASCII art indentation
fn print_tree(nodes: &[SymbolNode], prefix: &str) {
    for (i, node) in nodes.iter().enumerate() {
        let is_last = i == nodes.len() - 1;
        let connector = if is_last { "└─" } else { "├─" };
        let position = format!(
            "[{}:{} - {}:{}]",
            node.start.line, node.start.character, node.end.line, node.end.character
        );
        let visibility = node
            .visibility
            .as_ref()
            .map(|v| format!(" ({})", v))
            .unwrap_or_default();
        let context_indicator = format!(" [{}]", node.context);

        println!(
            "{}{} {} {}{}{}  {}",
            prefix, connector, node.kind, node.name, visibility, context_indicator, position
        );

        if !node.children.is_empty() {
            let child_prefix = if is_last {
                format!("{}   ", prefix)
            } else {
                format!("{}│  ", prefix)
            };
            print_tree(&node.children, &child_prefix);
        }
    }
}
