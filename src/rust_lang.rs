use crate::store::{EdgeRecord, ReferenceRecord, SymbolRecord, normalize_path};
use anyhow::{Context, Result};
use once_cell::sync::Lazy;
use std::collections::{HashMap, HashSet};
use std::fs;
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
    let qualifier = Some(module_qualifier(path, &container));
    let visibility = visibility(node, path);
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
        qualifier,
        visibility,
        container,
    }
}

fn module_qualifier(path: &Path, container: &Option<String>) -> String {
    let mut base = normalize_path(path);
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        let trim = ext.len() + 1;
        if base.len() > trim {
            base.truncate(base.len() - trim);
        }
    }
    if let Some(c) = container {
        base.push_str("::");
        base.push_str(c);
    }
    base
}

fn visibility(node: &Node, path: &Path) -> Option<String> {
    if let Some(vis) = node.child_by_field_name("visibility") {
        let text = slice_file(path, &vis);
        if !text.is_empty() {
            return Some(text);
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "visibility_modifier" || child.kind() == "pub" {
            let text = slice_file(path, &child);
            if !text.is_empty() {
                return Some(text);
            }
        }
    }
    None
}

fn slice(source: &str, node: &Node) -> String {
    let bytes = node.byte_range();
    source
        .get(bytes.clone())
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn slice_file(path: &Path, node: &Node) -> String {
    // Best-effort visibility slice using the file contents; if missing, fall back to node text.
    let source = fs::read_to_string(path).unwrap_or_default();
    slice(&source, node)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn extracts_rust_symbols_and_visibility() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("mod.rs");
        let source = r#"
            pub struct Thing;
            impl Thing {
                pub fn make() {}
            }
        "#;
        fs::write(&path, source).unwrap();

        let (symbols, _edges, _refs) = index_file(&path, source).unwrap();
        let names: Vec<_> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"Thing"));
        assert!(names.contains(&"make"));

        let thing = symbols.iter().find(|s| s.name == "Thing").unwrap();
        assert_eq!(thing.visibility.as_deref(), Some("pub"));
        assert!(thing.qualifier.as_deref().unwrap().contains("mod"));

        let make = symbols.iter().find(|s| s.name == "make").unwrap();
        assert_eq!(make.kind, "function");
    }
}
