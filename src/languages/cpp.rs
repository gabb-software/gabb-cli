use crate::languages::{slice, ImportBindingInfo, SymbolBinding};
use crate::store::{normalize_path, EdgeRecord, FileDependency, ReferenceRecord, SymbolRecord};
use anyhow::{Context, Result};
use once_cell::sync::Lazy;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use tree_sitter::{Language, Node, Parser, TreeCursor};

static CPP_LANGUAGE: Lazy<Language> = Lazy::new(|| tree_sitter_cpp::LANGUAGE.into());

/// Index a C++ file, returning symbols, edges, references, file dependencies, and import bindings.
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
        .set_language(&CPP_LANGUAGE)
        .context("failed to set C++ language")?;
    let tree = parser
        .parse(source, None)
        .context("failed to parse C++ file")?;

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

    // Extract file dependencies from #include directives
    let dependencies = collect_dependencies(path, source, &tree.root_node());

    // C++ doesn't have import bindings like TypeScript/Rust
    let import_bindings = Vec::new();

    Ok((symbols, edges, references, dependencies, import_bindings))
}

/// Extract file dependencies from #include directives
fn collect_dependencies(path: &Path, source: &str, root: &Node) -> Vec<FileDependency> {
    let mut dependencies = Vec::new();
    let mut seen = HashSet::new();
    let from_file = normalize_path(path);
    let parent = path.parent();

    let mut stack = vec![*root];
    while let Some(node) = stack.pop() {
        // Handle #include directives
        if node.kind() == "preproc_include" {
            if let Some(path_node) = node.child_by_field_name("path") {
                let include_path = slice(source, &path_node);
                // Remove quotes or angle brackets
                let cleaned_path = include_path
                    .trim_start_matches('"')
                    .trim_end_matches('"')
                    .trim_start_matches('<')
                    .trim_end_matches('>');

                if !cleaned_path.is_empty() && !seen.contains(cleaned_path) {
                    seen.insert(cleaned_path.to_string());

                    // Only resolve relative includes (with quotes)
                    if include_path.starts_with('"') {
                        if let Some(parent_dir) = parent {
                            let include_file = parent_dir.join(cleaned_path);
                            if include_file.exists() {
                                dependencies.push(FileDependency {
                                    from_file: from_file.clone(),
                                    to_file: normalize_path(&include_file),
                                    kind: "include".to_string(),
                                });
                            }
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

#[allow(clippy::too_many_arguments)]
fn walk_symbols(
    path: &Path,
    source: &str,
    cursor: &mut TreeCursor,
    container: Option<String>,
    namespace_path: &[String],
    symbols: &mut Vec<SymbolRecord>,
    edges: &mut Vec<EdgeRecord>,
    declared_spans: &mut HashSet<(usize, usize)>,
    symbol_by_name: &mut HashMap<String, SymbolBinding>,
) {
    loop {
        let node = cursor.node();
        match node.kind() {
            // Functions (regular and definitions)
            "function_definition" => {
                if let Some(declarator) = node.child_by_field_name("declarator") {
                    if let Some(name) = extract_function_name(source, &declarator) {
                        let sym = make_symbol(
                            path,
                            namespace_path,
                            &node,
                            &name,
                            "function",
                            container.clone(),
                            source.as_bytes(),
                        );
                        declared_spans.insert((sym.start as usize, sym.end as usize));
                        symbol_by_name
                            .entry(name)
                            .or_insert_with(|| SymbolBinding::from(&sym));
                        symbols.push(sym);
                    }
                }
            }
            // Function declarations (prototypes)
            "declaration" => {
                // Check if this is a function declaration
                if let Some(declarator) = node.child_by_field_name("declarator") {
                    if declarator.kind() == "function_declarator" {
                        if let Some(name) = extract_function_name(source, &declarator) {
                            let sym = make_symbol(
                                path,
                                namespace_path,
                                &node,
                                &name,
                                "function",
                                container.clone(),
                                source.as_bytes(),
                            );
                            declared_spans.insert((sym.start as usize, sym.end as usize));
                            symbol_by_name
                                .entry(name)
                                .or_insert_with(|| SymbolBinding::from(&sym));
                            symbols.push(sym);
                        }
                    }
                }
            }
            // Classes
            "class_specifier" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = slice(source, &name_node);
                    if !name.is_empty() {
                        let sym = make_symbol(
                            path,
                            namespace_path,
                            &node,
                            &name,
                            "class",
                            container.clone(),
                            source.as_bytes(),
                        );
                        declared_spans.insert((sym.start as usize, sym.end as usize));
                        symbol_by_name
                            .entry(name.clone())
                            .or_insert_with(|| SymbolBinding::from(&sym));

                        // Record inheritance edges
                        record_inheritance_edges(source, &node, &sym.id, edges);

                        symbols.push(sym);

                        // Recurse into class body with class as container
                        if cursor.goto_first_child() {
                            walk_symbols(
                                path,
                                source,
                                cursor,
                                Some(name),
                                namespace_path,
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
            }
            // Structs
            "struct_specifier" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = slice(source, &name_node);
                    if !name.is_empty() {
                        let sym = make_symbol(
                            path,
                            namespace_path,
                            &node,
                            &name,
                            "struct",
                            container.clone(),
                            source.as_bytes(),
                        );
                        declared_spans.insert((sym.start as usize, sym.end as usize));
                        symbol_by_name
                            .entry(name.clone())
                            .or_insert_with(|| SymbolBinding::from(&sym));

                        // Record inheritance edges
                        record_inheritance_edges(source, &node, &sym.id, edges);

                        symbols.push(sym);

                        // Recurse into struct body with struct as container
                        if cursor.goto_first_child() {
                            walk_symbols(
                                path,
                                source,
                                cursor,
                                Some(name),
                                namespace_path,
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
            }
            // Enums
            "enum_specifier" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = slice(source, &name_node);
                    if !name.is_empty() {
                        let sym = make_symbol(
                            path,
                            namespace_path,
                            &node,
                            &name,
                            "enum",
                            container.clone(),
                            source.as_bytes(),
                        );
                        declared_spans.insert((sym.start as usize, sym.end as usize));
                        symbol_by_name
                            .entry(name)
                            .or_insert_with(|| SymbolBinding::from(&sym));
                        symbols.push(sym);
                    }
                }
            }
            // Type aliases (typedef and using)
            "type_definition" | "alias_declaration" => {
                if let Some(name_node) = node.child_by_field_name("declarator") {
                    let name = slice(source, &name_node);
                    if !name.is_empty() {
                        let sym = make_symbol(
                            path,
                            namespace_path,
                            &node,
                            &name,
                            "type",
                            container.clone(),
                            source.as_bytes(),
                        );
                        declared_spans.insert((sym.start as usize, sym.end as usize));
                        symbol_by_name
                            .entry(name)
                            .or_insert_with(|| SymbolBinding::from(&sym));
                        symbols.push(sym);
                    }
                } else if let Some(name_node) = node.child_by_field_name("name") {
                    let name = slice(source, &name_node);
                    if !name.is_empty() {
                        let sym = make_symbol(
                            path,
                            namespace_path,
                            &node,
                            &name,
                            "type",
                            container.clone(),
                            source.as_bytes(),
                        );
                        declared_spans.insert((sym.start as usize, sym.end as usize));
                        symbol_by_name
                            .entry(name)
                            .or_insert_with(|| SymbolBinding::from(&sym));
                        symbols.push(sym);
                    }
                }
            }
            // Namespaces
            "namespace_definition" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = slice(source, &name_node);
                    if !name.is_empty() {
                        let mut ns_path = namespace_path.to_vec();
                        ns_path.push(name);
                        if cursor.goto_first_child() {
                            walk_symbols(
                                path,
                                source,
                                cursor,
                                container.clone(),
                                &ns_path,
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
            }
            // Template declarations - extract the underlying declaration
            "template_declaration" => {
                if cursor.goto_first_child() {
                    walk_symbols(
                        path,
                        source,
                        cursor,
                        container.clone(),
                        namespace_path,
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
            // Field declarations (methods in classes/structs)
            "field_declaration" => {
                if let Some(declarator) = node.child_by_field_name("declarator") {
                    if declarator.kind() == "function_declarator" {
                        if let Some(name) = extract_function_name(source, &declarator) {
                            let kind = if container.is_some() {
                                "method"
                            } else {
                                "function"
                            };
                            let sym = make_symbol(
                                path,
                                namespace_path,
                                &node,
                                &name,
                                kind,
                                container.clone(),
                                source.as_bytes(),
                            );
                            declared_spans.insert((sym.start as usize, sym.end as usize));
                            symbol_by_name
                                .entry(name)
                                .or_insert_with(|| SymbolBinding::from(&sym));
                            symbols.push(sym);
                        }
                    }
                }
            }
            _ => {}
        }

        if cursor.goto_first_child() {
            walk_symbols(
                path,
                source,
                cursor,
                container.clone(),
                namespace_path,
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

/// Extract function name from a function_declarator node
fn extract_function_name(source: &str, declarator: &Node) -> Option<String> {
    // function_declarator has a "declarator" field that contains the name
    // It could be an identifier, or a qualified_identifier for methods
    if let Some(name_node) = declarator.child_by_field_name("declarator") {
        let name = slice(source, &name_node);
        // For qualified names like ClassName::methodName, extract the method name
        if name.contains("::") {
            return name.rsplit("::").next().map(|s| s.to_string());
        }
        if !name.is_empty() {
            return Some(name);
        }
    }

    // Try to find identifier directly in children
    let mut cursor = declarator.walk();
    for child in declarator.children(&mut cursor) {
        if child.kind() == "identifier" || child.kind() == "field_identifier" {
            let name = slice(source, &child);
            if !name.is_empty() {
                return Some(name);
            }
        }
        // Handle qualified identifiers
        if child.kind() == "qualified_identifier" {
            let name = slice(source, &child);
            if name.contains("::") {
                return name.rsplit("::").next().map(|s| s.to_string());
            }
            if !name.is_empty() {
                return Some(name);
            }
        }
    }

    None
}

/// Record inheritance edges from base_class_clause
fn record_inheritance_edges(
    source: &str,
    node: &Node,
    class_id: &str,
    edges: &mut Vec<EdgeRecord>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "base_class_clause" {
            // In tree-sitter-cpp, base_class_clause children are:
            // - ":" token
            // - access_specifier (optional: public, private, protected)
            // - type_identifier or qualified_identifier or template_type
            let mut inner_cursor = child.walk();
            for base_child in child.children(&mut inner_cursor) {
                match base_child.kind() {
                    "type_identifier" | "qualified_identifier" | "template_type" => {
                        let base_name = slice(source, &base_child);
                        if !base_name.is_empty() {
                            edges.push(EdgeRecord {
                                src: class_id.to_string(),
                                dst: base_name,
                                kind: "extends".to_string(),
                            });
                        }
                    }
                    // Also check for base_class_specifier in case the grammar varies
                    "base_class_specifier" => {
                        let mut specifier_cursor = base_child.walk();
                        for type_child in base_child.children(&mut specifier_cursor) {
                            if matches!(
                                type_child.kind(),
                                "type_identifier" | "qualified_identifier" | "template_type"
                            ) {
                                let base_name = slice(source, &type_child);
                                if !base_name.is_empty() {
                                    edges.push(EdgeRecord {
                                        src: class_id.to_string(),
                                        dst: base_name,
                                        kind: "extends".to_string(),
                                    });
                                }
                                break;
                            }
                        }
                    }
                    _ => {}
                }
            }
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
        // Track identifier references
        if node.kind() == "identifier" || node.kind() == "type_identifier" {
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

fn make_symbol(
    path: &Path,
    namespace_path: &[String],
    node: &Node,
    name: &str,
    kind: &str,
    container: Option<String>,
    source: &[u8],
) -> SymbolRecord {
    let qualifier = Some(namespace_qualifier(path, namespace_path, &container));
    let visibility = extract_visibility(node, path);
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
        is_test: false, // C++ test detection not yet implemented
    }
}

fn namespace_qualifier(
    path: &Path,
    namespace_path: &[String],
    container: &Option<String>,
) -> String {
    let mut base = normalize_path(path);
    // Remove extension
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        let trim = ext.len() + 1;
        if base.len() > trim {
            base.truncate(base.len() - trim);
        }
    }
    for segment in namespace_path {
        base.push_str("::");
        base.push_str(segment);
    }
    if let Some(c) = container {
        base.push_str("::");
        base.push_str(c);
    }
    base
}

/// Extract visibility from access specifiers
fn extract_visibility(node: &Node, path: &Path) -> Option<String> {
    // Check for access specifier in parent context
    // For class members, look for preceding access specifier
    if let Some(parent) = node.parent() {
        let mut cursor = parent.walk();
        let mut current_visibility: Option<String> = None;

        for child in parent.children(&mut cursor) {
            if child.kind() == "access_specifier" {
                // Extract the visibility keyword (public, private, protected)
                let text = slice_bytes(path, &child);
                let vis = text.trim_end_matches(':').trim();
                if !vis.is_empty() {
                    current_visibility = Some(vis.to_string());
                }
            }
            if child.id() == node.id() {
                return current_visibility;
            }
        }
    }
    None
}

fn slice_bytes(path: &Path, node: &Node) -> String {
    let source = std::fs::read_to_string(path).unwrap_or_default();
    slice(&source, node)
}

// ============================================================================
// LanguageParser trait implementation
// ============================================================================

use super::traits::{LanguageConfig, LanguageParser, ParseResult};

/// C++ language parser implementing the `LanguageParser` trait.
#[derive(Clone)]
pub struct CppParser;

impl CppParser {
    /// Create a new C++ parser.
    pub fn new() -> Self {
        Self
    }
}

impl Default for CppParser {
    fn default() -> Self {
        Self::new()
    }
}

impl LanguageParser for CppParser {
    fn config(&self) -> LanguageConfig {
        LanguageConfig {
            name: "C++",
            extensions: &["cpp", "cc", "cxx", "c++", "hpp", "hh", "hxx", "h++"],
        }
    }

    fn language(&self) -> &Language {
        &CPP_LANGUAGE
    }

    fn parse(&self, path: &Path, source: &str) -> Result<ParseResult> {
        index_file(path, source).map(ParseResult::from_tuple)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn extracts_cpp_functions_and_classes() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.cpp");
        let source = r#"
            namespace MyNamespace {
                class MyClass {
                public:
                    void doSomething();
                private:
                    int value;
                };

                void MyClass::doSomething() {
                    // implementation
                }

                struct Point {
                    int x;
                    int y;
                };

                enum Color {
                    Red,
                    Green,
                    Blue
                };
            }

            void freeFunction(int arg) {
                return;
            }
        "#;
        fs::write(&path, source).unwrap();

        let (symbols, edges, _refs, _deps, _imports) = index_file(&path, source).unwrap();
        let names: Vec<_> = symbols.iter().map(|s| s.name.as_str()).collect();

        assert!(names.contains(&"MyClass"), "Should find MyClass");
        assert!(
            names.contains(&"doSomething"),
            "Should find doSomething method"
        );
        assert!(names.contains(&"Point"), "Should find Point struct");
        assert!(names.contains(&"Color"), "Should find Color enum");
        assert!(names.contains(&"freeFunction"), "Should find freeFunction");

        let my_class = symbols.iter().find(|s| s.name == "MyClass").unwrap();
        assert_eq!(my_class.kind, "class");
        assert!(
            my_class
                .qualifier
                .as_deref()
                .unwrap()
                .contains("MyNamespace"),
            "MyClass should have MyNamespace in qualifier"
        );

        let do_something = symbols.iter().find(|s| s.name == "doSomething").unwrap();
        assert!(
            do_something.kind == "method" || do_something.kind == "function",
            "doSomething should be method or function"
        );

        // Check we don't have edges for inheritance (MyClass has no base)
        let my_class_extends: Vec<_> = edges
            .iter()
            .filter(|e| e.src == my_class.id && e.kind == "extends")
            .collect();
        assert!(my_class_extends.is_empty(), "MyClass has no base class");
    }

    #[test]
    fn captures_inheritance_relationships() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("inherit.cpp");
        let source = r#"
            class Base {
            public:
                virtual void doIt() = 0;
            };

            class Derived : public Base {
            public:
                void doIt() override {}
            };
        "#;
        fs::write(&path, source).unwrap();

        let (symbols, edges, _refs, _deps, _imports) = index_file(&path, source).unwrap();

        let base = symbols.iter().find(|s| s.name == "Base");
        let derived = symbols.iter().find(|s| s.name == "Derived");

        assert!(base.is_some(), "Should find Base class");
        assert!(derived.is_some(), "Should find Derived class");

        let derived_id = &derived.unwrap().id;
        let extends_edges: Vec<_> = edges
            .iter()
            .filter(|e| e.src == *derived_id && e.kind == "extends")
            .collect();

        assert!(
            !extends_edges.is_empty(),
            "Should have extends edge from Derived"
        );
        assert!(
            extends_edges[0].dst.contains("Base"),
            "Derived should extend Base"
        );
    }

    #[test]
    fn handles_templates() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("template.cpp");
        let source = r#"
            template<typename T>
            class Container {
            public:
                void add(T item);
                T get(int index);
            };

            template<typename T>
            void swap(T& a, T& b) {
                T temp = a;
                a = b;
                b = temp;
            }
        "#;
        fs::write(&path, source).unwrap();

        let (symbols, _edges, _refs, _deps, _imports) = index_file(&path, source).unwrap();
        let names: Vec<_> = symbols.iter().map(|s| s.name.as_str()).collect();

        assert!(
            names.contains(&"Container"),
            "Should find Container template class"
        );
        assert!(
            names.contains(&"swap"),
            "Should find swap template function"
        );
    }
}
