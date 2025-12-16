use crate::languages::ImportBindingInfo;
use crate::store::{normalize_path, EdgeRecord, FileDependency, ReferenceRecord, SymbolRecord};
use anyhow::{Context, Result};
use once_cell::sync::Lazy;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use tree_sitter::{Language, Node, Parser, TreeCursor};

static KOTLIN_LANGUAGE: Lazy<Language> = Lazy::new(tree_sitter_kotlin_codanna::language);

/// Index a Kotlin file, returning symbols, edges, references, file dependencies, and import bindings.
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
        .set_language(&KOTLIN_LANGUAGE)
        .context("failed to set Kotlin language")?;
    let tree = parser
        .parse(source, None)
        .context("failed to parse Kotlin file")?;

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

    let (dependencies, import_bindings) = collect_imports(path, source, &tree.root_node());

    Ok((symbols, edges, references, dependencies, import_bindings))
}

/// Walk the AST and extract symbols
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
            "class_declaration" => {
                handle_class(
                    path,
                    source,
                    &node,
                    container.clone(),
                    symbols,
                    edges,
                    declared_spans,
                    symbol_by_name,
                );
            }
            "object_declaration" => {
                handle_object(
                    path,
                    source,
                    &node,
                    container.clone(),
                    symbols,
                    edges,
                    declared_spans,
                    symbol_by_name,
                );
            }
            "interface_declaration" => {
                handle_interface(
                    path,
                    source,
                    &node,
                    container.clone(),
                    symbols,
                    edges,
                    declared_spans,
                    symbol_by_name,
                );
            }
            "function_declaration" => {
                handle_function(
                    path,
                    source,
                    &node,
                    container.clone(),
                    symbols,
                    declared_spans,
                    symbol_by_name,
                );
            }
            "property_declaration" => {
                handle_property(
                    path,
                    source,
                    &node,
                    container.clone(),
                    symbols,
                    declared_spans,
                    symbol_by_name,
                );
            }
            "companion_object" => {
                // Companion objects are treated as nested objects
                if let Some(name) = find_name(&node, source) {
                    let sym = make_symbol(
                        path,
                        &node,
                        &name,
                        "object",
                        container.clone(),
                        source.as_bytes(),
                    );
                    declared_spans.insert((sym.start as usize, sym.end as usize));
                    symbol_by_name.insert(name.clone(), sym.id.clone());
                    symbols.push(sym);
                } else {
                    // Anonymous companion object - use "Companion" as the name
                    let sym = make_symbol(
                        path,
                        &node,
                        "Companion",
                        "object",
                        container.clone(),
                        source.as_bytes(),
                    );
                    declared_spans.insert((sym.start as usize, sym.end as usize));
                    symbol_by_name.insert("Companion".to_string(), sym.id.clone());
                    symbols.push(sym);
                }
            }
            _ => {}
        }

        // Recurse into children
        if cursor.goto_first_child() {
            let child_container = match node.kind() {
                "class_declaration" | "interface_declaration" | "object_declaration" => {
                    find_name(&node, source).or(container.clone())
                }
                _ => container.clone(),
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
fn handle_class(
    path: &Path,
    source: &str,
    node: &Node,
    container: Option<String>,
    symbols: &mut Vec<SymbolRecord>,
    edges: &mut Vec<EdgeRecord>,
    declared_spans: &mut HashSet<(usize, usize)>,
    symbol_by_name: &mut HashMap<String, String>,
) {
    if let Some(name) = find_name(node, source) {
        // Determine the kind based on modifiers and keywords
        let kind = determine_class_kind(node);

        let sym = make_symbol(
            path,
            node,
            &name,
            &kind,
            container.clone(),
            source.as_bytes(),
        );
        declared_spans.insert((sym.start as usize, sym.end as usize));
        symbol_by_name.insert(name.clone(), sym.id.clone());

        // Record inheritance edges
        record_inheritance_edges(path, source, node, &sym.id, edges, symbol_by_name);

        symbols.push(sym);

        // For enum classes, also extract enum entries
        if kind == "enum_class" {
            extract_enum_entries(
                path,
                source,
                node,
                Some(name),
                symbols,
                declared_spans,
                symbol_by_name,
            );
        }
    }
}

/// Determine the kind of a class_declaration node
fn determine_class_kind(node: &Node) -> String {
    let mut is_data = false;
    let mut is_sealed = false;
    let mut is_enum = false;
    let mut is_interface = false;

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "modifiers" => {
                // Check for data/sealed modifiers
                let mut mod_cursor = child.walk();
                for modifier in child.children(&mut mod_cursor) {
                    if modifier.kind() == "class_modifier" {
                        let mut class_mod_cursor = modifier.walk();
                        for cm in modifier.children(&mut class_mod_cursor) {
                            match cm.kind() {
                                "data" => is_data = true,
                                "sealed" => is_sealed = true,
                                _ => {}
                            }
                        }
                    }
                }
            }
            "enum" => is_enum = true,
            "interface" => is_interface = true,
            _ => {}
        }
    }

    // Determine final kind based on flags
    if is_enum {
        "enum_class".to_string()
    } else if is_sealed && is_interface {
        "sealed_interface".to_string()
    } else if is_sealed {
        "sealed_class".to_string()
    } else if is_data {
        "data_class".to_string()
    } else if is_interface {
        "interface".to_string()
    } else {
        "class".to_string()
    }
}

