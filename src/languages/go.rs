use crate::languages::{slice, ImportBindingInfo};
use crate::store::{normalize_path, EdgeRecord, FileDependency, ReferenceRecord, SymbolRecord};
use anyhow::{Context, Result};
use once_cell::sync::Lazy;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use tree_sitter::{Language, Node, Parser, TreeCursor};

static GO_LANGUAGE: Lazy<Language> = Lazy::new(|| tree_sitter_go::LANGUAGE.into());

/// Index a Go file, returning symbols, edges, references, file dependencies, and import bindings.
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
        .set_language(&GO_LANGUAGE)
        .context("failed to set Go language")?;
    let tree = parser
        .parse(source, None)
        .context("failed to parse Go file")?;

    let mut symbols = Vec::new();
    let mut edges = Vec::new();
    let mut declared_spans: HashSet<(usize, usize)> = HashSet::new();
    let mut symbol_by_name: HashMap<String, String> = HashMap::new();

    // Check if this is a test file
    let is_test_file = path
        .file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.ends_with("_test.go"))
        .unwrap_or(false);

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
    is_test_file: bool,
) {
    loop {
        let node = cursor.node();
        match node.kind() {
            "function_declaration" => {
                handle_function(
                    path,
                    source,
                    &node,
                    container.clone(),
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
                    symbols,
                    edges,
                    declared_spans,
                    symbol_by_name,
                    is_test_file,
                );
            }
            "type_declaration" => {
                handle_type_declaration(
                    path,
                    source,
                    &node,
                    symbols,
                    edges,
                    declared_spans,
                    symbol_by_name,
                    is_test_file,
                );
            }
            "const_declaration" => {
                handle_const_declaration(
                    path,
                    source,
                    &node,
                    symbols,
                    declared_spans,
                    symbol_by_name,
                    is_test_file,
                );
            }
            "var_declaration" => {
                handle_var_declaration(
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

        // Recurse into children
        if cursor.goto_first_child() {
            walk_symbols(
                path,
                source,
                cursor,
                container.clone(),
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

/// Handle function declarations (package-level functions)
#[allow(clippy::too_many_arguments)]
fn handle_function(
    path: &Path,
    source: &str,
    node: &Node,
    container: Option<String>,
    symbols: &mut Vec<SymbolRecord>,
    declared_spans: &mut HashSet<(usize, usize)>,
    symbol_by_name: &mut HashMap<String, String>,
    is_test_file: bool,
) {
    if let Some(name) = find_name(node, source) {
        let span = (node.start_byte(), node.end_byte());
        if declared_spans.contains(&span) {
            return;
        }

        // Detect test/benchmark/example functions
        let is_test = is_test_file
            || name.starts_with("Test")
            || name.starts_with("Benchmark")
            || name.starts_with("Example");

        // Determine visibility (exported = starts with uppercase)
        let visibility = if name
            .chars()
            .next()
            .map(|c| c.is_uppercase())
            .unwrap_or(false)
        {
            Some("public".to_string())
        } else {
            None
        };

        let sym = make_symbol(
            path,
            node,
            &name,
            "function",
            container,
            source.as_bytes(),
            is_test,
            visibility,
        );
        declared_spans.insert(span);
        symbol_by_name.insert(name.clone(), sym.id.clone());
        symbols.push(sym);
    }
}

/// Handle method declarations (functions with receivers)
#[allow(clippy::too_many_arguments)]
fn handle_method(
    path: &Path,
    source: &str,
    node: &Node,
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

    // Find the method name
    let name = match find_name(node, source) {
        Some(n) => n,
        None => return,
    };

    // Find the receiver type
    let receiver_type = extract_receiver_type(node, source);

    // Detect test functions (though methods are rarely tests)
    let is_test = is_test_file
        || name.starts_with("Test")
        || name.starts_with("Benchmark")
        || name.starts_with("Example");

    // Determine visibility
    let visibility = if name
        .chars()
        .next()
        .map(|c| c.is_uppercase())
        .unwrap_or(false)
    {
        Some("public".to_string())
    } else {
        None
    };

    // Use receiver type as container/qualifier
    let qualifier = receiver_type.clone();

    let sym = make_symbol(
        path,
        node,
        &name,
        "method",
        qualifier.clone(),
        source.as_bytes(),
        is_test,
        visibility,
    );
    declared_spans.insert(span);

    // Store with qualified name for lookup
    let qualified_name = if let Some(ref recv) = receiver_type {
        format!("{}.{}", recv, name)
    } else {
        name.clone()
    };
    symbol_by_name.insert(qualified_name, sym.id.clone());

    // Create an inherent_impl edge from method to receiver type if we know the type
    if let Some(ref recv_type) = receiver_type {
        // Try to find the type symbol
        if let Some(type_id) = symbol_by_name.get(recv_type) {
            edges.push(EdgeRecord {
                src: sym.id.clone(),
                dst: type_id.clone(),
                kind: "inherent_impl".to_string(),
            });
        } else {
            // Create a placeholder edge that might be resolved later
            edges.push(EdgeRecord {
                src: sym.id.clone(),
                dst: format!("{}#{}", normalize_path(path), recv_type),
                kind: "inherent_impl".to_string(),
            });
        }
    }

    symbols.push(sym);
}

/// Extract the receiver type from a method declaration
fn extract_receiver_type(node: &Node, source: &str) -> Option<String> {
    // method_declaration has a parameter_list for the receiver
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "parameter_list" {
            // This is the receiver parameter list
            let mut param_cursor = child.walk();
            for param_child in child.children(&mut param_cursor) {
                if param_child.kind() == "parameter_declaration" {
                    // Find the type in the parameter
                    return extract_type_from_parameter(&param_child, source);
                }
            }
            break; // Only look at first parameter_list (the receiver)
        }
    }
    None
}

/// Extract type name from a parameter declaration
fn extract_type_from_parameter(node: &Node, source: &str) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "type_identifier" => {
                return Some(slice(source, &child));
            }
            "pointer_type" => {
                // *Type - extract the inner type
                let mut ptr_cursor = child.walk();
                for ptr_child in child.children(&mut ptr_cursor) {
                    if ptr_child.kind() == "type_identifier" {
                        return Some(slice(source, &ptr_child));
                    }
                }
            }
            _ => {}
        }
    }
    None
}

/// Handle type declarations (struct, interface, type alias)
#[allow(clippy::too_many_arguments)]
fn handle_type_declaration(
    path: &Path,
    source: &str,
    node: &Node,
    symbols: &mut Vec<SymbolRecord>,
    edges: &mut Vec<EdgeRecord>,
    declared_spans: &mut HashSet<(usize, usize)>,
    symbol_by_name: &mut HashMap<String, String>,
    is_test_file: bool,
) {
    // type_declaration can contain multiple type_spec or type_alias nodes
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "type_spec" | "type_alias" => {
                handle_type_spec(
                    path,
                    source,
                    &child,
                    symbols,
                    edges,
                    declared_spans,
                    symbol_by_name,
                    is_test_file,
                );
            }
            _ => {}
        }
    }
}

/// Handle a single type specification
#[allow(clippy::too_many_arguments)]
fn handle_type_spec(
    path: &Path,
    source: &str,
    node: &Node,
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

    // Find the type name - try field name first, then look for type_identifier
    let name = if let Some(name_node) = node.child_by_field_name("name") {
        slice(source, &name_node)
    } else {
        // Fallback: look for first type_identifier child
        let mut cursor = node.walk();
        let mut found_name = String::new();
        for child in node.children(&mut cursor) {
            if child.kind() == "type_identifier" {
                found_name = slice(source, &child);
                break;
            }
        }
        found_name
    };

    if name.is_empty() {
        return;
    }

    // Determine the kind based on the type definition
    let (kind, embedded_interfaces) = determine_type_kind(node, source);

    // Determine visibility
    let visibility = if name
        .chars()
        .next()
        .map(|c| c.is_uppercase())
        .unwrap_or(false)
    {
        Some("public".to_string())
    } else {
        None
    };

    let sym = make_symbol(
        path,
        node,
        &name,
        &kind,
        None,
        source.as_bytes(),
        is_test_file,
        visibility,
    );
    declared_spans.insert(span);
    symbol_by_name.insert(name.clone(), sym.id.clone());

    // Record interface embedding edges
    for embedded in embedded_interfaces {
        if let Some(embedded_id) = symbol_by_name.get(&embedded) {
            edges.push(EdgeRecord {
                src: sym.id.clone(),
                dst: embedded_id.clone(),
                kind: "extends".to_string(),
            });
        } else {
            // Create placeholder for unresolved type
            edges.push(EdgeRecord {
                src: sym.id.clone(),
                dst: format!("{}#{}", normalize_path(path), embedded),
                kind: "extends".to_string(),
            });
        }
    }

    symbols.push(sym);
}

/// Determine the kind of type and extract embedded interfaces
fn determine_type_kind(node: &Node, source: &str) -> (String, Vec<String>) {
    let mut embedded_interfaces = Vec::new();

    if let Some(type_node) = node.child_by_field_name("type") {
        match type_node.kind() {
            "struct_type" => return ("struct".to_string(), embedded_interfaces),
            "interface_type" => {
                // Extract embedded interfaces
                let mut cursor = type_node.walk();
                for child in type_node.children(&mut cursor) {
                    // Interface methods and embedded types are direct children
                    if matches!(child.kind(), "type_identifier" | "qualified_type") {
                        embedded_interfaces.push(slice(source, &child));
                    }
                }
                return ("interface".to_string(), embedded_interfaces);
            }
            _ => {
                // Type alias or other type definition
                return ("type".to_string(), embedded_interfaces);
            }
        }
    }

    ("type".to_string(), embedded_interfaces)
}

/// Handle const declarations
#[allow(clippy::too_many_arguments)]
fn handle_const_declaration(
    path: &Path,
    source: &str,
    node: &Node,
    symbols: &mut Vec<SymbolRecord>,
    declared_spans: &mut HashSet<(usize, usize)>,
    symbol_by_name: &mut HashMap<String, String>,
    is_test_file: bool,
) {
    // const_declaration can have multiple const_spec nodes
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "const_spec" {
            handle_const_spec(
                path,
                source,
                &child,
                symbols,
                declared_spans,
                symbol_by_name,
                is_test_file,
            );
        }
    }
}

