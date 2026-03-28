use crate::languages::{slice, ImportBindingInfo};
use crate::store::{normalize_path, EdgeRecord, FileDependency, ReferenceRecord, SymbolRecord};
use anyhow::{Context, Result};
use once_cell::sync::Lazy;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use tree_sitter::{Language, Node, Parser, TreeCursor};

static CSHARP_LANGUAGE: Lazy<Language> = Lazy::new(|| tree_sitter_c_sharp::LANGUAGE.into());

/// Index a C# file, returning symbols, edges, references, file dependencies, and import bindings.
#[allow(clippy::type_complexity)]
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
        .set_language(&CSHARP_LANGUAGE)
        .context("failed to set C# language")?;
    let tree = parser
        .parse(source, None)
        .context("failed to parse C# file")?;

    let mut symbols = Vec::new();
    let mut edges = Vec::new();
    let mut declared_spans: HashSet<(usize, usize)> = HashSet::new();
    let mut symbol_by_name: HashMap<String, String> = HashMap::new();

    let is_test_file = is_test_path(path);

    // Detect file-scoped namespace
    let file_namespace = detect_file_scoped_namespace(source, &tree.root_node());

    {
        let mut cursor = tree.walk();
        walk_symbols(
            path,
            source,
            &mut cursor,
            None, // container
            &file_namespace,
            &mut symbols,
            &mut edges,
            &mut declared_spans,
            &mut symbol_by_name,
            is_test_file,
        );
    }

    let references = collect_references(
        path,
        source,
        &tree.root_node(),
        &declared_spans,
        &symbol_by_name,
    );

    let (dependencies, import_bindings) = collect_imports(path, source, &tree.root_node());

    Ok((symbols, edges, references, dependencies, import_bindings))
}

/// Check if a file path looks like a test file.
fn is_test_path(path: &Path) -> bool {
    let path_str = path.to_string_lossy();
    path_str.contains("/Tests/")
        || path_str.contains("/Test/")
        || path_str.ends_with("Tests.cs")
        || path_str.ends_with("Test.cs")
}

/// Detect a file-scoped namespace declaration and return its name.
fn detect_file_scoped_namespace(source: &str, root: &Node) -> Option<String> {
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        if child.kind() == "file_scoped_namespace_declaration" {
            return extract_namespace_name(source, &child);
        }
    }
    None
}

/// Extract the namespace name from a namespace declaration node.
fn extract_namespace_name(source: &str, node: &Node) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "identifier" => return Some(slice(source, &child)),
            "qualified_name" => return Some(slice(source, &child)),
            _ => {}
        }
    }
    None
}

/// Walk the AST and extract symbols.
#[allow(clippy::too_many_arguments)]
fn walk_symbols(
    path: &Path,
    source: &str,
    cursor: &mut TreeCursor,
    container: Option<String>,
    current_namespace: &Option<String>,
    symbols: &mut Vec<SymbolRecord>,
    edges: &mut Vec<EdgeRecord>,
    declared_spans: &mut HashSet<(usize, usize)>,
    symbol_by_name: &mut HashMap<String, String>,
    is_test_file: bool,
) {
    loop {
        let node = cursor.node();
        match node.kind() {
            "class_declaration" => {
                handle_class(
                    path,
                    source,
                    &node,
                    container.clone(),
                    current_namespace,
                    symbols,
                    edges,
                    declared_spans,
                    symbol_by_name,
                    is_test_file,
                );
            }
            "struct_declaration" => {
                handle_struct(
                    path,
                    source,
                    &node,
                    container.clone(),
                    current_namespace,
                    symbols,
                    edges,
                    declared_spans,
                    symbol_by_name,
                    is_test_file,
                );
            }
            "record_declaration" => {
                handle_record(
                    path,
                    source,
                    &node,
                    container.clone(),
                    current_namespace,
                    symbols,
                    edges,
                    declared_spans,
                    symbol_by_name,
                    is_test_file,
                );
            }
            "interface_declaration" => {
                handle_interface(
                    path,
                    source,
                    &node,
                    container.clone(),
                    current_namespace,
                    symbols,
                    edges,
                    declared_spans,
                    symbol_by_name,
                    is_test_file,
                );
            }
            "enum_declaration" => {
                handle_enum(
                    path,
                    source,
                    &node,
                    container.clone(),
                    current_namespace,
                    symbols,
                    declared_spans,
                    symbol_by_name,
                    is_test_file,
                );
            }
            "method_declaration" => {
                handle_method(
                    path,
                    source,
                    &node,
                    container.clone(),
                    current_namespace,
                    symbols,
                    declared_spans,
                    symbol_by_name,
                    is_test_file,
                );
            }
            "constructor_declaration" => {
                handle_constructor(
                    path,
                    source,
                    &node,
                    container.clone(),
                    current_namespace,
                    symbols,
                    declared_spans,
                    symbol_by_name,
                    is_test_file,
                );
            }
            "property_declaration" => {
                handle_property(
                    path,
                    source,
                    &node,
                    container.clone(),
                    current_namespace,
                    symbols,
                    declared_spans,
                    symbol_by_name,
                    is_test_file,
                );
            }
            "field_declaration" => {
                handle_field(
                    path,
                    source,
                    &node,
                    container.clone(),
                    current_namespace,
                    symbols,
                    declared_spans,
                    symbol_by_name,
                    is_test_file,
                );
            }
            "namespace_declaration" => {
                handle_namespace(
                    path,
                    source,
                    &node,
                    container.clone(),
                    symbols,
                    edges,
                    declared_spans,
                    symbol_by_name,
                    is_test_file,
                );
            }
            "file_scoped_namespace_declaration" => {
                handle_file_scoped_namespace(
                    path,
                    source,
                    &node,
                    symbols,
                    declared_spans,
                    symbol_by_name,
                    is_test_file,
                );
            }
            _ => {}
        }

        // Recurse into children, but skip type/namespace bodies since
        // they handle their own body traversal
        if !matches!(
            node.kind(),
            "class_declaration"
                | "struct_declaration"
                | "record_declaration"
                | "interface_declaration"
                | "enum_declaration"
                | "namespace_declaration"
                | "file_scoped_namespace_declaration"
        ) && cursor.goto_first_child()
        {
            walk_symbols(
                path,
                source,
                cursor,
                container.clone(),
                current_namespace,
                symbols,
                edges,
                declared_spans,
                symbol_by_name,
                is_test_file,
            );
            cursor.goto_parent();
        }

        if !cursor.goto_next_sibling() {
            break;
        }
    }
}