/// Extract enum entries from an enum class
fn extract_enum_entries(
    path: &Path,
    source: &str,
    node: &Node,
    container: Option<String>,
    symbols: &mut Vec<SymbolRecord>,
    declared_spans: &mut HashSet<(usize, usize)>,
    symbol_by_name: &mut HashMap<String, String>,
) {
    let mut stack = vec![*node];
    while let Some(n) = stack.pop() {
        if n.kind() == "enum_entry" {
            if let Some(entry_name) = find_name(&n, source) {
                let sym = make_symbol(
                    path,
                    &n,
                    &entry_name,
                    "enum_entry",
                    container.clone(),
                    source.as_bytes(),
                );
                declared_spans.insert((sym.start as usize, sym.end as usize));
                symbol_by_name.insert(entry_name, sym.id.clone());
                symbols.push(sym);
            }
        }
        let mut cursor = n.walk();
        for child in n.children(&mut cursor) {
            stack.push(child);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_object(
    path: &Path,
    source: &str,
    node: &Node,
    container: Option<String>,
    symbols: &mut Vec<SymbolRecord>,
    edges: &mut Vec<EdgeRecord>,
    declared_spans: &mut HashSet<(usize, usize)>,
    symbol_by_name: &mut HashMap<String, String>,
) {
    if let Some(name) = find_name(node, source) {
        let sym = make_symbol(
            path,
            node,
            &name,
            "object",
            container.clone(),
            source.as_bytes(),
        );
        declared_spans.insert((sym.start as usize, sym.end as usize));
        symbol_by_name.insert(name.clone(), sym.id.clone());

        // Objects can also implement interfaces
        record_inheritance_edges(path, source, node, &sym.id, edges, symbol_by_name);

        symbols.push(sym);
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_interface(
    path: &Path,
    source: &str,
    node: &Node,
    container: Option<String>,
    symbols: &mut Vec<SymbolRecord>,
    edges: &mut Vec<EdgeRecord>,
    declared_spans: &mut HashSet<(usize, usize)>,
    symbol_by_name: &mut HashMap<String, String>,
) {
    if let Some(name) = find_name(node, source) {
        let sym = make_symbol(
            path,
            node,
            &name,
            "interface",
            container.clone(),
            source.as_bytes(),
        );
        declared_spans.insert((sym.start as usize, sym.end as usize));
        symbol_by_name.insert(name.clone(), sym.id.clone());

        // Interfaces can extend other interfaces
        record_inheritance_edges(path, source, node, &sym.id, edges, symbol_by_name);

        symbols.push(sym);
    }
}

fn handle_function(
    path: &Path,
    source: &str,
    node: &Node,
    container: Option<String>,
    symbols: &mut Vec<SymbolRecord>,
    declared_spans: &mut HashSet<(usize, usize)>,
    symbol_by_name: &mut HashMap<String, String>,
) {
    if let Some(name) = find_name(node, source) {
        // Check for receiver_type (indicates extension function)
        let receiver_type = extract_receiver_type(node, source);

        let (kind, qualifier) = if let Some(ref recv_type) = receiver_type {
            // Extension function/method
            let kind = if container.is_some() {
                "extension_method"
            } else {
                "extension_function"
            };
            // Qualifier includes receiver type for searchability
            let qual = match &container {
                Some(c) => Some(format!("{}.{}", c, recv_type)),
                None => Some(recv_type.clone()),
            };
            (kind, qual)
        } else {
            // Regular function/method
            let kind = if container.is_some() {
                "method"
            } else {
                "function"
            };
            (kind, container.clone())
        };

        let sym = make_symbol_with_qualifier(
            path,
            node,
            &name,
            kind,
            container,
            qualifier,
            source.as_bytes(),
        );
        declared_spans.insert((sym.start as usize, sym.end as usize));
        symbol_by_name.insert(name.clone(), sym.id.clone());
        symbols.push(sym);
    }
}

/// Extract the receiver type from a function declaration if it's an extension function
fn extract_receiver_type(node: &Node, source: &str) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "receiver_type" {
            // Extract the type name from the receiver_type node
            return extract_type_name(&child, source);
        }
    }
    None
}

fn handle_property(
    path: &Path,
    source: &str,
    node: &Node,
    container: Option<String>,
    symbols: &mut Vec<SymbolRecord>,
    declared_spans: &mut HashSet<(usize, usize)>,
    symbol_by_name: &mut HashMap<String, String>,
) {
    if let Some(name) = find_property_name(node, source) {
        // Check for receiver_type (indicates extension property)
        let receiver_type = extract_receiver_type(node, source);

        let (kind, qualifier) = if let Some(ref recv_type) = receiver_type {
            // Extension property
            let qual = match &container {
                Some(c) => Some(format!("{}.{}", c, recv_type)),
                None => Some(recv_type.clone()),
            };
            ("extension_property", qual)
        } else {
            ("property", container.clone())
        };

        let sym = make_symbol_with_qualifier(
            path,
            node,
            &name,
            kind,
            container,
            qualifier,
            source.as_bytes(),
        );
        declared_spans.insert((sym.start as usize, sym.end as usize));
        symbol_by_name.insert(name.clone(), sym.id.clone());
        symbols.push(sym);
    }
}

/// Record extends and implements edges from delegation_specifiers
fn record_inheritance_edges(
    path: &Path,
    source: &str,
    node: &Node,
    src_id: &str,
    edges: &mut Vec<EdgeRecord>,
    symbol_by_name: &HashMap<String, String>,
) {
    // Look for delegation_specifiers (the `: BaseClass, Interface` part)
    let mut stack = vec![*node];
    while let Some(n) = stack.pop() {
        if n.kind() == "delegation_specifier" || n.kind() == "user_type" {
            // Extract the type name
            if let Some(type_name) = extract_type_name(&n, source) {
                // Try to resolve to known symbol, otherwise use name-based ID
                let dst_id = symbol_by_name
                    .get(&type_name)
                    .cloned()
                    .unwrap_or_else(|| format!("{}#{}", normalize_path(path), type_name));

                // Determine if extends or implements (heuristic: first is usually extends for classes)
                let kind = if type_name
                    .chars()
                    .next()
                    .map(|c| c.is_uppercase())
                    .unwrap_or(false)
                {
                    // Could be either - Kotlin doesn't syntactically distinguish
                    // We'll use "extends" for the first one and "implements" for others
                    "implements"
                } else {
                    "extends"
                };

                edges.push(EdgeRecord {
                    src: src_id.to_string(),
                    dst: dst_id,
                    kind: kind.to_string(),
                });
            }
        }

        // Continue walking
        let mut cursor = n.walk();
        for child in n.children(&mut cursor) {
            stack.push(child);
        }
    }
}

/// Extract the primary type name from a type node (e.g., "List" from "List<Int>")
fn extract_type_name(node: &Node, source: &str) -> Option<String> {
    // BFS approach to find the first type_identifier at the shallowest level
    // This ensures we get "List" from "List<Int>" rather than "Int"
    let mut queue = std::collections::VecDeque::new();
    queue.push_back(*node);

    while let Some(n) = queue.pop_front() {
        // Check if this node is a type identifier
        if n.kind() == "type_identifier"
            || n.kind() == "simple_identifier"
            || n.kind() == "identifier"
        {
            let name = slice(source, &n);
            if !name.is_empty() {
                return Some(name);
            }
        }

        // Add children, but skip type_arguments to avoid descending into generic params
        let mut cursor = n.walk();
        for child in n.children(&mut cursor) {
            // Skip type_arguments to avoid getting type params like <Int> in List<Int>
            if child.kind() != "type_arguments" {
                queue.push_back(child);
            }
        }
    }
    None
}

/// Collect references to symbols
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
        if node.kind() == "simple_identifier" {
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

/// Collect import statements and create dependencies
fn collect_imports(
    path: &Path,
    source: &str,
    root: &Node,
) -> (Vec<FileDependency>, Vec<ImportBindingInfo>) {
    let mut dependencies = Vec::new();
    let mut import_bindings = Vec::new();
    let from_file = normalize_path(path);

    let mut stack = vec![*root];
    while let Some(node) = stack.pop() {
        if node.kind() == "import_header" {
            if let Some((import_path, alias)) = parse_import(&node, source) {
                // For now, we create import bindings but can't resolve to files
                // without knowing the project structure
                let last_segment = import_path.rsplit('.').next().unwrap_or(&import_path);
                let local_name = alias.unwrap_or_else(|| last_segment.to_string());

                import_bindings.push(ImportBindingInfo {
                    local_name,
                    source_file: from_file.clone(), // Will need proper resolution
                    original_name: last_segment.to_string(),
                });

                // Create a dependency record (path-based resolution would need project context)
                dependencies.push(FileDependency {
                    from_file: from_file.clone(),
                    to_file: import_path,
                    kind: "import".to_string(),
                });
            }
        }

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            stack.push(child);
        }
    }

    (dependencies, import_bindings)
}

/// Parse an import statement and return (path, optional alias)
fn parse_import(node: &Node, source: &str) -> Option<(String, Option<String>)> {
    let mut import_path = String::new();
    let mut alias = None;

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "identifier" | "simple_identifier" => {
                if import_path.is_empty() {
                    import_path = slice(source, &child);
                } else {
                    import_path.push('.');
                    import_path.push_str(&slice(source, &child));
                }
            }
            "import_alias" => {
                // Extract alias name
                let mut alias_cursor = child.walk();
                for alias_child in child.children(&mut alias_cursor) {
                    if alias_child.kind() == "simple_identifier"
                        || alias_child.kind() == "identifier"
                    {
                        alias = Some(slice(source, &alias_child));
                        break;
                    }
                }
            }
            _ => {
                // Recurse to find identifiers in nested structures
                let mut inner_stack = vec![child];
                while let Some(inner) = inner_stack.pop() {
                    if inner.kind() == "simple_identifier" || inner.kind() == "identifier" {
                        if import_path.is_empty() {
                            import_path = slice(source, &inner);
                        } else {
                            import_path.push('.');
                            import_path.push_str(&slice(source, &inner));
                        }
                    }
                    let mut inner_cursor = inner.walk();
                    for inner_child in inner.children(&mut inner_cursor) {
                        inner_stack.push(inner_child);
                    }
                }
            }
        }
    }

    if import_path.is_empty() {
        None
    } else {
        Some((import_path, alias))
    }
}

/// Find the name of a declaration node
fn find_name(node: &Node, source: &str) -> Option<String> {
    // First try field lookup
    if let Some(name_node) = node.child_by_field_name("name") {
        let name = slice(source, &name_node);
        if !name.is_empty() {
            return Some(name);
        }
    }

    // Walk children looking for simple_identifier that's the name
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "simple_identifier" || child.kind() == "type_identifier" {
            let name = slice(source, &child);
            if !name.is_empty() {
                return Some(name);
            }
        }
    }
    None
}

