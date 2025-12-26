use crate::languages::{slice, ImportBindingInfo};
use crate::store::{normalize_path, EdgeRecord, FileDependency, ReferenceRecord, SymbolRecord};
use anyhow::{Context, Result};
use once_cell::sync::Lazy;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use tree_sitter::{Language, Node, Parser, TreeCursor};

static PYTHON_LANGUAGE: Lazy<Language> = Lazy::new(|| tree_sitter_python::LANGUAGE.into());

/// Index a Python file, returning symbols, edges, references, file dependencies, and import bindings.
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
        .set_language(&PYTHON_LANGUAGE)
        .context("failed to set Python language")?;
    let tree = parser
        .parse(source, None)
        .context("failed to parse Python file")?;

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
            "class_definition" => {
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
            "function_definition" => {
                handle_function(
                    path,
                    source,
                    &node,
                    container.clone(),
                    symbols,
                    declared_spans,
                    symbol_by_name,
                    false, // not a decorated definition
                );
            }
            "decorated_definition" => {
                handle_decorated_definition(
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
            "expression_statement" => {
                // Check for module-level assignments (constants/variables)
                if container.is_none() {
                    handle_assignment(path, source, &node, symbols, declared_spans, symbol_by_name);
                }
            }
            "type_alias_statement" => {
                // Python 3.12+ type aliases: type X = ...
                handle_type_alias(
                    path,
                    source,
                    &node,
                    container.clone(),
                    symbols,
                    declared_spans,
                    symbol_by_name,
                );
            }
            _ => {}
        }

        // Recurse into children for class/function bodies
        if cursor.goto_first_child() {
            let child_container = match node.kind() {
                "class_definition" => find_name(&node, source).or(container.clone()),
                "decorated_definition" => {
                    // For decorated classes, get the class name
                    if let Some(class_node) = find_class_in_decorated(&node) {
                        find_name(&class_node, source).or(container.clone())
                    } else {
                        container.clone()
                    }
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

/// Find class_definition within a decorated_definition
#[allow(clippy::manual_find)]
fn find_class_in_decorated<'a>(node: &'a Node<'a>) -> Option<Node<'a>> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "class_definition" {
            return Some(child);
        }
    }
    None
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
        let span = (node.start_byte(), node.end_byte());
        // Check if already declared (e.g., from decorated_definition)
        if declared_spans.contains(&span) {
            return;
        }

        // Detect special class types (Protocol, TypedDict, etc.)
        let kind = determine_class_kind(node, source);

        let sym = make_symbol(
            path,
            node,
            &name,
            &kind,
            container.clone(),
            source.as_bytes(),
            false,
        );
        declared_spans.insert(span);
        symbol_by_name.insert(name.clone(), sym.id.clone());

        // Record inheritance edges
        record_inheritance_edges(path, source, node, &sym.id, edges, symbol_by_name);

        symbols.push(sym);
    }
}

/// Determine the kind of a class (class, protocol, typeddict, etc.)
fn determine_class_kind(node: &Node, source: &str) -> String {
    // Check argument_list for base classes
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "argument_list" {
            let bases_text = slice(source, &child);
            if bases_text.contains("Protocol") {
                return "protocol".to_string();
            }
            if bases_text.contains("TypedDict") {
                return "typeddict".to_string();
            }
            if bases_text.contains("Enum") {
                return "enum".to_string();
            }
            if bases_text.contains("ABC") || bases_text.contains("ABCMeta") {
                return "abstract_class".to_string();
            }
        }
    }
    "class".to_string()
}

/// Record extends/implements edges from base classes
fn record_inheritance_edges(
    path: &Path,
    source: &str,
    node: &Node,
    src_id: &str,
    edges: &mut Vec<EdgeRecord>,
    symbol_by_name: &HashMap<String, String>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "argument_list" {
            // Extract base classes from the argument list
            let mut arg_cursor = child.walk();
            for arg in child.children(&mut arg_cursor) {
                if arg.kind() == "identifier" {
                    let base_name = slice(source, &arg);
                    if !base_name.is_empty() {
                        let dst_id = symbol_by_name
                            .get(&base_name)
                            .cloned()
                            .unwrap_or_else(|| format!("{}#{}", normalize_path(path), base_name));
                        edges.push(EdgeRecord {
                            src: src_id.to_string(),
                            dst: dst_id,
                            kind: "extends".to_string(),
                        });
                    }
                } else if arg.kind() == "attribute" {
                    // Handle qualified names like abc.ABC
                    let base_name = slice(source, &arg);
                    if !base_name.is_empty() {
                        edges.push(EdgeRecord {
                            src: src_id.to_string(),
                            dst: base_name.clone(),
                            kind: "extends".to_string(),
                        });
                    }
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_decorated_definition(
    path: &Path,
    source: &str,
    node: &Node,
    container: Option<String>,
    symbols: &mut Vec<SymbolRecord>,
    edges: &mut Vec<EdgeRecord>,
    declared_spans: &mut HashSet<(usize, usize)>,
    symbol_by_name: &mut HashMap<String, String>,
) {
    // Collect decorators
    let decorators = collect_decorators(node, source);

    // Find the definition (function or class)
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "function_definition" => {
                handle_function(
                    path,
                    source,
                    &child,
                    container.clone(),
                    symbols,
                    declared_spans,
                    symbol_by_name,
                    is_test_decorator(&decorators),
                );
            }
            "class_definition" => {
                handle_class(
                    path,
                    source,
                    &child,
                    container.clone(),
                    symbols,
                    edges,
                    declared_spans,
                    symbol_by_name,
                );
            }
            _ => {}
        }
    }
}

/// Collect decorator names from a decorated_definition
fn collect_decorators(node: &Node, source: &str) -> Vec<String> {
    let mut decorators = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "decorator" {
            // Find the identifier, call, or attribute inside the decorator
            let mut dec_cursor = child.walk();
            for dec_child in child.children(&mut dec_cursor) {
                if matches!(dec_child.kind(), "identifier" | "call" | "attribute") {
                    decorators.push(slice(source, &dec_child));
                }
            }
        }
    }
    decorators
}

/// Check if any decorator indicates a test function
fn is_test_decorator(decorators: &[String]) -> bool {
    decorators.iter().any(|d| {
        d.contains("test")
            || d.contains("pytest")
            || d.contains("unittest")
            || d.contains("fixture")
            || d.contains("parametrize")
    })
}

#[allow(clippy::too_many_arguments)]
fn handle_function(
    path: &Path,
    source: &str,
    node: &Node,
    container: Option<String>,
    symbols: &mut Vec<SymbolRecord>,
    declared_spans: &mut HashSet<(usize, usize)>,
    symbol_by_name: &mut HashMap<String, String>,
    is_decorated_test: bool,
) {
    if let Some(name) = find_name(node, source) {
        // Determine function kind
        let kind = if container.is_some() {
            if name.starts_with("__") && name.ends_with("__") {
                "dunder_method" // __init__, __str__, etc.
            } else if is_property_method(node, source) {
                "property"
            } else if is_classmethod(node, source) {
                "classmethod"
            } else if is_staticmethod(node, source) {
                "staticmethod"
            } else {
                "method"
            }
        } else {
            "function"
        };

        // Detect test functions
        let is_test = is_decorated_test
            || name.starts_with("test_")
            || name.starts_with("test")
            || (container.is_some() && name.starts_with("test"));

        let span = (node.start_byte(), node.end_byte());
        // Check if already declared (e.g., from decorated_definition)
        if declared_spans.contains(&span) {
            return;
        }

        let sym = make_symbol(
            path,
            node,
            &name,
            kind,
            container.clone(),
            source.as_bytes(),
            is_test,
        );
        declared_spans.insert(span);
        symbol_by_name.insert(name.clone(), sym.id.clone());
        symbols.push(sym);
    }
}

/// Check if this is a @property decorated method
fn is_property_method(_node: &Node, _source: &str) -> bool {
    // The decorator check happens in handle_decorated_definition
    // For now, we rely on @property decorator detection
    false
}

/// Check if this has @classmethod decorator
fn is_classmethod(_node: &Node, _source: &str) -> bool {
    false
}

/// Check if this has @staticmethod decorator
fn is_staticmethod(_node: &Node, _source: &str) -> bool {
    false
}

/// Handle module-level assignments (constants/variables)
fn handle_assignment(
    path: &Path,
    source: &str,
    node: &Node,
    symbols: &mut Vec<SymbolRecord>,
    declared_spans: &mut HashSet<(usize, usize)>,
    symbol_by_name: &mut HashMap<String, String>,
) {
    // Look for assignment inside expression_statement
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "assignment" {
            if let Some(left) = child.child_by_field_name("left") {
                if left.kind() == "identifier" {
                    let name = slice(source, &left);
                    if !name.is_empty() {
                        let span = (child.start_byte(), child.end_byte());
                        // Check if already declared
                        if declared_spans.contains(&span) {
                            continue;
                        }

                        // Determine if it's a constant (UPPER_CASE) or variable
                        let kind = if name.chars().all(|c| c.is_uppercase() || c == '_') {
                            "const"
                        } else {
                            "variable"
                        };

                        let sym =
                            make_symbol(path, &child, &name, kind, None, source.as_bytes(), false);
                        declared_spans.insert(span);
                        symbol_by_name.insert(name.clone(), sym.id.clone());
                        symbols.push(sym);
                    }
                }
            }
        }
    }
}

/// Handle type alias statements (Python 3.12+)
fn handle_type_alias(
    path: &Path,
    source: &str,
    node: &Node,
    container: Option<String>,
    symbols: &mut Vec<SymbolRecord>,
    declared_spans: &mut HashSet<(usize, usize)>,
    symbol_by_name: &mut HashMap<String, String>,
) {
    if let Some(name_node) = node.child_by_field_name("name") {
        let name = slice(source, &name_node);
        if !name.is_empty() {
            let span = (node.start_byte(), node.end_byte());
            // Check if already declared
            if declared_spans.contains(&span) {
                return;
            }

            let sym = make_symbol(
                path,
                node,
                &name,
                "type",
                container,
                source.as_bytes(),
                false,
            );
            declared_spans.insert(span);
            symbol_by_name.insert(name.clone(), sym.id.clone());
            symbols.push(sym);
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
        match node.kind() {
            "import_statement" => {
                // import foo, bar
                handle_import_statement(
                    &node,
                    source,
                    &from_file,
                    &mut dependencies,
                    &mut import_bindings,
                );
            }
            "import_from_statement" => {
                // from foo import bar
                handle_import_from_statement(
                    &node,
                    source,
                    &from_file,
                    &mut dependencies,
                    &mut import_bindings,
                );
            }
            _ => {
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    stack.push(child);
                }
            }
        }
    }

    (dependencies, import_bindings)
}

/// Handle `import foo` or `import foo as bar`
fn handle_import_statement(
    node: &Node,
    source: &str,
    from_file: &str,
    dependencies: &mut Vec<FileDependency>,
    import_bindings: &mut Vec<ImportBindingInfo>,
) {
    let import_text = slice(source, node);

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "dotted_name" => {
                let module_name = slice(source, &child);
                if !module_name.is_empty() {
                    dependencies.push(FileDependency {
                        from_file: from_file.to_string(),
                        to_file: module_name.clone(),
                        kind: "import".to_string(),
                    });
                    import_bindings.push(ImportBindingInfo {
                        local_name: module_name
                            .split('.')
                            .next()
                            .unwrap_or(&module_name)
                            .to_string(),
                        source_file: from_file.to_string(),
                        original_name: module_name.clone(),
                        import_text: import_text.clone(),
                    });
                }
            }
            "aliased_import" => {
                let mut alias_cursor = child.walk();
                let mut module_name = String::new();
                let mut alias = String::new();

                for alias_child in child.children(&mut alias_cursor) {
                    match alias_child.kind() {
                        "dotted_name" => {
                            module_name = slice(source, &alias_child);
                        }
                        "identifier" if alias.is_empty() && !module_name.is_empty() => {
                            alias = slice(source, &alias_child);
                        }
                        _ => {}
                    }
                }

                if !module_name.is_empty() {
                    dependencies.push(FileDependency {
                        from_file: from_file.to_string(),
                        to_file: module_name.clone(),
                        kind: "import".to_string(),
                    });
                    let local_name = if alias.is_empty() {
                        module_name
                            .split('.')
                            .next()
                            .unwrap_or(&module_name)
                            .to_string()
                    } else {
                        alias
                    };
                    import_bindings.push(ImportBindingInfo {
                        local_name,
                        source_file: from_file.to_string(),
                        original_name: module_name,
                        import_text: import_text.clone(),
                    });
                }
            }
            _ => {}
        }
    }
}

