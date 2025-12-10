use crate::store::{normalize_path, EdgeRecord, ReferenceRecord, SymbolRecord};
use anyhow::{Context, Result};
use once_cell::sync::Lazy;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use tree_sitter::{Language, Node, Parser, TreeCursor};

static TS_LANGUAGE: Lazy<Language> =
    Lazy::new(|| tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into());

pub fn index_file(path: &Path, source: &str) -> Result<(Vec<SymbolRecord>, Vec<EdgeRecord>, Vec<ReferenceRecord>)> {
    let mut parser = Parser::new();
    parser
        .set_language(&TS_LANGUAGE)
        .context("failed to set TypeScript language")?;
    let tree = parser
        .parse(source, None)
        .context("failed to parse TypeScript file")?;

    let mut symbols = Vec::new();
    let mut edges = Vec::new();
    let mut declared_spans: HashSet<(usize, usize)> = HashSet::new();
    let mut symbol_by_name: HashMap<String, String> = HashMap::new();

    {
        let mut cursor = tree.walk();
        walk_symbols(
            path,
            source,
            &mut cursor,
            None,
            &mut symbols,
            &mut edges,
            &mut declared_spans,
            &mut symbol_by_name,
        );
    }

    let references =
        collect_references(path, source, &tree.root_node(), &declared_spans, &symbol_by_name);

    Ok((symbols, edges, references))
}

fn walk_symbols(
    path: &Path,
    source: &str,
    cursor: &mut TreeCursor,
    container: Option<String>,
    symbols: &mut Vec<SymbolRecord>,
    edges: &mut Vec<EdgeRecord>,
    declared_spans: &mut HashSet<(usize, usize)>,
    symbol_by_name: &mut HashMap<String, String>,
) {
    loop {
        let node = cursor.node();
        match node.kind() {
            "function_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = slice(source, &name_node);
                    let sym = make_symbol(path, &node, &name, "function", container.clone());
                    declared_spans.insert((sym.start as usize, sym.end as usize));
                    symbol_by_name.entry(name.clone()).or_insert(sym.id.clone());
                    symbols.push(sym);
                }
            }
            "class_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = slice(source, &name_node);
                    let class_id;
                    {
                        let sym = make_symbol(path, &node, &name, "class", container.clone());
                        class_id = sym.id.clone();
                        declared_spans.insert((sym.start as usize, sym.end as usize));
                        symbol_by_name.entry(name.clone()).or_insert(sym.id.clone());
                        symbols.push(sym);
                    }
                    if let Some(implements) = node.child_by_field_name("implements") {
                        collect_type_list(path, source, &implements, &class_id, "implements", edges);
                    }
                    if let Some(extends) = node.child_by_field_name("superclass") {
                        collect_type_list(path, source, &extends, &class_id, "extends", edges);
                    }
                }
            }
            "interface_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = slice(source, &name_node);
                    let sym = make_symbol(path, &node, &name, "interface", container.clone());
                    declared_spans.insert((sym.start as usize, sym.end as usize));
                    symbol_by_name.entry(name.clone()).or_insert(sym.id.clone());
                    symbols.push(sym);
                }
            }
            "method_definition" | "method_signature" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = slice(source, &name_node);
                    let sym =
                        make_symbol(path, &node, &name, "method", container.clone());
                    declared_spans.insert((sym.start as usize, sym.end as usize));
                    symbol_by_name.entry(name.clone()).or_insert(sym.id.clone());
                    symbols.push(sym);
                }
            }
            _ => {}
        }

        if cursor.goto_first_child() {
            let child_container = if matches!(
                node.kind(),
                "class_declaration" | "interface_declaration"
            ) {
                node.child_by_field_name("name")
                    .map(|n| slice(source, &n))
                    .or(container.clone())
            } else {
                container.clone()
            };
            walk_symbols(
                path,
                source,
                cursor,
                child_container,
                symbols,
                edges,
                declared_spans,
                symbol_by_name,
            );
            cursor.goto_parent();
        }

        if !cursor.goto_next_sibling() {
            break;
        }
    }
}

fn collect_type_list(
    path: &Path,
    _source: &str,
    node: &Node,
    src_id: &str,
    kind: &str,
    edges: &mut Vec<EdgeRecord>,
) {
    // Collect identifiers used in implements/extends clauses.
    for child in node.children(&mut node.walk()) {
        if child.kind() == "identifier" {
            let dst_id = format!(
                "{}#{}-{}",
                normalize_path(path),
                child.start_byte(),
                child.end_byte()
            );
            edges.push(EdgeRecord {
                src: src_id.to_string(),
                dst: dst_id,
                kind: kind.to_string(),
            });
        }
    }
}

fn collect_references(
    path: &Path,
    source: &str,
    root: &Node,
    declared_spans: &HashSet<(usize, usize)>,
    symbol_by_name: &HashMap<String, String>,
) -> Vec<ReferenceRecord> {
    let mut refs = Vec::new();
    let mut stack = vec![*root];
    let file = normalize_path(path);

    while let Some(node) = stack.pop() {
        if node.kind() == "identifier" {
            let span = (node.start_byte(), node.end_byte());
            if !declared_spans.contains(&span) {
                let name = slice(source, &node);
                if let Some(sym_id) = symbol_by_name.get(&name) {
                    refs.push(ReferenceRecord {
                        file: file.clone(),
                        start: node.start_byte() as i64,
                        end: node.end_byte() as i64,
                        symbol_id: sym_id.clone(),
                    });
                }
            }
        }

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            stack.push(child);
        }
    }

    refs
}

fn make_symbol(
    path: &Path,
    node: &Node,
    name: &str,
    kind: &str,
    container: Option<String>,
) -> SymbolRecord {
    SymbolRecord {
        id: format!(
            "{}#{}-{}",
            normalize_path(path),
            node.start_byte(),
            node.end_byte()
        ),
        file: normalize_path(path),
        kind: kind.to_string(),
        name: name.to_string(),
        start: node.start_byte() as i64,
        end: node.end_byte() as i64,
        container,
    }
}

fn slice<'a>(source: &'a str, node: &Node) -> String {
    let bytes = node.byte_range();
    source
        .get(bytes.clone())
        .unwrap_or_default()
        .trim()
        .to_string()
}