/// Extract modifiers from a declaration node.
/// Returns (visibility, is_const, is_static).
fn extract_modifiers(source: &str, node: &Node) -> (Option<String>, bool, bool) {
    let mut visibility = None;
    let mut is_const = false;
    let mut is_static = false;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "modifier" {
            let mut mc = child.walk();
            for m in child.children(&mut mc) {
                match m.kind() {
                    "public" | "private" | "protected" | "internal" => {
                        visibility = Some(slice(source, &m));
                    }
                    "const" => {
                        is_const = true;
                    }
                    "static" => {
                        is_static = true;
                    }
                    _ => {}
                }
            }
        }
    }
    (visibility, is_const, is_static)
}

/// Extract the identifier name from a declaration node.
fn extract_name(source: &str, node: &Node) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "identifier" {
            let name = slice(source, &child);
            // Strip generic type parameters
            return Some(strip_generics(&name));
        }
    }
    None
}

/// Strip generic type parameters from a name (e.g., `List<T>` -> `List`).
fn strip_generics(name: &str) -> String {
    if let Some(idx) = name.find('<') {
        name[..idx].to_string()
    } else {
        name.to_string()
    }
}

/// Build qualifier from namespace and container.
fn build_qualifier(namespace: &Option<String>, container: &Option<String>) -> Option<String> {
    match (namespace, container) {
        (Some(ns), Some(c)) => Some(format!("{}.{}", ns, c)),
        (Some(ns), None) => Some(ns.clone()),
        (None, Some(c)) => Some(c.clone()),
        (None, None) => None,
    }
}