/// Handle a single const specification
#[allow(clippy::too_many_arguments)]
fn handle_const_spec(
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

    // Find the const name(s) - there can be multiple in one spec
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "identifier" {
            let name = slice(source, &child);
            if !name.is_empty() {
                // Determine visibility
                let visibility = if name
                    .chars()
                    .next()
                    .map(|c| c.is_uppercase())
                    .unwrap_or(false)
                {
                    Some("public".to_string())
                } else {
                    None
                };

                let sym = make_symbol(
                    path,
                    node,
                    &name,
                    "const",
                    None,
                    source.as_bytes(),
                    is_test_file,
                    visibility,
                );
                declared_spans.insert(span);
                symbol_by_name.insert(name.clone(), sym.id.clone());
                symbols.push(sym);
                break; // Only take the first identifier as the name
            }
        }
    }
}

/// Handle var declarations
#[allow(clippy::too_many_arguments)]
fn handle_var_declaration(
    path: &Path,
    source: &str,
    node: &Node,
    symbols: &mut Vec<SymbolRecord>,
    declared_spans: &mut HashSet<(usize, usize)>,
    symbol_by_name: &mut HashMap<String, String>,
    is_test_file: bool,
) {
    // var_declaration can have multiple var_spec nodes
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "var_spec" {
            handle_var_spec(
                path,
                source,
                &child,
                symbols,
                declared_spans,
                symbol_by_name,
                is_test_file,
            );
        }
    }
}