/// Find property name from variable declaration
fn find_property_name(node: &Node, source: &str) -> Option<String> {
    // Look for variable_declaration within property
    let mut stack = vec![*node];
    while let Some(n) = stack.pop() {
        if n.kind() == "variable_declaration" {
            if let Some(name) = find_name(&n, source) {
                return Some(name);
            }
        }
        if n.kind() == "simple_identifier" {
            let name = slice(source, &n);
            if !name.is_empty() {
                return Some(name);
            }
        }
        let mut cursor = n.walk();
        for child in n.children(&mut cursor) {
            stack.push(child);
        }
    }
    None
}

fn make_symbol(
    path: &Path,
    node: &Node,
    name: &str,
    kind: &str,
    container: Option<String>,
    source: &[u8],
) -> SymbolRecord {
    let visibility = extract_visibility(node);
    let content_hash = super::compute_content_hash(source, node.start_byte(), node.end_byte());
    let qualifier = container.as_ref().map(|c| c.to_string());

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
    }
}

/// Create a symbol with separate qualifier (used for extension functions where
/// qualifier shows the receiver type but container shows the enclosing class)
fn make_symbol_with_qualifier(
    path: &Path,
    node: &Node,
    name: &str,
    kind: &str,
    container: Option<String>,
    qualifier: Option<String>,
    source: &[u8],
) -> SymbolRecord {
    let visibility = extract_visibility(node);
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
    }
}