/// Check if a method has test attributes.
fn has_test_attribute(source: &str, node: &Node) -> bool {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "attribute_list" {
            let mut ac = child.walk();
            for attr_child in child.children(&mut ac) {
                if attr_child.kind() == "attribute" {
                    let attr_name = slice(source, &attr_child);
                    if matches!(
                        attr_name.as_str(),
                        "Test" | "Fact" | "Theory" | "TestMethod"
                    ) {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// Extract base types from a base_list node.
/// Returns (extends, implements) based on naming convention:
/// - Types starting with I + uppercase are interfaces
/// - First non-interface type is the base class
fn extract_base_types(source: &str, node: &Node) -> (Option<String>, Vec<String>) {
    let mut base_class = None;
    let mut interfaces = Vec::new();

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "identifier" || child.kind() == "qualified_name" {
            let name = strip_generics(&slice(source, &child));
            if looks_like_interface(&name) {
                interfaces.push(name);
            } else if base_class.is_none() {
                base_class = Some(name);
            } else {
                // Subsequent non-interface types treated as interfaces
                interfaces.push(name);
            }
        } else if child.kind() == "generic_name" {
            // Handle generic base types like BaseClass<T>
            if let Some(name_node) = child.child(0) {
                let name = slice(source, &name_node);
                if looks_like_interface(&name) {
                    interfaces.push(name);
                } else if base_class.is_none() {
                    base_class = Some(name);
                } else {
                    interfaces.push(name);
                }
            }
        }
    }

    (base_class, interfaces)
}

/// Check if a type name looks like an interface (starts with I followed by uppercase).
fn looks_like_interface(name: &str) -> bool {
    let mut chars = name.chars();
    if let Some('I') = chars.next() {
        if let Some(c) = chars.next() {
            return c.is_uppercase();
        }
    }
    false
}

/// Handle class declarations.
#[allow(clippy::too_many_arguments)]
fn handle_class(
    path: &Path,
    source: &str,
    node: &Node,
    container: Option<String>,
    current_namespace: &Option<String>,
    symbols: &mut Vec<SymbolRecord>,
    edges: &mut Vec<EdgeRecord>,
    declared_spans: &mut HashSet<(usize, usize)>,
    symbol_by_name: &mut HashMap<String, String>,
    is_test_file: bool,
) {
    let span = (node.start_byte(), node.end_byte());
    if declared_spans.contains(&span) {
        return;
    }

    let name = match extract_name(source, node) {
        Some(n) if !n.is_empty() => n,
        _ => return,
    };

    let (visibility, _, _) = extract_modifiers(source, node);
    let default_vis = if container.is_some() {
        "private"
    } else {
        "internal"
    };
    let vis = visibility.unwrap_or_else(|| default_vis.to_string());

    let qualifier = build_qualifier(current_namespace, &container);

    let sym = make_symbol(
        path,
        node,
        &name,
        "class",
        qualifier,
        container.clone(),
        source.as_bytes(),
        is_test_file,
        Some(vis),
    );
    declared_spans.insert(span);
    symbol_by_name.insert(name.clone(), sym.id.clone());

    // Handle base types
    handle_base_list(path, source, node, &sym.id, edges, symbol_by_name);

    symbols.push(sym);

    // Walk body
    walk_type_body(
        path,
        source,
        node,
        &name,
        current_namespace,
        symbols,
        edges,
        declared_spans,
        symbol_by_name,
        is_test_file,
    );
}

/// Handle struct declarations.
#[allow(clippy::too_many_arguments)]
fn handle_struct(
    path: &Path,
    source: &str,
    node: &Node,
    container: Option<String>,
    current_namespace: &Option<String>,
    symbols: &mut Vec<SymbolRecord>,
    edges: &mut Vec<EdgeRecord>,
    declared_spans: &mut HashSet<(usize, usize)>,
    symbol_by_name: &mut HashMap<String, String>,
    is_test_file: bool,
) {
    let span = (node.start_byte(), node.end_byte());
    if declared_spans.contains(&span) {
        return;
    }

    let name = match extract_name(source, node) {
        Some(n) if !n.is_empty() => n,
        _ => return,
    };

    let (visibility, _, _) = extract_modifiers(source, node);
    let default_vis = if container.is_some() {
        "private"
    } else {
        "internal"
    };
    let vis = visibility.unwrap_or_else(|| default_vis.to_string());

    let qualifier = build_qualifier(current_namespace, &container);

    let sym = make_symbol(
        path,
        node,
        &name,
        "struct",
        qualifier,
        container.clone(),
        source.as_bytes(),
        is_test_file,
        Some(vis),
    );
    declared_spans.insert(span);
    symbol_by_name.insert(name.clone(), sym.id.clone());

    handle_base_list(path, source, node, &sym.id, edges, symbol_by_name);

    symbols.push(sym);

    walk_type_body(
        path,
        source,
        node,
        &name,
        current_namespace,
        symbols,
        edges,
        declared_spans,
        symbol_by_name,
        is_test_file,
    );
}

/// Handle record declarations (both `record` and `record struct`).
#[allow(clippy::too_many_arguments)]
fn handle_record(
    path: &Path,
    source: &str,
    node: &Node,
    container: Option<String>,
    current_namespace: &Option<String>,
    symbols: &mut Vec<SymbolRecord>,
    edges: &mut Vec<EdgeRecord>,
    declared_spans: &mut HashSet<(usize, usize)>,
    symbol_by_name: &mut HashMap<String, String>,
    is_test_file: bool,
) {
    let span = (node.start_byte(), node.end_byte());
    if declared_spans.contains(&span) {
        return;
    }

    let name = match extract_name(source, node) {
        Some(n) if !n.is_empty() => n,
        _ => return,
    };

    // Determine if it's a record struct
    let is_record_struct = {
        let mut cursor = node.walk();
        let result = node
            .children(&mut cursor)
            .any(|child| child.kind() == "struct");
        result
    };

    let kind = if is_record_struct { "struct" } else { "class" };

    let (visibility, _, _) = extract_modifiers(source, node);
    let default_vis = if container.is_some() {
        "private"
    } else {
        "internal"
    };
    let vis = visibility.unwrap_or_else(|| default_vis.to_string());

    let qualifier = build_qualifier(current_namespace, &container);

    let sym = make_symbol(
        path,
        node,
        &name,
        kind,
        qualifier,
        container.clone(),
        source.as_bytes(),
        is_test_file,
        Some(vis),
    );
    declared_spans.insert(span);
    symbol_by_name.insert(name.clone(), sym.id.clone());

    handle_base_list(path, source, node, &sym.id, edges, symbol_by_name);

    symbols.push(sym);

    walk_type_body(
        path,
        source,
        node,
        &name,
        current_namespace,
        symbols,
        edges,
        declared_spans,
        symbol_by_name,
        is_test_file,
    );
}

/// Handle interface declarations.
#[allow(clippy::too_many_arguments)]
fn handle_interface(
    path: &Path,
    source: &str,
    node: &Node,
    container: Option<String>,
    current_namespace: &Option<String>,
    symbols: &mut Vec<SymbolRecord>,
    edges: &mut Vec<EdgeRecord>,
    declared_spans: &mut HashSet<(usize, usize)>,
    symbol_by_name: &mut HashMap<String, String>,
    is_test_file: bool,
) {
    let span = (node.start_byte(), node.end_byte());
    if declared_spans.contains(&span) {
        return;
    }

    let name = match extract_name(source, node) {
        Some(n) if !n.is_empty() => n,
        _ => return,
    };

    let (visibility, _, _) = extract_modifiers(source, node);
    let default_vis = if container.is_some() {
        "private"
    } else {
        "internal"
    };
    let vis = visibility.unwrap_or_else(|| default_vis.to_string());

    let qualifier = build_qualifier(current_namespace, &container);

    let sym = make_symbol(
        path,
        node,
        &name,
        "interface",
        qualifier,
        container.clone(),
        source.as_bytes(),
        is_test_file,
        Some(vis),
    );
    declared_spans.insert(span);
    symbol_by_name.insert(name.clone(), sym.id.clone());

    // For interfaces, all base types in base_list are implements (interface inheritance)
    handle_interface_base_list(path, source, node, &sym.id, edges, symbol_by_name);

    symbols.push(sym);

    walk_type_body(
        path,
        source,
        node,
        &name,
        current_namespace,
        symbols,
        edges,
        declared_spans,
        symbol_by_name,
        is_test_file,
    );
}

/// Handle enum declarations.
#[allow(clippy::too_many_arguments)]
fn handle_enum(
    path: &Path,
    source: &str,
    node: &Node,
    container: Option<String>,
    current_namespace: &Option<String>,
    symbols: &mut Vec<SymbolRecord>,
    declared_spans: &mut HashSet<(usize, usize)>,
    symbol_by_name: &mut HashMap<String, String>,
    is_test_file: bool,
) {
    let span = (node.start_byte(), node.end_byte());
    if declared_spans.contains(&span) {
        return;
    }

    let name = match extract_name(source, node) {
        Some(n) if !n.is_empty() => n,
        _ => return,
    };

    let (visibility, _, _) = extract_modifiers(source, node);
    let default_vis = if container.is_some() {
        "private"
    } else {
        "internal"
    };
    let vis = visibility.unwrap_or_else(|| default_vis.to_string());

    let qualifier = build_qualifier(current_namespace, &container);

    let sym = make_symbol(
        path,
        node,
        &name,
        "enum",
        qualifier,
        container,
        source.as_bytes(),
        is_test_file,
        Some(vis),
    );
    declared_spans.insert(span);
    symbol_by_name.insert(name, sym.id.clone());
    symbols.push(sym);
}

/// Handle method declarations.
#[allow(clippy::too_many_arguments)]
fn handle_method(
    path: &Path,
    source: &str,
    node: &Node,
    container: Option<String>,
    current_namespace: &Option<String>,
    symbols: &mut Vec<SymbolRecord>,
    declared_spans: &mut HashSet<(usize, usize)>,
    symbol_by_name: &mut HashMap<String, String>,
    is_test_file: bool,
) {
    let span = (node.start_byte(), node.end_byte());
    if declared_spans.contains(&span) {
        return;
    }

    let name = match extract_name(source, node) {
        Some(n) if !n.is_empty() => n,
        _ => return,
    };

    let kind = if container.is_some() {
        "method"
    } else {
        "function"
    };

    let (visibility, _, _) = extract_modifiers(source, node);
    let vis = if container.is_some() {
        Some(visibility.unwrap_or_else(|| "private".to_string()))
    } else {
        visibility
    };

    let is_test = is_test_file || has_test_attribute(source, node);

    let qualifier = build_qualifier(current_namespace, &container);

    let sym = make_symbol(
        path,
        node,
        &name,
        kind,
        qualifier,
        container,
        source.as_bytes(),
        is_test,
        vis,
    );
    declared_spans.insert(span);
    symbol_by_name.insert(name, sym.id.clone());
    symbols.push(sym);
}

/// Handle constructor declarations.
#[allow(clippy::too_many_arguments)]
fn handle_constructor(
    path: &Path,
    source: &str,
    node: &Node,
    container: Option<String>,
    current_namespace: &Option<String>,
    symbols: &mut Vec<SymbolRecord>,
    declared_spans: &mut HashSet<(usize, usize)>,
    symbol_by_name: &mut HashMap<String, String>,
    is_test_file: bool,
) {
    let span = (node.start_byte(), node.end_byte());
    if declared_spans.contains(&span) {
        return;
    }

    let name = match extract_name(source, node) {
        Some(n) if !n.is_empty() => n,
        _ => return,
    };

    let kind = if container.is_some() {
        "method"
    } else {
        "function"
    };

    let (visibility, _, _) = extract_modifiers(source, node);
    let vis = if container.is_some() {
        Some(visibility.unwrap_or_else(|| "private".to_string()))
    } else {
        visibility
    };

    let qualifier = build_qualifier(current_namespace, &container);

    let sym = make_symbol(
        path,
        node,
        &name,
        kind,
        qualifier,
        container,
        source.as_bytes(),
        is_test_file,
        vis,
    );
    declared_spans.insert(span);
    symbol_by_name.insert(name, sym.id.clone());
    symbols.push(sym);
}

/// Handle property declarations.
#[allow(clippy::too_many_arguments)]
fn handle_property(
    path: &Path,
    source: &str,
    node: &Node,
    container: Option<String>,
    current_namespace: &Option<String>,
    symbols: &mut Vec<SymbolRecord>,
    declared_spans: &mut HashSet<(usize, usize)>,
    symbol_by_name: &mut HashMap<String, String>,
    is_test_file: bool,
) {
    let span = (node.start_byte(), node.end_byte());
    if declared_spans.contains(&span) {
        return;
    }

    let name = match extract_name(source, node) {
        Some(n) if !n.is_empty() => n,
        _ => return,
    };

    let (visibility, _, _) = extract_modifiers(source, node);
    let vis = if container.is_some() {
        Some(visibility.unwrap_or_else(|| "private".to_string()))
    } else {
        visibility
    };

    let qualifier = build_qualifier(current_namespace, &container);

    let sym = make_symbol(
        path,
        node,
        &name,
        "property",
        qualifier,
        container,
        source.as_bytes(),
        is_test_file,
        vis,
    );
    declared_spans.insert(span);
    symbol_by_name.insert(name, sym.id.clone());
    symbols.push(sym);
}

/// Handle field declarations.
#[allow(clippy::too_many_arguments)]
fn handle_field(
    path: &Path,
    source: &str,
    node: &Node,
    container: Option<String>,
    current_namespace: &Option<String>,
    symbols: &mut Vec<SymbolRecord>,
    declared_spans: &mut HashSet<(usize, usize)>,
    symbol_by_name: &mut HashMap<String, String>,
    is_test_file: bool,
) {
    let span = (node.start_byte(), node.end_byte());
    if declared_spans.contains(&span) {
        return;
    }

    let (visibility, is_const, _) = extract_modifiers(source, node);
    let vis = if container.is_some() {
        Some(visibility.unwrap_or_else(|| "private".to_string()))
    } else {
        visibility
    };

    let kind = if is_const { "const" } else { "field" };

    // Extract field name from variable_declaration > variable_declarator > identifier
    let name = extract_field_name(source, node);
    let name = match name {
        Some(n) if !n.is_empty() => n,
        _ => return,
    };

    let qualifier = build_qualifier(current_namespace, &container);

    let sym = make_symbol(
        path,
        node,
        &name,
        kind,
        qualifier,
        container,
        source.as_bytes(),
        is_test_file,
        vis,
    );
    declared_spans.insert(span);
    symbol_by_name.insert(name, sym.id.clone());
    symbols.push(sym);
}

/// Extract field name from a field_declaration node.
fn extract_field_name(source: &str, node: &Node) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "variable_declaration" {
            let mut vc = child.walk();
            for vchild in child.children(&mut vc) {
                if vchild.kind() == "variable_declarator" {
                    let mut dc = vchild.walk();
                    for dchild in vchild.children(&mut dc) {
                        if dchild.kind() == "identifier" {
                            return Some(slice(source, &dchild));
                        }
                    }
                }
            }
        }
    }
    None
}

/// Handle block-scoped namespace declarations.
#[allow(clippy::too_many_arguments)]
fn handle_namespace(
    path: &Path,
    source: &str,
    node: &Node,
    container: Option<String>,
    symbols: &mut Vec<SymbolRecord>,
    edges: &mut Vec<EdgeRecord>,
    declared_spans: &mut HashSet<(usize, usize)>,
    symbol_by_name: &mut HashMap<String, String>,
    is_test_file: bool,
) {
    let span = (node.start_byte(), node.end_byte());
    if declared_spans.contains(&span) {
        return;
    }

    let ns_name = match extract_namespace_name(source, node) {
        Some(n) if !n.is_empty() => n,
        _ => return,
    };

    let qualifier = build_qualifier(&None, &container);

    let sym = make_symbol(
        path,
        node,
        &ns_name,
        "namespace",
        qualifier,
        container,
        source.as_bytes(),
        is_test_file,
        Some("public".to_string()),
    );
    declared_spans.insert(span);
    symbol_by_name.insert(ns_name.clone(), sym.id.clone());
    symbols.push(sym);

    // Walk the namespace body with the namespace as the current namespace context
    let ns_option = Some(ns_name);
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "declaration_list" {
            let mut body_cursor = child.walk();
            walk_symbols(
                path,
                source,
                &mut body_cursor,
                None, // types inside namespace don't have a container
                &ns_option,
                symbols,
                edges,
                declared_spans,
                symbol_by_name,
                is_test_file,
            );
        }
    }
}

/// Handle file-scoped namespace declarations.
#[allow(clippy::too_many_arguments)]
fn handle_file_scoped_namespace(
    path: &Path,
    source: &str,
    node: &Node,
    symbols: &mut Vec<SymbolRecord>,
    declared_spans: &mut HashSet<(usize, usize)>,
    symbol_by_name: &mut HashMap<String, String>,
    is_test_file: bool,
) {
    let span = (node.start_byte(), node.end_byte());
    if declared_spans.contains(&span) {
        return;
    }

    let ns_name = match extract_namespace_name(source, node) {
        Some(n) if !n.is_empty() => n,
        _ => return,
    };

    let sym = make_symbol(
        path,
        node,
        &ns_name,
        "namespace",
        None,
        None,
        source.as_bytes(),
        is_test_file,
        Some("public".to_string()),
    );
    declared_spans.insert(span);
    symbol_by_name.insert(ns_name, sym.id.clone());
    symbols.push(sym);
}

/// Walk the body (declaration_list) of a type declaration.
#[allow(clippy::too_many_arguments)]
fn walk_type_body(
    path: &Path,
    source: &str,
    node: &Node,
    type_name: &str,
    current_namespace: &Option<String>,
    symbols: &mut Vec<SymbolRecord>,
    edges: &mut Vec<EdgeRecord>,
    declared_spans: &mut HashSet<(usize, usize)>,
    symbol_by_name: &mut HashMap<String, String>,
    is_test_file: bool,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "declaration_list" {
            let mut body_cursor = child.walk();
            walk_symbols(
                path,
                source,
                &mut body_cursor,
                Some(type_name.to_string()),
                current_namespace,
                symbols,
                edges,
                declared_spans,
                symbol_by_name,
                is_test_file,
            );
        }
    }
}