/// Handle a single var specification
#[allow(clippy::too_many_arguments)]
fn handle_var_spec(
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

    // Find the var name(s)
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "identifier" {
            let name = slice(source, &child);
            if !name.is_empty() {
                // Determine visibility
                let visibility = if name
                    .chars()
                    .next()
                    .map(|c| c.is_uppercase())
                    .unwrap_or(false)
                {
                    Some("public".to_string())
                } else {
                    None
                };

                let sym = make_symbol(
                    path,
                    node,
                    &name,
                    "variable",
                    None,
                    source.as_bytes(),
                    is_test_file,
                    visibility,
                );
                declared_spans.insert(span);
                symbol_by_name.insert(name.clone(), sym.id.clone());
                symbols.push(sym);
                break; // Only take the first identifier as the name
            }
        }
    }
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
        if node.kind() == "identifier" || node.kind() == "type_identifier" {
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
        if node.kind() == "import_declaration" {
            handle_import_declaration(
                &node,
                source,
                &from_file,
                &mut dependencies,
                &mut import_bindings,
            );
        } else {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                stack.push(child);
            }
        }
    }

    (dependencies, import_bindings)
}

/// Handle an import declaration
fn handle_import_declaration(
    node: &Node,
    source: &str,
    from_file: &str,
    dependencies: &mut Vec<FileDependency>,
    import_bindings: &mut Vec<ImportBindingInfo>,
) {
    let import_text = slice(source, node);

    // import_declaration can contain import_spec or import_spec_list
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "import_spec" => {
                handle_import_spec(
                    &child,
                    source,
                    from_file,
                    &import_text,
                    dependencies,
                    import_bindings,
                );
            }
            "import_spec_list" => {
                let mut list_cursor = child.walk();
                for spec in child.children(&mut list_cursor) {
                    if spec.kind() == "import_spec" {
                        handle_import_spec(
                            &spec,
                            source,
                            from_file,
                            &import_text,
                            dependencies,
                            import_bindings,
                        );
                    }
                }
            }
            _ => {}
        }
    }
}