/// Handle `from foo import bar` or `from foo import bar as baz`
fn handle_import_from_statement(
    node: &Node,
    source: &str,
    from_file: &str,
    dependencies: &mut Vec<FileDependency>,
    import_bindings: &mut Vec<ImportBindingInfo>,
) {
    let import_text = slice(source, node);
    let mut module_name = String::new();
    let mut found_module = false;

    // First find the module name (first dotted_name or relative_import after 'from')
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "dotted_name" || child.kind() == "relative_import" {
            if !found_module {
                module_name = slice(source, &child);
                found_module = true;
            } else {
                // Second dotted_name is the imported name (for simple imports like `from X import Y`)
                let name = slice(source, &child);
                if !name.is_empty() {
                    import_bindings.push(ImportBindingInfo {
                        local_name: name.clone(),
                        source_file: from_file.to_string(),
                        original_name: name,
                        import_text: import_text.clone(),
                    });
                }
            }
        }
    }

    if module_name.is_empty() {
        return;
    }

    dependencies.push(FileDependency {
        from_file: from_file.to_string(),
        to_file: module_name.clone(),
        kind: "import".to_string(),
    });

    // Now find imported names - they can be in various structures
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            // Direct identifier after 'import' keyword
            "dotted_name" if found_module => {
                // Already handled above (it's the imported name for simple imports)
            }
            // Aliased import: `from X import Y as Z`
            "aliased_import" => {
                extract_aliased_import(&child, source, from_file, &import_text, import_bindings);
            }
            // Wildcard import: `from X import *`
            "wildcard_import" => {
                import_bindings.push(ImportBindingInfo {
                    local_name: "*".to_string(),
                    source_file: from_file.to_string(),
                    original_name: "*".to_string(),
                    import_text: import_text.clone(),
                });
            }
            // Walk into structures that might contain import names
            _ => {
                // Recurse to find identifiers and aliased_imports
                let mut stack = vec![child];
                while let Some(n) = stack.pop() {
                    match n.kind() {
                        "dotted_name"
                            if n.parent().map(|p| p.kind()) != Some("import_from_statement") =>
                        {
                            // This is an imported name inside a nested structure
                            let name = slice(source, &n);
                            if !name.is_empty() && name != module_name {
                                import_bindings.push(ImportBindingInfo {
                                    local_name: name.clone(),
                                    source_file: from_file.to_string(),
                                    original_name: name,
                                    import_text: import_text.clone(),
                                });
                            }
                        }
                        "aliased_import" => {
                            extract_aliased_import(
                                &n,
                                source,
                                from_file,
                                &import_text,
                                import_bindings,
                            );
                        }
                        _ => {
                            let mut inner_cursor = n.walk();
                            for inner_child in n.children(&mut inner_cursor) {
                                stack.push(inner_child);
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Extract aliased import (e.g., `Y as Z` from `from X import Y as Z`)
fn extract_aliased_import(
    node: &Node,
    source: &str,
    from_file: &str,
    import_text: &str,
    import_bindings: &mut Vec<ImportBindingInfo>,
) {
    let mut cursor = node.walk();
    let mut original_name = String::new();
    let mut local_name = String::new();

    for child in node.children(&mut cursor) {
        match child.kind() {
            "dotted_name" | "identifier" => {
                if original_name.is_empty() {
                    original_name = slice(source, &child);
                } else if local_name.is_empty() {
                    local_name = slice(source, &child);
                }
            }
            _ => {}
        }
    }

    if !original_name.is_empty() {
        if local_name.is_empty() {
            local_name = original_name.clone();
        }
        import_bindings.push(ImportBindingInfo {
            local_name,
            source_file: from_file.to_string(),
            original_name,
            import_text: import_text.to_string(),
        });
    }
}

/// Find the name of a definition node (class or function)
fn find_name(node: &Node, source: &str) -> Option<String> {
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

fn make_symbol(
    path: &Path,
    node: &Node,
    name: &str,
    kind: &str,
    container: Option<String>,
    source: &[u8],
    is_test: bool,
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
        visibility: None, // Python doesn't have explicit visibility modifiers
        container,
        content_hash,
        is_test,
    }
}

// ============================================================================
// LanguageParser trait implementation
// ============================================================================

use super::traits::{LanguageConfig, LanguageParser, ParseResult};

/// Python language parser implementing the `LanguageParser` trait.
#[derive(Clone)]
pub struct PythonParser;

impl PythonParser {
    /// Create a new Python parser.
    pub fn new() -> Self {
        Self
    }
}

impl Default for PythonParser {
    fn default() -> Self {
        Self::new()
    }
}

impl LanguageParser for PythonParser {
    fn config(&self) -> LanguageConfig {
        LanguageConfig {
            name: "Python",
            extensions: &["py", "pyi"],
        }
    }

    fn language(&self) -> &Language {
        &PYTHON_LANGUAGE
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
    fn extracts_python_symbols() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.py");
        let source = r#"
class Person:
    def __init__(self, name):
        self.name = name

    def greet(self):
        print(f"Hello, {self.name}")

def top_level_function():
    pass

MAX_SIZE = 100
config = {"key": "value"}
"#;
        fs::write(&path, source).unwrap();

        let (symbols, _edges, _refs, _deps, _imports) = index_file(&path, source).unwrap();
        let names: Vec<_> = symbols.iter().map(|s| s.name.as_str()).collect();

        assert!(names.contains(&"Person"), "Should find Person class");
        assert!(names.contains(&"__init__"), "Should find __init__ method");
        assert!(names.contains(&"greet"), "Should find greet method");
        assert!(
            names.contains(&"top_level_function"),
            "Should find top_level_function"
        );
        assert!(names.contains(&"MAX_SIZE"), "Should find MAX_SIZE constant");
        assert!(names.contains(&"config"), "Should find config variable");
    }

    #[test]
    fn extracts_class_kinds() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("types.py");
        let source = r#"
from typing import Protocol, TypedDict
from enum import Enum
from abc import ABC

class MyProtocol(Protocol):
    def method(self) -> None: ...

class MyTypedDict(TypedDict):
    name: str
    age: int

class Color(Enum):
    RED = 1
    GREEN = 2

class AbstractBase(ABC):
    pass

class RegularClass:
    pass
"#;
        fs::write(&path, source).unwrap();

        let (symbols, _edges, _refs, _deps, _imports) = index_file(&path, source).unwrap();

        let protocol = symbols.iter().find(|s| s.name == "MyProtocol").unwrap();
        assert_eq!(protocol.kind, "protocol", "MyProtocol should be a protocol");

        let typed_dict = symbols.iter().find(|s| s.name == "MyTypedDict").unwrap();
        assert_eq!(
            typed_dict.kind, "typeddict",
            "MyTypedDict should be a typeddict"
        );

        let color = symbols.iter().find(|s| s.name == "Color").unwrap();
        assert_eq!(color.kind, "enum", "Color should be an enum");

        let abstract_base = symbols.iter().find(|s| s.name == "AbstractBase").unwrap();
        assert_eq!(
            abstract_base.kind, "abstract_class",
            "AbstractBase should be an abstract_class"
        );

        let regular = symbols.iter().find(|s| s.name == "RegularClass").unwrap();
        assert_eq!(regular.kind, "class", "RegularClass should be a class");
    }

    #[test]
    fn extracts_function_kinds() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("methods.py");
        let source = r#"
class MyClass:
    def regular_method(self):
        pass

    def __init__(self):
        pass

    def __str__(self):
        return "MyClass"

def standalone_function():
    pass
"#;
        fs::write(&path, source).unwrap();

        let (symbols, _edges, _refs, _deps, _imports) = index_file(&path, source).unwrap();

        let regular = symbols.iter().find(|s| s.name == "regular_method").unwrap();
        assert_eq!(regular.kind, "method", "regular_method should be a method");
        assert_eq!(
            regular.container.as_deref(),
            Some("MyClass"),
            "regular_method should be inside MyClass"
        );

        let init = symbols.iter().find(|s| s.name == "__init__").unwrap();
        assert_eq!(
            init.kind, "dunder_method",
            "__init__ should be a dunder_method"
        );

        let str_method = symbols.iter().find(|s| s.name == "__str__").unwrap();
        assert_eq!(
            str_method.kind, "dunder_method",
            "__str__ should be a dunder_method"
        );

        let standalone = symbols
            .iter()
            .find(|s| s.name == "standalone_function")
            .unwrap();
        assert_eq!(
            standalone.kind, "function",
            "standalone_function should be a function"
        );
        assert!(
            standalone.container.is_none(),
            "standalone_function should not have a container"
        );
    }

    #[test]
    fn detects_test_functions() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test_example.py");
        let source = r#"
import pytest

def test_basic():
    assert True

def test_another():
    pass

def helper_function():
    pass

class TestSuite:
    def test_in_class(self):
        pass

    def helper_method(self):
        pass

@pytest.fixture
def my_fixture():
    return 42
"#;
        fs::write(&path, source).unwrap();

        let (symbols, _edges, _refs, _deps, _imports) = index_file(&path, source).unwrap();

        let test_basic = symbols.iter().find(|s| s.name == "test_basic").unwrap();
        assert!(test_basic.is_test, "test_basic should be marked as test");

        let test_another = symbols.iter().find(|s| s.name == "test_another").unwrap();
        assert!(
            test_another.is_test,
            "test_another should be marked as test"
        );

        let helper = symbols
            .iter()
            .find(|s| s.name == "helper_function")
            .unwrap();
        assert!(
            !helper.is_test,
            "helper_function should not be marked as test"
        );

        let test_in_class = symbols.iter().find(|s| s.name == "test_in_class").unwrap();
        assert!(
            test_in_class.is_test,
            "test_in_class should be marked as test"
        );

        let helper_method = symbols.iter().find(|s| s.name == "helper_method").unwrap();
        assert!(
            !helper_method.is_test,
            "helper_method should not be marked as test"
        );

        let fixture = symbols.iter().find(|s| s.name == "my_fixture").unwrap();
        assert!(
            fixture.is_test,
            "my_fixture should be marked as test (pytest fixture)"
        );
    }

    #[test]
    fn extracts_imports() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("imports.py");
        let source = r#"
import os
import sys as system
from pathlib import Path
from typing import Optional, List
from collections import defaultdict as dd
from . import relative_module
from ..parent import ParentClass
"#;
        fs::write(&path, source).unwrap();

        let (_symbols, _edges, _refs, deps, imports) = index_file(&path, source).unwrap();

        // Check dependencies
        let dep_modules: Vec<_> = deps.iter().map(|d| d.to_file.as_str()).collect();
        assert!(dep_modules.contains(&"os"), "Should have os dependency");
        assert!(dep_modules.contains(&"sys"), "Should have sys dependency");
        assert!(
            dep_modules.contains(&"pathlib"),
            "Should have pathlib dependency"
        );
        assert!(
            dep_modules.contains(&"typing"),
            "Should have typing dependency"
        );

        // Check import bindings
        let local_names: Vec<_> = imports.iter().map(|i| i.local_name.as_str()).collect();
        assert!(local_names.contains(&"os"), "Should have os binding");
        assert!(local_names.contains(&"system"), "Should have system alias");
        assert!(local_names.contains(&"Path"), "Should have Path binding");
        assert!(local_names.contains(&"dd"), "Should have dd alias");
    }

    #[test]
    fn captures_inheritance_edges() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("inheritance.py");
        let source = r#"
class Animal:
    def speak(self):
        pass

class Mammal(Animal):
    pass

class Dog(Mammal):
    def speak(self):
        print("Woof!")
"#;
        fs::write(&path, source).unwrap();

        let (symbols, edges, _refs, _deps, _imports) = index_file(&path, source).unwrap();

        assert!(symbols.iter().any(|s| s.name == "Animal"));
        assert!(symbols.iter().any(|s| s.name == "Mammal"));
        assert!(symbols.iter().any(|s| s.name == "Dog"));

        // Mammal extends Animal
        assert!(
            edges.iter().any(|e| e.kind == "extends"),
            "Should have inheritance edges"
        );
    }

    #[test]
    fn extracts_constants_and_variables() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("constants.py");
        let source = r#"
MAX_CONNECTIONS = 100
DEFAULT_TIMEOUT = 30
API_URL = "https://api.example.com"

config = {}
counter = 0
"#;
        fs::write(&path, source).unwrap();

        let (symbols, _edges, _refs, _deps, _imports) = index_file(&path, source).unwrap();

        let max_conn = symbols
            .iter()
            .find(|s| s.name == "MAX_CONNECTIONS")
            .unwrap();
        assert_eq!(
            max_conn.kind, "const",
            "MAX_CONNECTIONS should be a constant"
        );

        let api_url = symbols.iter().find(|s| s.name == "API_URL").unwrap();
        assert_eq!(api_url.kind, "const", "API_URL should be a constant");

        let config = symbols.iter().find(|s| s.name == "config").unwrap();
        assert_eq!(config.kind, "variable", "config should be a variable");

        let counter = symbols.iter().find(|s| s.name == "counter").unwrap();
        assert_eq!(counter.kind, "variable", "counter should be a variable");
    }

    #[test]
    fn handles_decorated_classes() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("decorated.py");
        let source = r#"
