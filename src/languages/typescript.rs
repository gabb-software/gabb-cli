use crate::languages::ImportBindingInfo;
use crate::store::{EdgeRecord, FileDependency, ReferenceRecord, SymbolRecord, normalize_path};
use anyhow::{Context, Result};
use once_cell::sync::Lazy;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use tree_sitter::{Language, Node, Parser, TreeCursor};

static TS_LANGUAGE: Lazy<Language> =
    Lazy::new(|| tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into());

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
struct ImportBinding {
    qualifier: Option<String>,
    imported_name: Option<String>,
}

impl ImportBinding {
    fn new(qualifier: Option<String>, imported_name: Option<String>) -> Self {
        Self {
            qualifier,
            imported_name,
        }
    }

    fn symbol_id(&self, fallback: &str) -> String {
        let name = self.imported_name.as_deref().unwrap_or(fallback);
        if let Some(q) = &self.qualifier {
            format!("{q}::{name}")
        } else {
            fallback.to_string()
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

/// Index a TypeScript/TSX file, returning symbols, edges, references, file dependencies, and import bindings.
pub fn index_file(
    path: &Path,
    source: &str,
) -> Result<(
    Vec<SymbolRecord>,
    Vec<EdgeRecord>,
    Vec<ReferenceRecord>,
    Vec<FileDependency>,
    Vec<ImportBindingInfo>,
)> {
    let mut parser = Parser::new();
    parser
        .set_language(&TS_LANGUAGE)
        .context("failed to set TypeScript language")?;
    let tree = parser
        .parse(source, None)
        .context("failed to parse TypeScript file")?;

    let mut symbols = Vec::new();
    let mut declared_spans: HashSet<(usize, usize)> = HashSet::new();
    let mut symbol_by_name: HashMap<String, SymbolBinding> = HashMap::new();
    let (imports, mut edges, dependencies, import_bindings) =
        collect_import_bindings(path, source, &tree.root_node());

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
            &imports,
        );
    }

    let references = collect_references(
        path,
        source,
        &tree.root_node(),
        &declared_spans,
        &symbol_by_name,
        &imports,
    );
    edges.extend(collect_export_edges(
        path,
        source,
        &tree.root_node(),
        &symbol_by_name,
        &imports,
    ));

    Ok((symbols, edges, references, dependencies, import_bindings))
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
    symbol_by_name: &mut HashMap<String, SymbolBinding>,
    imports: &HashMap<String, ImportBinding>,
) {
    loop {
        let node = cursor.node();
        match node.kind() {
            "function_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = slice(source, &name_node);
                    let sym = make_symbol(path, &node, &name, "function", container.clone(), source.as_bytes());
                    declared_spans.insert((sym.start as usize, sym.end as usize));
                    symbol_by_name
                        .entry(name.clone())
                        .or_insert_with(|| SymbolBinding::from(&sym));
                    symbols.push(sym);
                }
            }
            "class_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = slice(source, &name_node);
                    let sym = make_symbol(path, &node, &name, "class", container.clone(), source.as_bytes());
                    declared_spans.insert((sym.start as usize, sym.end as usize));
                    symbol_by_name
                        .entry(name.clone())
                        .or_insert_with(|| SymbolBinding::from(&sym));
                    let class_id = sym.id.clone();
                    symbols.push(sym);

                    let implements_node = node
                        .child_by_field_name("implements")
                        .or_else(|| find_child_kind(&node, "implements_clause"));
                    if let Some(implements) = implements_node {
                        for target in
                            collect_type_targets(path, source, &implements, symbol_by_name, imports)
                        {
                            edges.push(EdgeRecord {
                                src: class_id.clone(),
                                dst: target.id,
                                kind: "implements".to_string(),
                            });
                        }
                    }
                    let extends_node = node
                        .child_by_field_name("superclass")
                        .or_else(|| find_child_kind(&node, "extends_clause"));
                    if let Some(extends) = extends_node {
                        for target in
                            collect_type_targets(path, source, &extends, symbol_by_name, imports)
                        {
                            edges.push(EdgeRecord {
                                src: class_id.clone(),
                                dst: target.id,
                                kind: "extends".to_string(),
                            });
                        }
                    }
                }
            }
            "interface_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = slice(source, &name_node);
                    let sym = make_symbol(path, &node, &name, "interface", container.clone(), source.as_bytes());
                    declared_spans.insert((sym.start as usize, sym.end as usize));
                    symbol_by_name
                        .entry(name.clone())
                        .or_insert_with(|| SymbolBinding::from(&sym));
                    symbols.push(sym);
                }
            }
            "method_definition" | "method_signature" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = slice(source, &name_node);
                    let sym = make_symbol(path, &node, &name, "method", container.clone(), source.as_bytes());
                    declared_spans.insert((sym.start as usize, sym.end as usize));
                    symbol_by_name
                        .entry(name.clone())
                        .or_insert_with(|| SymbolBinding::from(&sym));
                    add_override_edges(
                        path,
                        source,
                        &node,
                        &name,
                        &sym.id,
                        edges,
                        symbol_by_name,
                        imports,
                    );
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
                imports,
            );
            cursor.goto_parent();
        }

        if !cursor.goto_next_sibling() {
            break;
        }
    }
}