/// Handle base_list for class/struct/record - applies extends/implements heuristic.
fn handle_base_list(
    path: &Path,
    source: &str,
    node: &Node,
    src_id: &str,
    edges: &mut Vec<EdgeRecord>,
    symbol_by_name: &HashMap<String, String>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "base_list" {
            let (base_class, interfaces) = extract_base_types(source, &child);
            if let Some(base) = base_class {
                let dst_id = symbol_by_name
                    .get(&base)
                    .cloned()
                    .unwrap_or_else(|| format!("{}#{}", normalize_path(path), base));
                edges.push(EdgeRecord {
                    src: src_id.to_string(),
                    dst: dst_id,
                    kind: "extends".to_string(),
                });
            }
            for iface in interfaces {
                let dst_id = symbol_by_name
                    .get(&iface)
                    .cloned()
                    .unwrap_or_else(|| format!("{}#{}", normalize_path(path), iface));
                edges.push(EdgeRecord {
                    src: src_id.to_string(),
                    dst: dst_id,
                    kind: "implements".to_string(),
                });
            }
        }
    }
}

/// Handle base_list for interfaces - all base types are implements edges.
fn handle_interface_base_list(
    path: &Path,
    source: &str,
    node: &Node,
    src_id: &str,
    edges: &mut Vec<EdgeRecord>,
    symbol_by_name: &HashMap<String, String>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "base_list" {
            let mut bc = child.walk();
            for base_child in child.children(&mut bc) {
                if base_child.kind() == "identifier" || base_child.kind() == "qualified_name" {
                    let name = strip_generics(&slice(source, &base_child));
                    if !name.is_empty() {
                        let dst_id = symbol_by_name
                            .get(&name)
                            .cloned()
                            .unwrap_or_else(|| format!("{}#{}", normalize_path(path), name));
                        edges.push(EdgeRecord {
                            src: src_id.to_string(),
                            dst: dst_id,
                            kind: "implements".to_string(),
                        });
                    }
                }
            }
        }
    }
}