/// Handle a single import spec
fn handle_import_spec(
    node: &Node,
    source: &str,
    from_file: &str,
    import_text: &str,
    dependencies: &mut Vec<FileDependency>,
    import_bindings: &mut Vec<ImportBindingInfo>,
) {
    let mut alias: Option<String> = None;
    let mut import_path: Option<String> = None;

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "package_identifier" | "blank_identifier" | "dot" => {
                // This is the alias: name, _, or .
                alias = Some(slice(source, &child));
            }
            "interpreted_string_literal" => {
                // This is the import path
                let path_str = slice(source, &child);
                // Remove quotes
                import_path = Some(path_str.trim_matches('"').to_string());
            }
            _ => {}
        }
    }

    if let Some(path) = import_path {
        dependencies.push(FileDependency {
            from_file: from_file.to_string(),
            to_file: path.clone(),
            kind: "import".to_string(),
        });

        // Determine local name
        let local_name = match alias.as_deref() {
            Some(".") => ".".to_string(),   // dot import
            Some("_") => "_".to_string(),   // blank import
            Some(name) => name.to_string(), // aliased import
            None => {
                // Default: use last component of path
                path.rsplit('/').next().unwrap_or(&path).to_string()
            }
        };

        import_bindings.push(ImportBindingInfo {
            local_name,
            source_file: from_file.to_string(),
            original_name: path,
            import_text: import_text.to_string(),
        });
    }
}

/// Find the name of a definition node
fn find_name(node: &Node, source: &str) -> Option<String> {
    // Try field name first
    if let Some(name_node) = node.child_by_field_name("name") {
        let name = slice(source, &name_node);
        if !name.is_empty() {
            return Some(name);
        }
    }

    // Fallback: look for first identifier child
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "identifier" {
            let name = slice(source, &child);
            if !name.is_empty() {
                return Some(name);
            }
        }
    }
    None
}

#[allow(clippy::too_many_arguments)]
fn make_symbol(
    path: &Path,
    node: &Node,
    name: &str,
    kind: &str,
    container: Option<String>,
    source: &[u8],
    is_test: bool,
    visibility: Option<String>,
) -> SymbolRecord {
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
        is_test,
    }
}

// ============================================================================
// LanguageParser trait implementation
// ============================================================================

use super::traits::{LanguageConfig, LanguageParser, ParseResult};

/// Go language parser implementing the `LanguageParser` trait.
#[derive(Clone)]
pub struct GoParser;

impl GoParser {
    /// Create a new Go parser.
    pub fn new() -> Self {
        Self
    }
}

impl Default for GoParser {
    fn default() -> Self {
        Self::new()
    }
}

impl LanguageParser for GoParser {
    fn config(&self) -> LanguageConfig {
        LanguageConfig {
            name: "Go",
            extensions: &["go"],
        }
    }