fn collect_type_targets(
    path: &Path,
    source: &str,
    node: &Node,
    symbol_by_name: &HashMap<String, SymbolBinding>,
    imports: &HashMap<String, ImportBinding>,
) -> Vec<ResolvedTarget> {
    let mut targets = Vec::new();
    for child in node.children(&mut node.walk()) {
        if matches!(
            child.kind(),
            "identifier" | "type_identifier" | "nested_type_identifier"
        ) {
            targets.push(resolve_target(
                path,
                source,
                &child,
                symbol_by_name,
                imports,
            ));
        }
    }
    targets
}

fn resolve_target(
    path: &Path,
    source: &str,
    node: &Node,
    symbol_by_name: &HashMap<String, SymbolBinding>,
    imports: &HashMap<String, ImportBinding>,
) -> ResolvedTarget {
    let name = slice(source, node);
    resolve_name(
        &name,
        Some((node.start_byte(), node.end_byte())),
        path,
        symbol_by_name,
        imports,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
fn resolve_name(
    name: &str,
    span: Option<(usize, usize)>,
    path: &Path,
    symbol_by_name: &HashMap<String, SymbolBinding>,
    imports: &HashMap<String, ImportBinding>,
    qualifier_override: Option<String>,
) -> ResolvedTarget {
    if let Some(binding) = symbol_by_name.get(name) {
        return ResolvedTarget {
            id: binding.id.clone(),
            qualifier: binding.qualifier.clone(),
        };
    }
    if let Some(q) = qualifier_override {
        return ResolvedTarget {
            id: format!("{q}::{name}"),
            qualifier: Some(q),
        };
    }
    if let Some(binding) = imports.get(name) {
        let id = binding.symbol_id(name);
        return ResolvedTarget {
            id,
            qualifier: binding.qualifier.clone(),
        };
    }
    let fallback = if let Some((start, end)) = span {
        format!("{}#{}-{}", normalize_path(path), start, end)
    } else {
        format!("{}::{}", normalize_path(path), name)
    };
    ResolvedTarget {
        id: fallback,
        qualifier: None,
    }
}

#[allow(clippy::too_many_arguments)]
fn add_override_edges(
    path: &Path,
    source: &str,
    node: &Node,
    method_name: &str,
    method_id: &str,
    edges: &mut Vec<EdgeRecord>,
    symbol_by_name: &HashMap<String, SymbolBinding>,
    imports: &HashMap<String, ImportBinding>,
) {
    if let Some(class_node) = find_enclosing_class(node.parent()) {
        let implements = class_node
            .child_by_field_name("implements")
            .or_else(|| find_child_kind(&class_node, "implements_clause"))
            .map(|n| collect_type_targets(path, source, &n, symbol_by_name, imports))
            .unwrap_or_default();
        let supers = class_node
            .child_by_field_name("superclass")
            .or_else(|| find_child_kind(&class_node, "extends_clause"))
            .map(|n| collect_type_targets(path, source, &n, symbol_by_name, imports))
            .unwrap_or_default();

        for target in implements.iter().chain(supers.iter()) {
            edges.push(EdgeRecord {
                src: method_id.to_string(),
                dst: target.member_id(method_name),
                kind: "overrides".to_string(),
            });
        }
    }
}

fn find_enclosing_class(mut node: Option<Node>) -> Option<Node> {
    while let Some(n) = node {
        if n.kind() == "class_declaration" {
            return Some(n);
        }
        node = n.parent();
    }
    None
}

fn collect_import_bindings(
    path: &Path,
    source: &str,
    root: &Node,
) -> (
    HashMap<String, ImportBinding>,
    Vec<EdgeRecord>,
    Vec<FileDependency>,
    Vec<ImportBindingInfo>,
) {
    let mut imports = HashMap::new();
    let mut edges = Vec::new();
    let mut dependencies = Vec::new();
    let mut import_binding_infos = Vec::new();
    let mut seen_deps: HashSet<String> = HashSet::new();
    let mut stack = vec![*root];
    let from_file = normalize_path(path);

    while let Some(node) = stack.pop() {
        if node.kind() == "import_statement" {
            let raw_source = node
                .child_by_field_name("source")
                .map(|s| slice(source, &s));

            let qualifier = raw_source.as_ref().map(|raw| import_qualifier(path, raw));

            // Record file dependency with resolved path
            let resolved_source = raw_source.as_ref().and_then(|raw| resolve_import_path(path, raw));
            if let Some(ref resolved) = resolved_source {
                if !seen_deps.contains(resolved) {
                    seen_deps.insert(resolved.clone());
                    dependencies.push(FileDependency {
                        from_file: from_file.clone(),
                        to_file: resolved.clone(),
                        kind: "import".to_string(),
                    });
                }
            }

            let mut import_stack = vec![node];
            while let Some(n) = import_stack.pop() {
                match n.kind() {
                    "import_specifier" => {
                        let imported_node = n.child_by_field_name("name").unwrap_or(n);
                        let alias_node = n.child_by_field_name("alias").unwrap_or(imported_node);
                        let imported_name = slice(source, &imported_node);
                        let local_name = if let Some(alias) = n.child_by_field_name("alias") {
                            slice(source, &alias)
                        } else {
                            imported_name.clone()
                        };
                        let binding = ImportBinding::new(qualifier.clone(), Some(imported_name.clone()));
                        add_import_binding(
                            path,
                            &alias_node,
                            local_name.clone(),
                            binding,
                            &mut imports,
                            &mut edges,
                        );
                        // Track for two-phase resolution
                        if let Some(ref source_file) = resolved_source {
                            import_binding_infos.push(ImportBindingInfo {
                                local_name,
                                source_file: source_file.clone(),
                                original_name: imported_name,
                            });
                        }
                        continue;
                    }
                    "identifier" => {
                        let name = slice(source, &n);
                        let binding = ImportBinding::new(qualifier.clone(), None);
                        add_import_binding(path, &n, name.clone(), binding, &mut imports, &mut edges);
                        // Default import - local name equals original name
                        if let Some(ref source_file) = resolved_source {
                            import_binding_infos.push(ImportBindingInfo {
                                local_name: name.clone(),
                                source_file: source_file.clone(),
                                original_name: name,
                            });
                        }
                        continue;
                    }
                    "namespace_import" => {
                        if let Some(name_node) = n.child_by_field_name("name") {
                            let name = slice(source, &name_node);
                            let binding = ImportBinding::new(qualifier.clone(), None);
                            add_import_binding(
                                path,
                                &name_node,
                                name,
                                binding,
                                &mut imports,
                                &mut edges,
                            );
                            // Namespace imports are handled specially - they don't map to a single symbol
                        }
                        continue;
                    }
                    _ => {}
                }

                let mut cursor = n.walk();
                for child in n.children(&mut cursor) {
                    import_stack.push(child);
                }
            }
            continue;
        }

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            stack.push(child);
        }
    }

    (imports, edges, dependencies, import_binding_infos)
}

fn add_import_binding(
    path: &Path,
    alias_node: &Node,
    local_name: String,
    binding: ImportBinding,
    imports: &mut HashMap<String, ImportBinding>,
    edges: &mut Vec<EdgeRecord>,
) {
    imports.entry(local_name.clone()).or_insert(binding.clone());
    if binding.qualifier.is_some() {
        edges.push(EdgeRecord {
            src: import_edge_id(path, alias_node),
            dst: binding.symbol_id(&local_name),
            kind: "import".to_string(),
        });
    }
}

fn import_edge_id(path: &Path, node: &Node) -> String {
    format!("{}#import-{}", normalize_path(path), node.start_byte())
}

fn export_edge_id(path: &Path, node: &Node) -> String {
    format!("{}#export-{}", normalize_path(path), node.start_byte())
}

fn import_qualifier(path: &Path, raw: &str) -> String {
    let cleaned = raw.trim().trim_matches('"').trim_matches('\'');
    let mut target = PathBuf::from(cleaned);
    if target.is_relative() {
        if let Some(parent) = path.parent() {
            target = parent.join(target);
        }
    }
    let mut qualifier = normalize_path(&target);
    if let Some(ext) = target.extension().and_then(|e| e.to_str()) {
        let trim = ext.len() + 1;
        if qualifier.len() > trim {
            qualifier.truncate(qualifier.len() - trim);
        }
    }
    qualifier
}

/// Resolve import specifier to actual file path for dependency tracking
fn resolve_import_path(importing_file: &Path, specifier: &str) -> Option<String> {
    let cleaned = specifier.trim().trim_matches('"').trim_matches('\'');

    // Skip non-relative imports (node_modules, etc.)
    if !cleaned.starts_with('.') && !cleaned.starts_with('/') {
        return None;
    }

    let parent = importing_file.parent()?;
    let base_path = parent.join(cleaned);

    // Try common TypeScript extensions
    let extensions = ["", ".ts", ".tsx", "/index.ts", "/index.tsx"];
    for ext in extensions {
        let candidate = if ext.is_empty() {
            base_path.clone()
        } else if ext.starts_with('/') {
            base_path.join(&ext[1..])
        } else {
            PathBuf::from(format!("{}{}", base_path.display(), ext))
        };

        if candidate.exists() {
            if let Ok(canonical) = candidate.canonicalize() {
                return Some(normalize_path(&canonical));
            }
        }
    }

    // Return best-effort normalized path even if file doesn't exist
    Some(normalize_path(&base_path))
}

fn collect_export_edges(
    path: &Path,
    source: &str,
    root: &Node,
    symbol_by_name: &HashMap<String, SymbolBinding>,
    imports: &HashMap<String, ImportBinding>,
) -> Vec<EdgeRecord> {
    let mut edges = Vec::new();
    let mut stack = vec![*root];

    while let Some(node) = stack.pop() {
        if node.kind() == "export_statement" {
            let qualifier_override = node
                .child_by_field_name("source")
                .map(|s| slice(source, &s))
                .map(|raw| import_qualifier(path, &raw));
            let mut produced = false;
            let mut export_stack = vec![node];
            while let Some(n) = export_stack.pop() {
                if n.kind() == "export_specifier" {
                    let name_node = n.child_by_field_name("name").unwrap_or(n);
                    let alias = n
                        .child_by_field_name("alias")
                        .map(|al| slice(source, &al))
                        .unwrap_or_else(|| slice(source, &name_node));
                    let resolved = resolve_name(
                        &slice(source, &name_node),
                        Some((name_node.start_byte(), name_node.end_byte())),
                        path,
                        symbol_by_name,
                        imports,
                        qualifier_override.clone(),
                    );
                    let target_id = resolved.id.clone();
                    edges.push(EdgeRecord {
                        src: export_edge_id(path, &name_node),
                        dst: target_id.clone(),
                        kind: "export".to_string(),
                    });
                    if alias != slice(source, &name_node) {
                        edges.push(EdgeRecord {
                            src: export_edge_id(path, &n),
                            dst: target_id,
                            kind: "export".to_string(),
                        });
                    }
                    produced = true;
                    continue;
                }
                let mut cursor = n.walk();
                for child in n.children(&mut cursor) {
                    export_stack.push(child);
                }
            }

            if !produced {
                if let Some(q) = qualifier_override {
                    edges.push(EdgeRecord {
                        src: export_edge_id(path, &node),
                        dst: format!("{q}::*"),
                        kind: "export".to_string(),
                    });
                }
            }
            continue;
        }

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            stack.push(child);
        }
    }

    edges
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
    symbol_by_name: &HashMap<String, SymbolBinding>,
    imports: &HashMap<String, ImportBinding>,
) -> Vec<ReferenceRecord> {
    let mut refs = Vec::new();
    let mut stack = vec![*root];
    let file = normalize_path(path);

    while let Some(node) = stack.pop() {
        if node.kind() == "identifier" {
            let span = (node.start_byte(), node.end_byte());
            if !declared_spans.contains(&span) {
                let name = slice(source, &node);
                // First try local symbols
                if let Some(sym) = symbol_by_name.get(&name) {
                    refs.push(ReferenceRecord {
                        file: file.clone(),
                        start: node.start_byte() as i64,
                        end: node.end_byte() as i64,
                        symbol_id: sym.id.clone(),
                    });
                } else if let Some(import) = imports.get(&name) {
                    // Cross-file reference via import
                    refs.push(ReferenceRecord {
                        file: file.clone(),
                        start: node.start_byte() as i64,
                        end: node.end_byte() as i64,
                        symbol_id: import.symbol_id(&name),
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
    source: &[u8],
) -> SymbolRecord {
    let qualifier = Some(module_qualifier(path, &container));
    let content_hash = super::compute_content_hash(source, node.start_byte(), node.end_byte());
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
        content_hash,
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

        let (symbols, edges, _refs, _deps, _imports) = index_file(&path, source).unwrap();
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
        assert!(
            edges.iter().any(|e| e.kind == "overrides"),
            "expected method override edge, got {:?}",
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

        let (iface_symbols, _, _, _, _) = index_file(&iface_path, iface_src).unwrap();
        let (_, impl_edges, _, _, _) = index_file(&impl_path, impl_src).unwrap();

        let _base = iface_symbols.iter().find(|s| s.name == "Base").unwrap();
        assert!(
            impl_edges.iter().any(|e| e.kind == "extends"),
            "expected extends edge pointing to Base"
        );
        // We do not resolve cross-file edges yet; just assert we recorded an extends relationship.
    }

    #[test]
    fn records_import_export_edges() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("use.ts");
        let source = r#"
            import { Foo as Renamed } from "./defs";
            export { Renamed as Visible };
            export * from "./defs";
        "#;
        fs::write(&path, source).unwrap();

        let (_symbols, edges, _refs, _deps, _imports) = index_file(&path, source).unwrap();
        let import_edges: Vec<_> = edges
            .iter()
            .filter(|e| e.kind == "import")
            .map(|e| e.dst.clone())
            .collect();
        assert!(
            import_edges.iter().any(|d| d.ends_with("defs::Foo")),
            "expected import edge to defs::Foo, got {:?}",
            import_edges
        );

        let export_edges: Vec<_> = edges.iter().filter(|e| e.kind == "export").collect();
        assert!(
            !export_edges.is_empty(),
            "expected export edges for re-exports"
        );
    }
}