/// Collect references to symbols.
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

/// Collect using directives as file dependencies and import bindings.
fn collect_imports(
    path: &Path,
    source: &str,
    root: &Node,
) -> (Vec<FileDependency>, Vec<ImportBindingInfo>) {
    let mut dependencies = Vec::new();
    let mut import_bindings = Vec::new();
    let from_file = normalize_path(path);

    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        if child.kind() == "using_directive" {
            let import_text = slice(source, &child);

            // Extract the namespace name
            let mut nc = child.walk();
            for uchild in child.children(&mut nc) {
                if uchild.kind() == "identifier" || uchild.kind() == "qualified_name" {
                    let ns = slice(source, &uchild);
                    if !ns.is_empty() {
                        dependencies.push(FileDependency {
                            from_file: from_file.clone(),
                            to_file: ns.clone(),
                            kind: "using".to_string(),
                        });

                        // Local name is the last dotted component
                        let local_name = ns.rsplit('.').next().unwrap_or(&ns).to_string();

                        import_bindings.push(ImportBindingInfo {
                            local_name,
                            source_file: from_file.clone(),
                            original_name: ns,
                            import_text: import_text.clone(),
                        });
                    }
                }
            }
        }
    }

    (dependencies, import_bindings)
}

#[allow(clippy::too_many_arguments)]
fn make_symbol(
    path: &Path,
    node: &Node,
    name: &str,
    kind: &str,
    qualifier: Option<String>,
    container: Option<String>,
    source: &[u8],
    is_test: bool,
    visibility: Option<String>,
) -> SymbolRecord {
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
        visibility,
        container,
        content_hash,
        is_test,
    }
}

