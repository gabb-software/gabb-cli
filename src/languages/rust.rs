use crate::store::{EdgeRecord, FileDependency, ReferenceRecord, SymbolRecord, normalize_path};
use anyhow::{Context, Result};
use once_cell::sync::Lazy;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use tree_sitter::{Language, Node, Parser, TreeCursor};

static RUST_LANGUAGE: Lazy<Language> = Lazy::new(|| tree_sitter_rust::LANGUAGE.into());

#[derive(Clone, Debug)]
struct SymbolBinding {
    id: String,
    qualifier: Option<String>,
}

impl From<&SymbolRecord> for SymbolBinding {
    fn from(value: &SymbolRecord) -> Self {
        Self {
            id: value.id.clone(),
            qualifier: value.qualifier.clone(),
        }
    }
}

#[derive(Clone, Debug)]
struct ResolvedTarget {
    id: String,
    qualifier: Option<String>,
}

impl ResolvedTarget {
    fn member_id(&self, member: &str) -> String {
        if let Some(q) = &self.qualifier {
            format!("{q}::{member}")
        } else {
            format!("{}::{member}", self.id)
        }
    }
}

/// Index a Rust file, returning symbols, edges, references, and file dependencies.
pub fn index_file(
    path: &Path,
    source: &str,
) -> Result<(
    Vec<SymbolRecord>,
    Vec<EdgeRecord>,
    Vec<ReferenceRecord>,
    Vec<FileDependency>,
)> {
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
    let mut symbol_by_name: HashMap<String, SymbolBinding> = HashMap::new();

    {
        let mut cursor = tree.walk();
        walk_symbols(
            path,
            source,
            &mut cursor,
            None,
            &[],
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

    // Extract file dependencies from mod declarations
    let dependencies = collect_mod_dependencies(path, source, &tree.root_node());

    Ok((symbols, edges, references, dependencies))
}

/// Extract file dependencies from `mod` declarations (e.g., `mod foo;`).
/// These indicate that this file depends on foo.rs or foo/mod.rs.
fn collect_mod_dependencies(path: &Path, source: &str, root: &Node) -> Vec<FileDependency> {
    let mut dependencies = Vec::new();
    let mut seen = HashSet::new();
    let from_file = normalize_path(path);
    let parent = path.parent();

    let mut stack = vec![*root];
    while let Some(node) = stack.pop() {
        // Look for `mod foo;` declarations (without body)
        if node.kind() == "mod_item" {
            let has_body = node
                .children(&mut node.walk())
                .any(|c| c.kind() == "declaration_list");
            if !has_body {
                // This is a `mod foo;` declaration - it references an external file
                if let Some(name_node) = node.child_by_field_name("name") {
                    let mod_name = slice(source, &name_node);
                    if !mod_name.is_empty() && !seen.contains(&mod_name) {
                        seen.insert(mod_name.clone());

                        // Try to resolve the module file path
                        if let Some(parent_dir) = parent {
                            // Try mod_name.rs
                            let mod_file = parent_dir.join(format!("{}.rs", mod_name));
                            let mod_dir_file = parent_dir.join(&mod_name).join("mod.rs");

                            let to_file = if mod_file.exists() {
                                normalize_path(&mod_file)
                            } else if mod_dir_file.exists() {
                                normalize_path(&mod_dir_file)
                            } else {
                                // Use the expected path even if it doesn't exist
                                normalize_path(&mod_file)
                            };

                            dependencies.push(FileDependency {
                                from_file: from_file.clone(),
                                to_file,
                                kind: "mod".to_string(),
                            });
                        }
                    }
                }
            }
        }

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            stack.push(child);
        }
    }

    dependencies
}

#[allow(clippy::too_many_arguments, clippy::only_used_in_recursion)]
fn walk_symbols(
    path: &Path,
    source: &str,
    cursor: &mut TreeCursor,
    container: Option<String>,
    module_path: &[String],
    impl_trait: Option<ResolvedTarget>,
    symbols: &mut Vec<SymbolRecord>,
    edges: &mut Vec<EdgeRecord>,
    declared_spans: &mut HashSet<(usize, usize)>,
    symbol_by_name: &mut HashMap<String, SymbolBinding>,
) {
    loop {
        let node = cursor.node();
        match node.kind() {
            "function_item" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = slice(source, &name_node);
                    let sym = make_symbol(
                        path,
                        module_path,
                        &node,
                        &name,
                        "function",
                        container.clone(),
                    );
                    declared_spans.insert((sym.start as usize, sym.end as usize));
                    symbol_by_name
                        .entry(name.clone())
                        .or_insert_with(|| SymbolBinding::from(&sym));
                    if let Some(trait_target) = &impl_trait {
                        edges.push(EdgeRecord {
                            src: sym.id.clone(),
                            dst: trait_target.member_id(&name),
                            kind: "overrides".to_string(),
                        });
                    }
                    if let Some(parent) = &container {
                        if let Some(binding) = symbol_by_name.get(parent) {
                            edges.push(EdgeRecord {
                                src: sym.id.clone(),
                                dst: binding.id.clone(),
                                kind: "inherent_impl".to_string(),
                            });
                        }
                    }
                    symbols.push(sym);
                }
            }
            "struct_item" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = slice(source, &name_node);
                    let sym =
                        make_symbol(path, module_path, &node, &name, "struct", container.clone());
                    declared_spans.insert((sym.start as usize, sym.end as usize));
                    symbol_by_name
                        .entry(name)
                        .or_insert_with(|| SymbolBinding::from(&sym));
                    symbols.push(sym);
                }
            }
            "enum_item" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = slice(source, &name_node);
                    let sym =
                        make_symbol(path, module_path, &node, &name, "enum", container.clone());
                    declared_spans.insert((sym.start as usize, sym.end as usize));
                    symbol_by_name
                        .entry(name)
                        .or_insert_with(|| SymbolBinding::from(&sym));
                    symbols.push(sym);
                }
            }
            "trait_item" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = slice(source, &name_node);
                    let sym =
                        make_symbol(path, module_path, &node, &name, "trait", container.clone());
                    declared_spans.insert((sym.start as usize, sym.end as usize));
                    symbol_by_name
                        .entry(name)
                        .or_insert_with(|| SymbolBinding::from(&sym));
                    symbols.push(sym);
                }
            }
            "mod_item" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = slice(source, &name_node);
                    let mut mod_path = module_path.to_vec();
                    mod_path.push(name);
                    if cursor.goto_first_child() {
                        walk_symbols(
                            path,
                            source,
                            cursor,
                            container.clone(),
                            &mod_path,
                            None,
                            symbols,
                            edges,
                            declared_spans,
                            symbol_by_name,
                        );
                        cursor.goto_parent();
                    }
                    if cursor.goto_next_sibling() {
                        continue;
                    } else {
                        break;
                    }
                }
            }
            _ => {}
        }

        if cursor.goto_first_child() {
            let mut child_container = container.clone();
            let mut child_trait = impl_trait.clone();
            let child_modules = module_path.to_vec();
            if node.kind() == "impl_item" {
                let (ty, trait_target) =
                    record_impl_edges(path, source, &node, module_path, symbol_by_name, edges);
                child_container = ty.or(container.clone());
                child_trait = trait_target;
            }
            walk_symbols(
                path,
                source,
                cursor,
                child_container,
                &child_modules,
                child_trait,
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
    symbol_by_name: &HashMap<String, SymbolBinding>,
) -> Vec<ReferenceRecord> {
    let mut refs = Vec::new();
    let mut stack = vec![*root];
    let file = normalize_path(path);

    while let Some(node) = stack.pop() {
        if node.kind() == "identifier" {
            let span = (node.start_byte(), node.end_byte());
            if !declared_spans.contains(&span) {
                let name = slice(source, &node);
                if let Some(sym) = symbol_by_name.get(&name) {
                    refs.push(ReferenceRecord {
                        file: file.clone(),
                        start: node.start_byte() as i64,
                        end: node.end_byte() as i64,
                        symbol_id: sym.id.clone(),
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

fn record_impl_edges(
    path: &Path,
    source: &str,
    node: &Node,
    module_path: &[String],
    symbol_by_name: &HashMap<String, SymbolBinding>,
    edges: &mut Vec<EdgeRecord>,
) -> (Option<String>, Option<ResolvedTarget>) {
    let ty_name = node
        .child_by_field_name("type")
        .map(|ty| slice(source, &ty))
        .filter(|s| !s.is_empty());
    let trait_name = node
        .child_by_field_name("trait")
        .map(|tr| slice(source, &tr))
        .filter(|s| !s.is_empty());

    let mut trait_target = None;
    if let (Some(ty), Some(tr)) = (ty_name.as_ref(), trait_name.as_ref()) {
        let src = resolve_rust_name(
            ty,
            Some((node.start_byte(), node.end_byte())),
            path,
            module_path,
            symbol_by_name,
        );
        let dst = resolve_rust_name(
            tr,
            Some((node.start_byte(), node.end_byte())),
            path,
            module_path,
            symbol_by_name,
        );
        trait_target = Some(dst.clone());
        edges.push(EdgeRecord {
            src: src.id,
            dst: dst.id,
            kind: "trait_impl".to_string(),
        });
    }

    (ty_name, trait_target)
}

fn resolve_rust_name(
    name: &str,
    span: Option<(usize, usize)>,
    path: &Path,
    module_path: &[String],
    symbol_by_name: &HashMap<String, SymbolBinding>,
) -> ResolvedTarget {
    if let Some(binding) = symbol_by_name.get(name) {
        return ResolvedTarget {
            id: binding.id.clone(),
            qualifier: binding.qualifier.clone(),
        };
    }
    let prefix = module_prefix(path, module_path);
    let id = match span {
        Some((start, end)) => format!("{}#{}-{}", normalize_path(path), start, end),
        None => format!("{prefix}::{name}"),
    };
    let qualifier = Some(prefix);
    ResolvedTarget { id, qualifier }
}

fn module_prefix(path: &Path, module_path: &[String]) -> String {
    let mut base = normalize_path(path);
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        let trim = ext.len() + 1;
        if base.len() > trim {
            base.truncate(base.len() - trim);
        }
    }
    for segment in module_path {
        base.push_str("::");
        base.push_str(segment);
    }
    base
}

fn make_symbol(
    path: &Path,
    module_path: &[String],
    node: &Node,
    name: &str,
    kind: &str,
    container: Option<String>,
) -> SymbolRecord {
    let qualifier = Some(module_qualifier(path, module_path, &container));
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

fn module_qualifier(path: &Path, module_path: &[String], container: &Option<String>) -> String {
    let mut base = module_prefix(path, module_path);
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
            pub mod inner {
                pub struct Thing;
                impl Thing {
                    pub fn make() {}
                }
            }
        "#;
        fs::write(&path, source).unwrap();

        let (symbols, edges, _refs, _deps) = index_file(&path, source).unwrap();
        let names: Vec<_> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"Thing"));
        assert!(names.contains(&"make"));

        let thing = symbols.iter().find(|s| s.name == "Thing").unwrap();
        assert_eq!(thing.visibility.as_deref(), Some("pub"));
        assert!(thing.qualifier.as_deref().unwrap().contains("mod::inner"));

        let make = symbols.iter().find(|s| s.name == "make").unwrap();
        assert_eq!(make.kind, "function");
        assert!(
            edges.iter().any(|e| e.kind == "inherent_impl"),
            "expected inherent_impl edge from make to Thing"
        );
    }

    #[test]
    fn captures_trait_impl_relationship() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("impl.rs");
        let source = r#"
            trait Greeter {
                fn greet(&self);
            }
            struct Person;
            impl Greeter for Person {
                fn greet(&self) {}
            }
        "#;
        fs::write(&path, source).unwrap();

        let (symbols, edges, _refs, _deps) = index_file(&path, source).unwrap();
        let person = symbols.iter().find(|s| s.name == "Person").unwrap();
        let greeter = symbols.iter().find(|s| s.name == "Greeter").unwrap();

        assert!(symbols.iter().any(|s| s.name == "greet"));
        let path_str = path.to_string_lossy();
        assert!(person.id.starts_with(path_str.as_ref()));
        assert!(greeter.id.starts_with(path_str.as_ref()));
        assert!(
            edges.iter().any(|e| e.kind == "trait_impl"),
            "expected trait_impl edge"
        );
        assert!(
            edges.iter().any(|e| e.kind == "overrides"),
            "expected method overrides edges for trait methods"
        );
    }
}