/// Extract visibility modifier from a node
fn extract_visibility(node: &Node) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "modifiers" {
            let mut mod_cursor = child.walk();
            for modifier in child.children(&mut mod_cursor) {
                if modifier.kind() == "visibility_modifier" {
                    let mut vis_cursor = modifier.walk();
                    for vis in modifier.children(&mut vis_cursor) {
                        match vis.kind() {
                            "public" => return Some("public".to_string()),
                            "private" => return Some("private".to_string()),
                            "protected" => return Some("protected".to_string()),
                            "internal" => return Some("internal".to_string()),
                            _ => {}
                        }
                    }
                }
            }
        }
    }
    // Default visibility in Kotlin is public
    Some("public".to_string())
}

fn slice(source: &str, node: &Node) -> String {
    let bytes = node.byte_range();
    source.get(bytes).unwrap_or_default().trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn extracts_kotlin_symbols() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("Test.kt");
        let source = r#"
            class Person(val name: String) {
                fun greet() {
                    println("Hello, $name")
                }
            }

            interface Greeter {
                fun greet()
            }

            object Singleton {
                val instance = "single"
            }

            fun topLevel() {}
        "#;
        fs::write(&path, source).unwrap();

        let (symbols, _edges, _refs, _deps, _imports) = index_file(&path, source).unwrap();
        let names: Vec<_> = symbols.iter().map(|s| s.name.as_str()).collect();

        assert!(names.contains(&"Person"), "Should find Person class");
        assert!(names.contains(&"greet"), "Should find greet method");
        assert!(names.contains(&"Greeter"), "Should find Greeter interface");
        assert!(names.contains(&"Singleton"), "Should find Singleton object");
        assert!(names.contains(&"topLevel"), "Should find topLevel function");
    }

    #[test]
    fn extracts_visibility_modifiers() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("Visibility.kt");
        let source = r#"
            public class PublicClass
            private class PrivateClass
            internal class InternalClass
            protected class ProtectedClass
        "#;
        fs::write(&path, source).unwrap();

        let (symbols, _edges, _refs, _deps, _imports) = index_file(&path, source).unwrap();

        let public_class = symbols.iter().find(|s| s.name == "PublicClass").unwrap();
        assert_eq!(public_class.visibility.as_deref(), Some("public"));

        let private_class = symbols.iter().find(|s| s.name == "PrivateClass").unwrap();
        assert_eq!(private_class.visibility.as_deref(), Some("private"));

        let internal_class = symbols.iter().find(|s| s.name == "InternalClass").unwrap();
        assert_eq!(internal_class.visibility.as_deref(), Some("internal"));
    }

    #[test]
    fn captures_inheritance_edges() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("Inheritance.kt");
        let source = r#"
            interface Animal {
                fun speak()
            }

            open class Mammal

            class Dog : Mammal(), Animal {
                override fun speak() {}
            }
        "#;
        fs::write(&path, source).unwrap();

        let (symbols, edges, _refs, _deps, _imports) = index_file(&path, source).unwrap();

        assert!(symbols.iter().any(|s| s.name == "Dog"));
        assert!(symbols.iter().any(|s| s.name == "Animal"));
        assert!(symbols.iter().any(|s| s.name == "Mammal"));

        // Dog should have edges to Animal and Mammal
        assert!(
            edges
                .iter()
                .any(|e| e.kind == "implements" || e.kind == "extends"),
            "Should have inheritance edges"
        );
    }

    #[test]
    fn extracts_companion_objects() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("Companion.kt");
        let source = r#"
            class Factory {
                companion object {
                    fun create(): Factory = Factory()
                }
            }
        "#;
        fs::write(&path, source).unwrap();

        let (symbols, _edges, _refs, _deps, _imports) = index_file(&path, source).unwrap();
        let names: Vec<_> = symbols.iter().map(|s| s.name.as_str()).collect();

        assert!(names.contains(&"Factory"));
        assert!(names.contains(&"Companion"));
        assert!(names.contains(&"create"));
    }

    #[test]
    fn extracts_data_sealed_enum_classes() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("SpecialClasses.kt");
        let source = r#"