// ============================================================================
// LanguageParser trait implementation
// ============================================================================

use super::traits::{LanguageConfig, LanguageParser, ParseResult};

/// C# language parser implementing the `LanguageParser` trait.
#[derive(Clone)]
pub struct CSharpParser;

impl CSharpParser {
    /// Create a new C# parser.
    pub fn new() -> Self {
        Self
    }
}

impl Default for CSharpParser {
    fn default() -> Self {
        Self::new()
    }
}

impl LanguageParser for CSharpParser {
    fn config(&self) -> LanguageConfig {
        LanguageConfig {
            name: "C#",
            extensions: &["cs"],
        }
    }

    fn language(&self) -> &Language {
        &CSHARP_LANGUAGE
    }

    fn parse(&self, path: &Path, source: &str) -> Result<ParseResult> {
        let tuple = index_file(path, source)?;
        Ok(ParseResult::from_tuple(tuple))
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn extracts_class_with_visibility_and_inheritance() {
        let source = r#"
public class Animal
{
    public void Speak() { }
}

public class Dog : Animal
{
    public void Speak() { }
}
"#;
        let path = PathBuf::from("test.cs");
        let (symbols, edges, _, _, _) = index_file(&path, source).unwrap();

        let animal = symbols.iter().find(|s| s.name == "Animal").unwrap();
        assert_eq!(animal.kind, "class");
        assert_eq!(animal.visibility, Some("public".to_string()));

        let dog = symbols.iter().find(|s| s.name == "Dog").unwrap();
        assert_eq!(dog.kind, "class");

        // Dog extends Animal
        let extends_edges: Vec<_> = edges.iter().filter(|e| e.kind == "extends").collect();
        assert_eq!(extends_edges.len(), 1);
        assert_eq!(extends_edges[0].src, dog.id);
    }

    #[test]
    fn extracts_struct() {
        let source = r#"
public struct Point
{
    public int X;
    public int Y;
}
"#;
        let path = PathBuf::from("test.cs");
        let (symbols, _, _, _, _) = index_file(&path, source).unwrap();

        let point = symbols.iter().find(|s| s.name == "Point").unwrap();
        assert_eq!(point.kind, "struct");
        assert_eq!(point.visibility, Some("public".to_string()));

        let x = symbols.iter().find(|s| s.name == "X").unwrap();
        assert_eq!(x.kind, "field");

        let y = symbols.iter().find(|s| s.name == "Y").unwrap();
        assert_eq!(y.kind, "field");
    }

    #[test]
    fn extracts_record_and_record_struct() {
        let source = r#"
public record Person(string Name, int Age);
public record struct Coord(int X, int Y);
"#;
        let path = PathBuf::from("test.cs");
        let (symbols, _, _, _, _) = index_file(&path, source).unwrap();

        let person = symbols.iter().find(|s| s.name == "Person").unwrap();
        assert_eq!(person.kind, "class"); // record -> class kind

        let coord = symbols.iter().find(|s| s.name == "Coord").unwrap();
        assert_eq!(coord.kind, "struct"); // record struct -> struct kind
    }

    #[test]
    fn extracts_interface_with_inheritance() {
        let source = r#"
public interface IParent
{
    void DoStuff();
}

public interface IChild : IParent
{
    void DoMore();
}
"#;
        let path = PathBuf::from("test.cs");
        let (symbols, edges, _, _, _) = index_file(&path, source).unwrap();

        let parent = symbols.iter().find(|s| s.name == "IParent").unwrap();
        assert_eq!(parent.kind, "interface");

        let child = symbols.iter().find(|s| s.name == "IChild").unwrap();
        assert_eq!(child.kind, "interface");

        // IChild implements IParent
        let impl_edges: Vec<_> = edges.iter().filter(|e| e.kind == "implements").collect();
        assert_eq!(impl_edges.len(), 1);
        assert_eq!(impl_edges[0].src, child.id);
    }

    #[test]
    fn extracts_enum() {
        let source = r#"
public enum Color
{
    Red,
    Green,
    Blue
}
"#;
        let path = PathBuf::from("test.cs");
        let (symbols, _, _, _, _) = index_file(&path, source).unwrap();

        let color = symbols.iter().find(|s| s.name == "Color").unwrap();
        assert_eq!(color.kind, "enum");
        assert_eq!(color.visibility, Some("public".to_string()));
    }

    #[test]
    fn extracts_methods_and_constructors() {
        let source = r#"
public class Calculator
{
    public Calculator() { }

    public int Add(int a, int b) { return a + b; }

    private int Subtract(int a, int b) { return a - b; }
}
"#;
        let path = PathBuf::from("test.cs");
        let (symbols, _, _, _, _) = index_file(&path, source).unwrap();

        let ctor = symbols
            .iter()
            .find(|s| s.name == "Calculator" && s.kind == "method")
            .unwrap();
        assert_eq!(ctor.kind, "method");
        assert_eq!(ctor.container, Some("Calculator".to_string()));
        assert_eq!(ctor.visibility, Some("public".to_string()));

        let add = symbols.iter().find(|s| s.name == "Add").unwrap();
        assert_eq!(add.kind, "method");
        assert_eq!(add.visibility, Some("public".to_string()));

        let sub = symbols.iter().find(|s| s.name == "Subtract").unwrap();
        assert_eq!(sub.kind, "method");
        assert_eq!(sub.visibility, Some("private".to_string()));
    }

    #[test]
    fn extracts_properties() {
        let source = r#"
public class User
{
    public string Name { get; set; }
    private int Age { get; set; }
}
"#;
        let path = PathBuf::from("test.cs");
        let (symbols, _, _, _, _) = index_file(&path, source).unwrap();

        let name = symbols.iter().find(|s| s.name == "Name").unwrap();
        assert_eq!(name.kind, "property");
        assert_eq!(name.visibility, Some("public".to_string()));

        let age = symbols.iter().find(|s| s.name == "Age").unwrap();
        assert_eq!(age.kind, "property");
        assert_eq!(age.visibility, Some("private".to_string()));
    }

    #[test]
    fn extracts_fields_and_consts() {
        let source = r#"
public class Config
{
    private string _name;
    public const int MaxAge = 150;
    public int Count;
}
"#;
        let path = PathBuf::from("test.cs");
        let (symbols, _, _, _, _) = index_file(&path, source).unwrap();

        let name_field = symbols.iter().find(|s| s.name == "_name").unwrap();
        assert_eq!(name_field.kind, "field");
        assert_eq!(name_field.visibility, Some("private".to_string()));

        let max_age = symbols.iter().find(|s| s.name == "MaxAge").unwrap();
        assert_eq!(max_age.kind, "const");
        assert_eq!(max_age.visibility, Some("public".to_string()));

        let count = symbols.iter().find(|s| s.name == "Count").unwrap();
        assert_eq!(count.kind, "field");
        assert_eq!(count.visibility, Some("public".to_string()));
    }

    #[test]
    fn handles_block_scoped_namespace() {
        let source = r#"
namespace MyApp.Models
{
    public class User { }
}
"#;
        let path = PathBuf::from("test.cs");
        let (symbols, _, _, _, _) = index_file(&path, source).unwrap();

        let ns = symbols.iter().find(|s| s.name == "MyApp.Models").unwrap();
        assert_eq!(ns.kind, "namespace");

        let user = symbols.iter().find(|s| s.name == "User").unwrap();
        assert_eq!(user.kind, "class");
        assert_eq!(user.qualifier, Some("MyApp.Models".to_string()));
    }

    #[test]
    fn handles_file_scoped_namespace() {
        let source = r#"namespace MyApp.Models;

public class User { }
"#;
        let path = PathBuf::from("test.cs");
        let (symbols, _, _, _, _) = index_file(&path, source).unwrap();

        let ns = symbols.iter().find(|s| s.name == "MyApp.Models").unwrap();
        assert_eq!(ns.kind, "namespace");

        let user = symbols.iter().find(|s| s.name == "User").unwrap();
        assert_eq!(user.kind, "class");
        assert_eq!(user.qualifier, Some("MyApp.Models".to_string()));
    }

    #[test]
    fn handles_nested_types() {
        let source = r#"
public class Outer
{
    public class Inner
    {
        public void InnerMethod() { }
    }
}
"#;
        let path = PathBuf::from("test.cs");
        let (symbols, _, _, _, _) = index_file(&path, source).unwrap();

        let outer = symbols.iter().find(|s| s.name == "Outer").unwrap();
        assert_eq!(outer.kind, "class");

        let inner = symbols.iter().find(|s| s.name == "Inner").unwrap();
        assert_eq!(inner.kind, "class");
        assert_eq!(inner.container, Some("Outer".to_string()));

        let method = symbols.iter().find(|s| s.name == "InnerMethod").unwrap();
        assert_eq!(method.kind, "method");
        assert_eq!(method.container, Some("Inner".to_string()));
    }

    #[test]
    fn class_inheritance_and_interface_implementation() {
        let source = r#"
public class MyClass : BaseClass, IFoo, IBar
{
}
"#;
        let path = PathBuf::from("test.cs");
        let (_, edges, _, _, _) = index_file(&path, source).unwrap();

        let extends_edges: Vec<_> = edges.iter().filter(|e| e.kind == "extends").collect();
        assert_eq!(extends_edges.len(), 1);
        assert!(extends_edges[0].dst.contains("BaseClass"));

        let impl_edges: Vec<_> = edges.iter().filter(|e| e.kind == "implements").collect();
        assert_eq!(impl_edges.len(), 2);
    }

    #[test]
    fn extracts_using_directives() {
        let source = r#"using System;
using System.Collections.Generic;
using MyApp.Models;

public class Foo { }
"#;
        let path = PathBuf::from("test.cs");
        let (_, _, _, dependencies, import_bindings) = index_file(&path, source).unwrap();

        assert_eq!(dependencies.len(), 3);

        let system_dep = dependencies.iter().find(|d| d.to_file == "System").unwrap();
        assert_eq!(system_dep.kind, "using");

        let generic_dep = dependencies
            .iter()
            .find(|d| d.to_file == "System.Collections.Generic")
            .unwrap();
        assert_eq!(generic_dep.kind, "using");

        assert_eq!(import_bindings.len(), 3);
        let generic_binding = import_bindings
            .iter()
            .find(|b| b.original_name == "System.Collections.Generic")
            .unwrap();
        assert_eq!(generic_binding.local_name, "Generic");
    }

    #[test]
    fn detects_test_file_by_path() {
        let source = r#"
public class UserTests
{
    public void TestCreate() { }
}
"#;

        // Test by path containing /Tests/
        let path = PathBuf::from("src/Tests/UserTests.cs");
        let (symbols, _, _, _, _) = index_file(&path, source).unwrap();
        for sym in &symbols {
            assert!(sym.is_test, "Symbol {} should be marked as test", sym.name);
        }

        // Test by filename ending with Tests.cs
        let path = PathBuf::from("UserTests.cs");
        let (symbols, _, _, _, _) = index_file(&path, source).unwrap();
        for sym in &symbols {
            assert!(sym.is_test, "Symbol {} should be marked as test", sym.name);
        }
    }

    #[test]
    fn detects_test_methods_by_attributes() {
        let source = r#"
public class MyTests
{
    [Test]
    public void TestMethod() { }

    [Fact]
    public void FactMethod() { }

    [Theory]
    public void TheoryMethod() { }

    [TestMethod]
    public void MSTestMethod() { }

    public void NotATest() { }
}
"#;
        let path = PathBuf::from("src/MyClass.cs"); // NOT a test path
        let (symbols, _, _, _, _) = index_file(&path, source).unwrap();

        let test_m = symbols.iter().find(|s| s.name == "TestMethod").unwrap();
        assert!(test_m.is_test);

        let fact_m = symbols.iter().find(|s| s.name == "FactMethod").unwrap();
        assert!(fact_m.is_test);

        let theory_m = symbols.iter().find(|s| s.name == "TheoryMethod").unwrap();
        assert!(theory_m.is_test);

        let mstest_m = symbols.iter().find(|s| s.name == "MSTestMethod").unwrap();
        assert!(mstest_m.is_test);

        let not_test = symbols.iter().find(|s| s.name == "NotATest").unwrap();
        assert!(!not_test.is_test);
    }

    #[test]
    fn collects_references() {
        let source = r#"
public class Greeter
{
    public void Greet() { }
}

public class Main
{
    public void Run()
    {
        var g = new Greeter();
        g.Greet();
    }
}
"#;
        let path = PathBuf::from("test.cs");
        let (_, _, refs, _, _) = index_file(&path, source).unwrap();

        // Should have at least one reference
        assert!(!refs.is_empty());
    }

    #[test]
    fn visibility_defaults() {
        let source = r#"
class TopLevel
{
    void DefaultMethod() { }
    int DefaultField;
    string DefaultProp { get; set; }
}
"#;
        let path = PathBuf::from("test.cs");
        let (symbols, _, _, _, _) = index_file(&path, source).unwrap();

        // Top-level type defaults to internal
        let top = symbols.iter().find(|s| s.name == "TopLevel").unwrap();
        assert_eq!(top.visibility, Some("internal".to_string()));

        // Members default to private
        let method = symbols.iter().find(|s| s.name == "DefaultMethod").unwrap();
        assert_eq!(method.visibility, Some("private".to_string()));

        let field = symbols.iter().find(|s| s.name == "DefaultField").unwrap();
        assert_eq!(field.visibility, Some("private".to_string()));

        let prop = symbols.iter().find(|s| s.name == "DefaultProp").unwrap();
        assert_eq!(prop.visibility, Some("private".to_string()));
    }

    #[test]
    fn registry_finds_csharp_parser() {
        use crate::languages::ParserRegistry;

        let registry = ParserRegistry::new();
        assert!(registry.is_supported(&PathBuf::from("test.cs")));
    }
}