from dataclasses import dataclass

@dataclass
class Point:
    x: int
    y: int

@dataclass(frozen=True)
class FrozenPoint:
    x: int
    y: int
"#;
        fs::write(&path, source).unwrap();

        let (symbols, _edges, _refs, _deps, _imports) = index_file(&path, source).unwrap();
        let names: Vec<_> = symbols.iter().map(|s| s.name.as_str()).collect();

        assert!(names.contains(&"Point"), "Should find Point class");
        assert!(
            names.contains(&"FrozenPoint"),
            "Should find FrozenPoint class"
        );
    }

    #[test]
    fn extracts_nested_classes() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nested.py");
        let source = r#"
class Outer:
    class Inner:
        def inner_method(self):
            pass

    def outer_method(self):
        pass
"#;
        fs::write(&path, source).unwrap();

        let (symbols, _edges, _refs, _deps, _imports) = index_file(&path, source).unwrap();

        let outer = symbols.iter().find(|s| s.name == "Outer").unwrap();
        assert!(
            outer.container.is_none(),
            "Outer should not have a container"
        );

        let inner = symbols.iter().find(|s| s.name == "Inner").unwrap();
        assert_eq!(
            inner.container.as_deref(),
            Some("Outer"),
            "Inner should be inside Outer"
        );

        let inner_method = symbols.iter().find(|s| s.name == "inner_method").unwrap();
        assert_eq!(
            inner_method.container.as_deref(),
            Some("Inner"),
            "inner_method should be inside Inner"
        );
    }

    #[test]
    fn handles_stub_files() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("types.pyi");
        let source = r#"