data class Person(val name: String, val age: Int)

sealed class Result {
    data class Success(val value: String) : Result()
    data class Error(val message: String) : Result()
}

sealed interface Event {
    data class Click(val x: Int) : Event
    object Close : Event
}

enum class Color {
    RED,
    GREEN,
    BLUE
}

enum class Direction(val degrees: Int) {
    NORTH(0),
    EAST(90),
    SOUTH(180),
    WEST(270)
}
        "#;
        fs::write(&path, source).unwrap();

        let (symbols, _edges, _refs, _deps, _imports) = index_file(&path, source).unwrap();

        // Check data class
        let person = symbols.iter().find(|s| s.name == "Person").unwrap();
        assert_eq!(person.kind, "data_class", "Person should be a data_class");

        // Check sealed class
        let result = symbols.iter().find(|s| s.name == "Result").unwrap();
        assert_eq!(
            result.kind, "sealed_class",
            "Result should be a sealed_class"
        );

        // Check nested data classes inside sealed class
        let success = symbols.iter().find(|s| s.name == "Success").unwrap();
        assert_eq!(success.kind, "data_class", "Success should be a data_class");
        assert_eq!(
            success.container.as_deref(),
            Some("Result"),
            "Success should be inside Result"
        );

        // Check sealed interface
        let event = symbols.iter().find(|s| s.name == "Event").unwrap();
        assert_eq!(
            event.kind, "sealed_interface",
            "Event should be a sealed_interface"
        );

        // Check enum class
        let color = symbols.iter().find(|s| s.name == "Color").unwrap();
        assert_eq!(color.kind, "enum_class", "Color should be an enum_class");

        // Check enum entries
        let red = symbols.iter().find(|s| s.name == "RED").unwrap();
        assert_eq!(red.kind, "enum_entry", "RED should be an enum_entry");
        assert_eq!(
            red.container.as_deref(),
            Some("Color"),
            "RED should be inside Color"
        );

        let green = symbols.iter().find(|s| s.name == "GREEN").unwrap();
        assert_eq!(green.kind, "enum_entry", "GREEN should be an enum_entry");

        // Check Direction enum
        let direction = symbols.iter().find(|s| s.name == "Direction").unwrap();
        assert_eq!(
            direction.kind, "enum_class",
            "Direction should be an enum_class"
        );

        let north = symbols.iter().find(|s| s.name == "NORTH").unwrap();
        assert_eq!(north.kind, "enum_entry", "NORTH should be an enum_entry");
        assert_eq!(
            north.container.as_deref(),
            Some("Direction"),
            "NORTH should be inside Direction"
        );
    }

    #[test]
    fn extracts_extension_functions_and_properties() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("Extensions.kt");
        let source = r#"
