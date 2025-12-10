use crate::store::{EdgeRecord, ReferenceRecord, SymbolRecord, normalize_path};
use anyhow::{Context, Result};
use once_cell::sync::Lazy;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use tree_sitter::{Language, Node, Parser, TreeCursor};

static RUST_LANGUAGE: Lazy<Language> = Lazy::new(|| tree_sitter_rust::LANGUAGE.into());

pub fn index_file(
    path: &Path,
    source: &str,
) -> Result<(Vec<SymbolRecord>, Vec<EdgeRecord>, Vec<ReferenceRecord>)> {
    let mut parser = Parser::new();
    parser
        .set_language(&RUST_LANGUAGE)
        .context("failed to set Rust language")?;
    let tree = parser
        .parse(source, None)
        .context("failed to parse Rust file")?;

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

    let references = collect_references(
        path,
        source,
        &tree.root_node(),
        &declared_spans,
        &symbol_by_name,
    );

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
            "function_item" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = slice(source, &name_node);
                    let sym = make_symbol(path, &node, &name, "function", container.clone());
                    declared_spans.insert((sym.start as usize, sym.end as usize));
                    symbol_by_name.entry(name).or_insert(sym.id.clone());
                    symbols.push(sym);
                }
            }
            "struct_item" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = slice(source, &name_node);
                    let sym = make_symbol(path, &node, &name, "struct", container.clone());
                    declared_spans.insert((sym.start as usize, sym.end as usize));
                    symbol_by_name.entry(name).or_insert(sym.id.clone());
                    symbols.push(sym);
                }
            }
            "enum_item" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = slice(source, &name_node);
                    let sym = make_symbol(path, &node, &name, "enum", container.clone());
                    declared_spans.insert((sym.start as usize, sym.end as usize));
                    symbol_by_name.entry(name).or_insert(sym.id.clone());
                    symbols.push(sym);
                }
            }
            "trait_item" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = slice(source, &name_node);
                    let sym = make_symbol(path, &node, &name, "trait", container.clone());
                    declared_spans.insert((sym.start as usize, sym.end as usize));
                    symbol_by_name.entry(name).or_insert(sym.id.clone());
                    symbols.push(sym);
                }
            }
            _ => {}
        }

        if cursor.goto_first_child() {
            let child_container = if node.kind() == "impl_item" {
                impl_container_name(source, &node).or(container.clone())
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

fn impl_container_name(source: &str, node: &Node) -> Option<String> {
    node.child_by_field_name("type")
        .map(|ty| slice(source, &ty))
        .filter(|s| !s.is_empty())
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

fn slice(source: &str, node: &Node) -> String {
    let bytes = node.byte_range();
    source
        .get(bytes.clone())
        .unwrap_or_default()
        .trim()
        .to_string()
}