from typing import Optional

class MyClass:
    name: str
    value: Optional[int]

    def method(self, arg: str) -> bool: ...

def function(x: int) -> str: ...
"#;
        fs::write(&path, source).unwrap();

        let (symbols, _edges, _refs, _deps, _imports) = index_file(&path, source).unwrap();
        let names: Vec<_> = symbols.iter().map(|s| s.name.as_str()).collect();

        assert!(
            names.contains(&"MyClass"),
            "Should find MyClass in stub file"
        );
        assert!(names.contains(&"method"), "Should find method in stub file");
        assert!(
            names.contains(&"function"),
            "Should find function in stub file"
        );
    }

    #[test]
    fn no_duplicate_symbols_for_decorated_definitions() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("decorated.py");
        let source = r#"
@decorator
def decorated_function():
    pass

@classmethod
def decorated_classmethod(cls):
    pass

@dataclass
class DecoratedClass:
    x: int

@decorator1
@decorator2
def multi_decorated():
    pass
"#;
        fs::write(&path, source).unwrap();

        let (symbols, _edges, _refs, _deps, _imports) = index_file(&path, source).unwrap();

        // Count occurrences of each symbol name
        let count = |name: &str| symbols.iter().filter(|s| s.name == name).count();

        assert_eq!(
            count("decorated_function"),
            1,
            "decorated_function should appear exactly once"
        );
        assert_eq!(
            count("decorated_classmethod"),
            1,
            "decorated_classmethod should appear exactly once"
        );
        assert_eq!(
            count("DecoratedClass"),
            1,
            "DecoratedClass should appear exactly once"
        );
        assert_eq!(
            count("multi_decorated"),
            1,
            "multi_decorated should appear exactly once"
        );
    }
}