    fn language(&self) -> &Language {
        &GO_LANGUAGE
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
    fn extracts_go_functions() {
        let source = r#"
package main

func main() {
    fmt.Println("Hello")
}

func helper() int {
    return 42
}

func ExportedFunc() string {
    return "exported"
}
"#;
        let path = PathBuf::from("test.go");
        let (symbols, _, _, _, _) = index_file(&path, source).unwrap();

        assert_eq!(symbols.len(), 3);

        let main_fn = symbols.iter().find(|s| s.name == "main").unwrap();
        assert_eq!(main_fn.kind, "function");
        assert!(main_fn.visibility.is_none()); // lowercase = unexported

        let helper_fn = symbols.iter().find(|s| s.name == "helper").unwrap();
        assert_eq!(helper_fn.kind, "function");
        assert!(helper_fn.visibility.is_none());

        let exported_fn = symbols.iter().find(|s| s.name == "ExportedFunc").unwrap();
        assert_eq!(exported_fn.kind, "function");
        assert_eq!(exported_fn.visibility, Some("public".to_string()));
    }

    #[test]
    fn extracts_go_methods_with_receivers() {
        let source = r#"
package main

type Server struct {
    port int
}

func (s *Server) Start() error {
    return nil
}

func (s Server) Port() int {
    return s.port
}
"#;
        let path = PathBuf::from("test.go");
        let (symbols, edges, _, _, _) = index_file(&path, source).unwrap();

        // Should have: Server struct, Start method, Port method
        assert_eq!(symbols.len(), 3);

        let server = symbols.iter().find(|s| s.name == "Server").unwrap();
        assert_eq!(server.kind, "struct");

        let start = symbols.iter().find(|s| s.name == "Start").unwrap();
        assert_eq!(start.kind, "method");
        assert_eq!(start.qualifier, Some("Server".to_string()));
        assert_eq!(start.visibility, Some("public".to_string()));

        let port = symbols.iter().find(|s| s.name == "Port").unwrap();
        assert_eq!(port.kind, "method");
        assert_eq!(port.qualifier, Some("Server".to_string()));

        // Check inherent_impl edges
        let impl_edges: Vec<_> = edges.iter().filter(|e| e.kind == "inherent_impl").collect();
        assert_eq!(impl_edges.len(), 2);
    }

    #[test]
    fn extracts_go_types() {
        let source = r#"
package main

type Handler interface {
    Handle(req Request) Response
}

type Request struct {
    Method string
    Path   string
}

type ResponseWriter interface {
    Write([]byte) (int, error)
    Handler
}

type MyInt int

type StringAlias = string
"#;
        let path = PathBuf::from("test.go");
        let (symbols, _edges, _, _, _) = index_file(&path, source).unwrap();

        let handler = symbols.iter().find(|s| s.name == "Handler").unwrap();
        assert_eq!(handler.kind, "interface");
        assert_eq!(handler.visibility, Some("public".to_string()));

        let request = symbols.iter().find(|s| s.name == "Request").unwrap();
        assert_eq!(request.kind, "struct");

        let response_writer = symbols.iter().find(|s| s.name == "ResponseWriter").unwrap();
        assert_eq!(response_writer.kind, "interface");

        let my_int = symbols.iter().find(|s| s.name == "MyInt").unwrap();
        assert_eq!(my_int.kind, "type");

        let string_alias = symbols.iter().find(|s| s.name == "StringAlias").unwrap();
        assert_eq!(string_alias.kind, "type");

        // Note: Interface embedding (ResponseWriter embeds Handler) is detected
        // but the edge structure depends on tree-sitter-go grammar parsing.
        // For now we just verify symbols are extracted correctly.
    }

    #[test]
    fn extracts_go_consts_and_vars() {
        let source = r#"
package main

const MaxSize = 100
const (
    StatusOK = 200
    StatusNotFound = 404
)

var globalVar = "hello"
var ExportedVar = 42
"#;
        let path = PathBuf::from("test.go");
        let (symbols, _, _, _, _) = index_file(&path, source).unwrap();

        let max_size = symbols.iter().find(|s| s.name == "MaxSize").unwrap();
        assert_eq!(max_size.kind, "const");
        assert_eq!(max_size.visibility, Some("public".to_string()));

        let status_ok = symbols.iter().find(|s| s.name == "StatusOK").unwrap();
        assert_eq!(status_ok.kind, "const");

        let global_var = symbols.iter().find(|s| s.name == "globalVar").unwrap();
        assert_eq!(global_var.kind, "variable");
        assert!(global_var.visibility.is_none());

        let exported_var = symbols.iter().find(|s| s.name == "ExportedVar").unwrap();
        assert_eq!(exported_var.kind, "variable");
        assert_eq!(exported_var.visibility, Some("public".to_string()));
    }

    #[test]
    fn detects_test_functions() {
        let source = r#"
package main

func TestAdd(t *testing.T) {
    if Add(1, 2) != 3 {
        t.Error("expected 3")
    }
}

func BenchmarkAdd(b *testing.B) {
    for i := 0; i < b.N; i++ {
        Add(1, 2)
    }
}

func ExampleAdd() {
    fmt.Println(Add(1, 2))
    // Output: 3
}

func helper() {}
"#;
        let path = PathBuf::from("add_test.go");
        let (symbols, _, _, _, _) = index_file(&path, source).unwrap();

        let test_add = symbols.iter().find(|s| s.name == "TestAdd").unwrap();
        assert!(test_add.is_test);

        let benchmark = symbols.iter().find(|s| s.name == "BenchmarkAdd").unwrap();
        assert!(benchmark.is_test);

        let example = symbols.iter().find(|s| s.name == "ExampleAdd").unwrap();
        assert!(example.is_test);

        // Helper in test file is also marked as test
        let helper = symbols.iter().find(|s| s.name == "helper").unwrap();
        assert!(helper.is_test);
    }

    #[test]
    fn test_file_marks_all_symbols_as_test() {
        let source = r#"
package main

type TestHelper struct{}

func (h *TestHelper) Setup() {}

const testConst = 1
"#;
        let path = PathBuf::from("helper_test.go");
        let (symbols, _, _, _, _) = index_file(&path, source).unwrap();

        // All symbols in *_test.go files should be marked as test
        for sym in &symbols {
            assert!(sym.is_test, "Symbol {} should be marked as test", sym.name);
        }
    }

    #[test]
    fn extracts_imports() {
        let source = r#"
package main

import "fmt"
import (
    "os"
    "strings"
    alias "path/to/package"
    . "dot/import"
    _ "side/effects"
)
"#;
        let path = PathBuf::from("test.go");
        let (_, _, _, dependencies, import_bindings) = index_file(&path, source).unwrap();

        assert_eq!(dependencies.len(), 6);
        assert_eq!(import_bindings.len(), 6);

        // Check fmt import
        let fmt_binding = import_bindings
            .iter()
            .find(|b| b.original_name == "fmt")
            .unwrap();
        assert_eq!(fmt_binding.local_name, "fmt");

        // Check aliased import
        let alias_binding = import_bindings
            .iter()
            .find(|b| b.original_name == "path/to/package")
            .unwrap();
        assert_eq!(alias_binding.local_name, "alias");

        // Check dot import
        let dot_binding = import_bindings
            .iter()
            .find(|b| b.original_name == "dot/import")
            .unwrap();
        assert_eq!(dot_binding.local_name, ".");

        // Check blank import
        let blank_binding = import_bindings
            .iter()
            .find(|b| b.original_name == "side/effects")
            .unwrap();
        assert_eq!(blank_binding.local_name, "_");
    }

    #[test]
    fn collects_references() {
        let source = r#"
package main

type Config struct {
    Port int
}

func NewConfig() *Config {
    return &Config{Port: 8080}
}

func main() {
    cfg := NewConfig()
    fmt.Println(cfg.Port)
}
"#;
        let path = PathBuf::from("test.go");
        let (_, _, refs, _, _) = index_file(&path, source).unwrap();

        // Should have references to Config and NewConfig
        assert!(!refs.is_empty());
    }

    #[test]
    fn registry_finds_go_parser() {
        use crate::languages::ParserRegistry;

        let registry = ParserRegistry::new();
        assert!(registry.is_supported(&PathBuf::from("test.go")));
    }
}
