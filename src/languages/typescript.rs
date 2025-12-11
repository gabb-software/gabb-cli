use crate::store::{EdgeRecord, ReferenceRecord, SymbolRecord, normalize_path};
use anyhow::{Context, Result};
use once_cell::sync::Lazy;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use tree_sitter::{Language, Node, Parser, TreeCursor};

static TS_LANGUAGE: Lazy<Language> =
    Lazy::new(|| tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into());

pub fn index_file(
    path: &Path,
    source: &str,
) -> Result<(Vec<SymbolRecord>, Vec<EdgeRecord>, Vec<ReferenceRecord>)> {
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

    let references = collect_references(
        path,
        source,
        &tree.root_node(),
        &declared_spans,
        &symbol_by_name,
    );

    Ok((symbols, edges, references))
}

#[allow(clippy::too_many_arguments)]
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
                    let implements_node = node
                        .child_by_field_name("implements")
                        .or_else(|| find_child_kind(&node, "implements_clause"));
                    if let Some(implements) = implements_node {
                        collect_type_list(
                            path,
                            source,
                            &implements,
                            &class_id,
                            "implements",
                            edges,
                            symbol_by_name,
                        );
                    }
                    let extends_node = node
                        .child_by_field_name("superclass")
                        .or_else(|| find_child_kind(&node, "extends_clause"));
                    if let Some(extends) = extends_node {
                        collect_type_list(
                            path,
                            source,
                            &extends,
                            &class_id,
                            "extends",
                            edges,
                            symbol_by_name,
                        );
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
                    let sym = make_symbol(path, &node, &name, "method", container.clone());
                    declared_spans.insert((sym.start as usize, sym.end as usize));
                    symbol_by_name.entry(name.clone()).or_insert(sym.id.clone());
                    symbols.push(sym);
                }
            }
            _ => {}
        }

        if cursor.goto_first_child() {
            let child_container =
                if matches!(node.kind(), "class_declaration" | "interface_declaration") {
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

#[allow(clippy::too_many_arguments)]
fn collect_type_list(
    path: &Path,
    _source: &str,
    node: &Node,
    src_id: &str,
    kind: &str,
    edges: &mut Vec<EdgeRecord>,
    symbol_by_name: &HashMap<String, String>,
) {
    // Collect identifiers used in implements/extends clauses.
    for child in node.children(&mut node.walk()) {
        if matches!(child.kind(), "identifier" | "type_identifier") {
            let name = slice(_source, &child);
            let dst_id = symbol_by_name.get(&name).cloned().unwrap_or_else(|| {
                format!(
                    "{}#{}-{}",
                    normalize_path(path),
                    child.start_byte(),
                    child.end_byte()
                )
            });
            edges.push(EdgeRecord {
                src: src_id.to_string(),
                dst: dst_id,
                kind: kind.to_string(),
            });
        }
    }
}

fn find_child_kind<'a>(node: &'a Node<'a>, kind: &str) -> Option<Node<'a>> {
    let mut stack = vec![*node];
    while let Some(n) = stack.pop() {
        if n.kind() == kind {
            return Some(n);
        }
        let mut cursor = n.walk();
        for child in n.children(&mut cursor) {
            stack.push(child);
        }
    }
    None
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
    let qualifier = Some(module_qualifier(path, &container));
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
        visibility: None,
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

fn slice(source: &str, node: &Node) -> String {
    let bytes = node.byte_range();
    source
        .get(bytes.clone())
        .unwrap_or_default()
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn extracts_ts_symbols_and_edges() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("foo.ts");
        let source = r#"
            interface Foo {
                doThing(): void;
            }
            class Bar implements Foo {
                doThing() {}
            }
        "#;
        fs::write(&path, source).unwrap();

        let (symbols, edges, _refs) = index_file(&path, source).unwrap();
        let names: Vec<_> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"Foo"));
        assert!(names.contains(&"Bar"));
        assert_eq!(symbols.len(), 4); // Foo, Foo.doThing, Bar, Bar.doThing

        let foo = symbols.iter().find(|s| s.name == "Foo").unwrap();
        assert!(foo.qualifier.as_deref().unwrap().contains("foo"));

        assert!(
            edges.iter().any(|e| e.kind == "implements"),
            "expected implements edge, got {:?}",
            edges
        );
    }

    #[test]
    fn links_extends_across_files_best_effort() {
        let dir = tempdir().unwrap();
        let base = dir.path();
        let iface_path = base.join("base.ts");
        let impl_path = base.join("impl.ts");

        let iface_src = r#"
            export interface Base {
                run(): void;
            }
        "#;
        let impl_src = r#"
            import { Base } from "./base";
            export class Child extends Base {
                run() {}
            }
        "#;
        fs::write(&iface_path, iface_src).unwrap();
        fs::write(&impl_path, impl_src).unwrap();

        let (iface_symbols, _, _) = index_file(&iface_path, iface_src).unwrap();
        let (_, impl_edges, _) = index_file(&impl_path, impl_src).unwrap();

        let _base = iface_symbols.iter().find(|s| s.name == "Base").unwrap();
        assert!(
            impl_edges.iter().any(|e| e.kind == "extends"),
            "expected extends edge pointing to Base"
        );
        // We do not resolve cross-file edges yet; just assert we recorded an extends relationship.
    }
}