fun String.addExclamation() = "$this!"

fun List<Int>.sum(): Int = this.fold(0) { acc, i -> acc + i }

fun <T> MutableList<T>.swap(i: Int, j: Int) {
    val tmp = this[i]
    this[i] = this[j]
    this[j] = tmp
}

val String.lastChar: Char
    get() = this[length - 1]

class StringUtils {
    fun String.toTitleCase(): String = this.capitalize()
}
        "#;
        fs::write(&path, source).unwrap();

        let (symbols, _edges, _refs, _deps, _imports) = index_file(&path, source).unwrap();

        // Check extension function
        let add_excl = symbols.iter().find(|s| s.name == "addExclamation").unwrap();
        assert_eq!(
            add_excl.kind, "extension_function",
            "addExclamation should be an extension_function"
        );
        assert_eq!(
            add_excl.qualifier.as_deref(),
            Some("String"),
            "addExclamation should have String as qualifier"
        );

        // Check extension function on generic type
        let sum = symbols.iter().find(|s| s.name == "sum").unwrap();
        assert_eq!(
            sum.kind, "extension_function",
            "sum should be an extension_function"
        );
        assert_eq!(
            sum.qualifier.as_deref(),
            Some("List"),
            "sum should have List as qualifier"
        );

        // Check generic extension function
        let swap = symbols.iter().find(|s| s.name == "swap").unwrap();
        assert_eq!(
            swap.kind, "extension_function",
            "swap should be an extension_function"
        );
        assert_eq!(
            swap.qualifier.as_deref(),
            Some("MutableList"),
            "swap should have MutableList as qualifier"
        );

        // Check extension property
        let last_char = symbols.iter().find(|s| s.name == "lastChar").unwrap();
        assert_eq!(
            last_char.kind, "extension_property",
            "lastChar should be an extension_property"
        );
        assert_eq!(
            last_char.qualifier.as_deref(),
            Some("String"),
            "lastChar should have String as qualifier"
        );

        // Check extension function defined inside a class
        let to_title = symbols.iter().find(|s| s.name == "toTitleCase").unwrap();
        assert_eq!(
            to_title.kind, "extension_method",
            "toTitleCase should be an extension_method"
        );
        assert_eq!(
            to_title.container.as_deref(),
            Some("StringUtils"),
            "toTitleCase should be inside StringUtils"
        );
        assert_eq!(
            to_title.qualifier.as_deref(),
            Some("StringUtils.String"),
            "toTitleCase should have StringUtils.String as qualifier"
        );
    }
}
