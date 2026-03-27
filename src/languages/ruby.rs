use crate::languages::{slice, ImportBindingInfo};
use crate::store::{normalize_path, EdgeRecord, FileDependency, ReferenceRecord, SymbolRecord};
use anyhow::{Context, Result};
use once_cell::sync::Lazy;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use tree_sitter::{Language, Node, Parser, TreeCursor};

static RUBY_LANGUAGE: Lazy<Language> = Lazy::new(|| tree_sitter_ruby::LANGUAGE.into());

/// Index a Ruby file, returning symbols, edges, references, file dependencies, and import bindings.
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
        .set_language(&RUBY_LANGUAGE)
        .context("failed to set Ruby language")?;
    let tree = parser
        .parse(source, None)
        .context("failed to parse Ruby file")?;

    let mut symbols = Vec::new();
    let mut edges = Vec::new();
    let mut declared_spans: HashSet<(usize, usize)> = HashSet::new();
    let mut symbol_by_name: HashMap<String, String> = HashMap::new();

    // Track visibility state: when `private` or `protected` is called without arguments
    // in a class/module body, all subsequent methods get that visibility.
    let mut visibility_stack: Vec<Option<String>> = vec![None];

    let is_test_file = is_test_path(path);

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
            &mut visibility_stack,
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

/// Check if a file path looks like a test/spec file
fn is_test_path(path: &Path) -> bool {
    let path_str = path.to_string_lossy();
    path_str.contains("/spec/")
        || path_str.contains("/test/")
        || path_str.ends_with("_test.rb")
        || path_str.ends_with("_spec.rb")
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
    visibility_stack: &mut Vec<Option<String>>,
    is_test_file: bool,
) {
    loop {
        let node = cursor.node();
        match node.kind() {
            "class" => {
                handle_class(
                    path,
                    source,
                    &node,
                    container.clone(),
                    symbols,
                    edges,
                    declared_spans,
                    symbol_by_name,
                    visibility_stack,
                    is_test_file,
                );
            }
            "module" => {
                handle_module(
                    path,
                    source,
                    &node,
                    container.clone(),
                    symbols,
                    edges,
                    declared_spans,
                    symbol_by_name,
                    visibility_stack,
                    is_test_file,
                );
            }
            "method" => {
                handle_method(
                    path,
                    source,
                    &node,
                    container.clone(),
                    symbols,
                    declared_spans,
                    symbol_by_name,
                    visibility_stack,
                    is_test_file,
                );
            }
            "singleton_method" => {
                handle_singleton_method(
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
            "assignment" => {
                handle_assignment(
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
            "call" => {
                // Handle include/extend calls and visibility with arguments
                handle_call(
                    path,
                    source,
                    &node,
                    container.clone(),
                    edges,
                    symbol_by_name,
                    visibility_stack,
                );
            }
            "identifier" if container.is_some() => {
                // Bare visibility modifiers (private/protected/public without args)
                // are parsed as plain identifiers in class/module bodies
                let name = slice(source, &node);
                if matches!(name.as_str(), "private" | "protected" | "public") {
                    if let Some(last) = visibility_stack.last_mut() {
                        *last = Some(name);
                    }
                }
            }
            _ => {}
        }

        // Recurse into children, but skip class/module bodies since they
        // handle their own body traversal with visibility scoping
        if !matches!(node.kind(), "class" | "module") && cursor.goto_first_child() {
            walk_symbols(
                path,
                source,
                cursor,
                container.clone(),
                symbols,
                edges,
                declared_spans,
                symbol_by_name,
                visibility_stack,
                is_test_file,
            );
            cursor.goto_parent();
        }

        if !cursor.goto_next_sibling() {
            break;
        }
    }
}

/// Handle class definitions
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
    visibility_stack: &mut Vec<Option<String>>,
    is_test_file: bool,
) {
    let span = (node.start_byte(), node.end_byte());
    if declared_spans.contains(&span) {
        return;
    }

    let name = match node.child_by_field_name("name") {
        Some(name_node) => extract_constant_name(source, &name_node),
        None => return,
    };
    if name.is_empty() {
        return;
    }

    let sym = make_symbol(
        path,
        node,
        &name,
        "class",
        container.clone(),
        source.as_bytes(),
        is_test_file,
        Some("public".to_string()),
    );
    declared_spans.insert(span);
    symbol_by_name.insert(name.clone(), sym.id.clone());

    // Handle superclass (inheritance)
    if let Some(superclass_node) = node.child_by_field_name("superclass") {
        let mut sc_cursor = superclass_node.walk();
        for child in superclass_node.children(&mut sc_cursor) {
            if child.is_named() {
                let base_name = extract_constant_name(source, &child);
                if !base_name.is_empty() {
                    let dst_id = symbol_by_name
                        .get(&base_name)
                        .cloned()
                        .unwrap_or_else(|| format!("{}#{}", normalize_path(path), base_name));
                    edges.push(EdgeRecord {
                        src: sym.id.clone(),
                        dst: dst_id,
                        kind: "extends".to_string(),
                    });
                }
                break;
            }
        }
    }

    symbols.push(sym);

    // Push new visibility scope for class body, then walk body, then pop
    visibility_stack.push(None);
    if let Some(body) = node.child_by_field_name("body") {
        let mut body_cursor = body.walk();
        walk_symbols(
            path,
            source,
            &mut body_cursor,
            Some(name),
            symbols,
            edges,
            declared_spans,
            symbol_by_name,
            visibility_stack,
            is_test_file,
        );
    }
    visibility_stack.pop();
}

/// Handle module definitions
#[allow(clippy::too_many_arguments)]
fn handle_module(
    path: &Path,
    source: &str,
    node: &Node,
    container: Option<String>,
    symbols: &mut Vec<SymbolRecord>,
    edges: &mut Vec<EdgeRecord>,
    declared_spans: &mut HashSet<(usize, usize)>,
    symbol_by_name: &mut HashMap<String, String>,
    visibility_stack: &mut Vec<Option<String>>,
    is_test_file: bool,
) {
    let span = (node.start_byte(), node.end_byte());
    if declared_spans.contains(&span) {
        return;
    }

    let name = match node.child_by_field_name("name") {
        Some(name_node) => extract_constant_name(source, &name_node),
        None => return,
    };
    if name.is_empty() {
        return;
    }

    let sym = make_symbol(
        path,
        node,
        &name,
        "module",
        container.clone(),
        source.as_bytes(),
        is_test_file,
        Some("public".to_string()),
    );
    declared_spans.insert(span);
    symbol_by_name.insert(name.clone(), sym.id.clone());
    symbols.push(sym);

    // Push new visibility scope for module body
    visibility_stack.push(None);
    if let Some(body) = node.child_by_field_name("body") {
        let mut body_cursor = body.walk();
        walk_symbols(
            path,
            source,
            &mut body_cursor,
            Some(name),
            symbols,
            edges,
            declared_spans,
            symbol_by_name,
            visibility_stack,
            is_test_file,
        );
    }
    visibility_stack.pop();
}

/// Handle method definitions (instance methods)
#[allow(clippy::too_many_arguments)]
fn handle_method(
    path: &Path,
    source: &str,
    node: &Node,
    container: Option<String>,
    symbols: &mut Vec<SymbolRecord>,
    declared_spans: &mut HashSet<(usize, usize)>,
    symbol_by_name: &mut HashMap<String, String>,
    visibility_stack: &mut [Option<String>],
    is_test_file: bool,
) {
    let span = (node.start_byte(), node.end_byte());
    if declared_spans.contains(&span) {
        return;
    }

    let name = match node.child_by_field_name("name") {
        Some(name_node) => slice(source, &name_node),
        None => return,
    };
    if name.is_empty() {
        return;
    }

    let kind = if container.is_some() {
        "method"
    } else {
        "function"
    };

    let is_test = is_test_file || name.starts_with("test_") || name.starts_with("test");

    // Determine visibility from the visibility stack
    let visibility = if container.is_some() {
        visibility_stack
            .last()
            .cloned()
            .flatten()
            .or_else(|| Some("public".to_string()))
    } else {
        None
    };

    let sym = make_symbol(
        path,
        node,
        &name,
        kind,
        container,
        source.as_bytes(),
        is_test,
        visibility,
    );
    declared_spans.insert(span);
    symbol_by_name.insert(name.clone(), sym.id.clone());
    symbols.push(sym);
}

/// Handle singleton method definitions (class methods like `def self.foo`)
#[allow(clippy::too_many_arguments)]
fn handle_singleton_method(
    path: &Path,
    source: &str,
    node: &Node,
    container: Option<String>,
    symbols: &mut Vec<SymbolRecord>,
    declared_spans: &mut HashSet<(usize, usize)>,
    symbol_by_name: &mut HashMap<String, String>,
    is_test_file: bool,
) {
    let span = (node.start_byte(), node.end_byte());
    if declared_spans.contains(&span) {
        return;
    }

    let name = match node.child_by_field_name("name") {
        Some(name_node) => slice(source, &name_node),
        None => return,
    };
    if name.is_empty() {
        return;
    }

    let sym = make_symbol(
        path,
        node,
        &name,
        "method",
        container,
        source.as_bytes(),
        is_test_file,
        Some("public".to_string()),
    );
    declared_spans.insert(span);

    // Store with "self." prefix for lookup disambiguation
    let qualified = format!("self.{}", name);
    symbol_by_name.insert(qualified, sym.id.clone());
    symbol_by_name.insert(name.clone(), sym.id.clone());
    symbols.push(sym);
}

/// Handle constant and variable assignments
#[allow(clippy::too_many_arguments)]
fn handle_assignment(
    path: &Path,
    source: &str,
    node: &Node,
    container: Option<String>,
    symbols: &mut Vec<SymbolRecord>,
    declared_spans: &mut HashSet<(usize, usize)>,
    symbol_by_name: &mut HashMap<String, String>,
    is_test_file: bool,
) {
    let span = (node.start_byte(), node.end_byte());
    if declared_spans.contains(&span) {
        return;
    }

    let left = match node.child_by_field_name("left") {
        Some(l) => l,
        None => return,
    };

    // Only handle constant assignments (e.g., `FOO = 1`, `MyConst = ...`)
    if left.kind() == "constant" {
        let name = slice(source, &left);
        if !name.is_empty() {
            let sym = make_symbol(
                path,
                node,
                &name,
                "const",
                container,
                source.as_bytes(),
                is_test_file,
                Some("public".to_string()),
            );
            declared_spans.insert(span);
            symbol_by_name.insert(name.clone(), sym.id.clone());
            symbols.push(sym);
        }
    }
}

/// Handle call nodes for visibility modifiers, include, and extend
fn handle_call(
    path: &Path,
    source: &str,
    node: &Node,
    container: Option<String>,
    edges: &mut Vec<EdgeRecord>,
    symbol_by_name: &HashMap<String, String>,
    visibility_stack: &mut [Option<String>],
) {
    let method_name = match node.child_by_field_name("method") {
        Some(m) => slice(source, &m),
        None => return,
    };

    // Only handle bare calls (no receiver) for visibility/include/extend
    if node.child_by_field_name("receiver").is_some() {
        return;
    }

    match method_name.as_str() {
        "private" | "protected" | "public" => {
            // Check if called with arguments (e.g., `private :method_name`)
            // or without arguments (changes default visibility for subsequent methods)
            let has_args = if let Some(args) = node.child_by_field_name("arguments") {
                let mut c = args.walk();
                let result = args.children(&mut c).any(|ch| ch.is_named());
                result
            } else {
                false
            };

            if !has_args {
                // Bare visibility modifier - affects subsequent methods
                if let Some(last) = visibility_stack.last_mut() {
                    *last = Some(method_name.clone());
                }
            }
        }
        "include" | "extend" | "prepend" => {
            // Extract module names from arguments
            if let Some(container_name) = &container {
                if let Some(container_id) = symbol_by_name.get(container_name) {
                    if let Some(args) = node.child_by_field_name("arguments") {
                        let mut cursor = args.walk();
                        for arg in args.children(&mut cursor) {
                            if arg.kind() == "constant" || arg.kind() == "scope_resolution" {
                                let mod_name = extract_constant_name(source, &arg);
                                if !mod_name.is_empty() {
                                    let edge_kind = if method_name == "extend" {
                                        "extends"
                                    } else {
                                        "implements"
                                    };
                                    let dst_id =
                                        symbol_by_name.get(&mod_name).cloned().unwrap_or_else(
                                            || format!("{}#{}", normalize_path(path), mod_name),
                                        );
                                    edges.push(EdgeRecord {
                                        src: container_id.clone(),
                                        dst: dst_id,
                                        kind: edge_kind.to_string(),
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }
        _ => {}
    }
}

/// Extract a constant name, handling scope resolution (e.g., `Foo::Bar` → "Foo::Bar")
fn extract_constant_name(source: &str, node: &Node) -> String {
    match node.kind() {
        "constant" => slice(source, node),
        "scope_resolution" => slice(source, node),
        _ => slice(source, node),
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
        if matches!(node.kind(), "identifier" | "constant") {
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

/// Collect require/require_relative statements
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
        if node.kind() == "call" {
            let method_name = node
                .child_by_field_name("method")
                .map(|m| slice(source, &m));

            if matches!(method_name.as_deref(), Some("require" | "require_relative")) {
                let import_text = slice(source, &node);

                if let Some(args) = node.child_by_field_name("arguments") {
                    let mut cursor = args.walk();
                    for arg in args.children(&mut cursor) {
                        if arg.kind() == "string" {
                            let raw = slice(source, &arg);
                            // Remove surrounding quotes
                            let import_path = raw
                                .trim_start_matches(['"', '\''])
                                .trim_end_matches(['"', '\''])
                                .to_string();

                            if !import_path.is_empty() {
                                let kind = if method_name.as_deref() == Some("require_relative") {
                                    "require_relative"
                                } else {
                                    "require"
                                };

                                dependencies.push(FileDependency {
                                    from_file: from_file.clone(),
                                    to_file: import_path.clone(),
                                    kind: kind.to_string(),
                                });

                                // Local name is the last path component
                                let local_name = import_path
                                    .rsplit('/')
                                    .next()
                                    .unwrap_or(&import_path)
                                    .to_string();

                                import_bindings.push(ImportBindingInfo {
                                    local_name,
                                    source_file: from_file.clone(),
                                    original_name: import_path,
                                    import_text: import_text.clone(),
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

    (dependencies, import_bindings)
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

/// Ruby language parser implementing the `LanguageParser` trait.
#[derive(Clone)]
pub struct RubyParser;

impl RubyParser {
    /// Create a new Ruby parser.
    pub fn new() -> Self {
        Self
    }
}

impl Default for RubyParser {
    fn default() -> Self {
        Self::new()
    }
}

impl LanguageParser for RubyParser {
    fn config(&self) -> LanguageConfig {
        LanguageConfig {
            name: "Ruby",
            extensions: &["rb"],
        }
    }

    fn language(&self) -> &Language {
        &RUBY_LANGUAGE
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
    fn extracts_classes() {
        let source = r#"
class Animal
  def speak
    "..."
  end
end

class Dog < Animal
  def speak
    "Woof!"
  end
end
"#;
        let path = PathBuf::from("test.rb");
        let (symbols, edges, _, _, _) = index_file(&path, source).unwrap();

        let animal = symbols.iter().find(|s| s.name == "Animal").unwrap();
        assert_eq!(animal.kind, "class");
        assert_eq!(animal.visibility, Some("public".to_string()));

        let dog = symbols.iter().find(|s| s.name == "Dog").unwrap();
        assert_eq!(dog.kind, "class");

        // Dog should have an extends edge to Animal
        let extends_edges: Vec<_> = edges.iter().filter(|e| e.kind == "extends").collect();
        assert_eq!(extends_edges.len(), 1);
        assert_eq!(extends_edges[0].src, dog.id);
    }

    #[test]
    fn extracts_modules() {
        let source = r#"
module Serializable
  def serialize
    to_json
  end
end

module Validators
  module Email
    def validate_email(addr)
      addr.include?("@")
    end
  end
end
"#;
        let path = PathBuf::from("test.rb");
        let (symbols, _, _, _, _) = index_file(&path, source).unwrap();

        let serializable = symbols.iter().find(|s| s.name == "Serializable").unwrap();
        assert_eq!(serializable.kind, "module");

        let validators = symbols.iter().find(|s| s.name == "Validators").unwrap();
        assert_eq!(validators.kind, "module");

        let email = symbols.iter().find(|s| s.name == "Email").unwrap();
        assert_eq!(email.kind, "module");
        assert_eq!(email.qualifier, Some("Validators".to_string()));
    }

    #[test]
    fn extracts_methods() {
        let source = r#"
class Calculator
  def add(a, b)
    a + b
  end

  def self.create
    new
  end
end

def standalone_function
  puts "hello"
end
"#;
        let path = PathBuf::from("test.rb");
        let (symbols, _, _, _, _) = index_file(&path, source).unwrap();

        let add = symbols.iter().find(|s| s.name == "add").unwrap();
        assert_eq!(add.kind, "method");
        assert_eq!(add.qualifier, Some("Calculator".to_string()));

        let create = symbols.iter().find(|s| s.name == "create").unwrap();
        assert_eq!(create.kind, "method");
        assert_eq!(create.qualifier, Some("Calculator".to_string()));

        let standalone = symbols
            .iter()
            .find(|s| s.name == "standalone_function")
            .unwrap();
        assert_eq!(standalone.kind, "function");
        assert!(standalone.qualifier.is_none());
    }

    #[test]
    fn extracts_constants() {
        let source = r#"
MAX_SIZE = 100
VERSION = "1.0.0"

class Config
  DEFAULT_PORT = 3000
end
"#;
        let path = PathBuf::from("test.rb");
        let (symbols, _, _, _, _) = index_file(&path, source).unwrap();

        let max_size = symbols.iter().find(|s| s.name == "MAX_SIZE").unwrap();
        assert_eq!(max_size.kind, "const");

        let version = symbols.iter().find(|s| s.name == "VERSION").unwrap();
        assert_eq!(version.kind, "const");

        let default_port = symbols.iter().find(|s| s.name == "DEFAULT_PORT").unwrap();
        assert_eq!(default_port.kind, "const");
        assert_eq!(default_port.qualifier, Some("Config".to_string()));
    }

    #[test]
    fn handles_visibility_modifiers() {
        let source = r#"
class MyClass
  def public_method
    "public"
  end

  private

  def secret_method
    "private"
  end

  def another_secret
    "also private"
  end

  protected

  def protected_method
    "protected"
  end
end
"#;
        let path = PathBuf::from("test.rb");
        let (symbols, _, _, _, _) = index_file(&path, source).unwrap();

        let public_m = symbols.iter().find(|s| s.name == "public_method").unwrap();
        assert_eq!(public_m.visibility, Some("public".to_string()));

        let secret_m = symbols.iter().find(|s| s.name == "secret_method").unwrap();
        assert_eq!(secret_m.visibility, Some("private".to_string()));

        let another = symbols.iter().find(|s| s.name == "another_secret").unwrap();
        assert_eq!(another.visibility, Some("private".to_string()));

        let protected_m = symbols
            .iter()
            .find(|s| s.name == "protected_method")
            .unwrap();
        assert_eq!(protected_m.visibility, Some("protected".to_string()));
    }

    #[test]
    fn detects_test_files() {
        let source = r#"
class UserTest
  def test_create_user
    assert User.new("test").valid?
  end

  def helper
    "not a test"
  end
end
"#;
        let path = PathBuf::from("test/user_test.rb");
        let (symbols, _, _, _, _) = index_file(&path, source).unwrap();

        // All symbols in test files are marked as test
        for sym in &symbols {
            assert!(sym.is_test, "Symbol {} should be marked as test", sym.name);
        }
    }

    #[test]
    fn extracts_require_imports() {
        let source = r#"
require 'json'
require "net/http"
require_relative '../lib/helper'
"#;
        let path = PathBuf::from("test.rb");
        let (_, _, _, dependencies, import_bindings) = index_file(&path, source).unwrap();

        assert_eq!(dependencies.len(), 3);

        let json_dep = dependencies.iter().find(|d| d.to_file == "json").unwrap();
        assert_eq!(json_dep.kind, "require");

        let http_dep = dependencies
            .iter()
            .find(|d| d.to_file == "net/http")
            .unwrap();
        assert_eq!(http_dep.kind, "require");

        let helper_dep = dependencies
            .iter()
            .find(|d| d.to_file == "../lib/helper")
            .unwrap();
        assert_eq!(helper_dep.kind, "require_relative");

        assert_eq!(import_bindings.len(), 3);
        let helper_binding = import_bindings
            .iter()
            .find(|b| b.original_name == "../lib/helper")
            .unwrap();
        assert_eq!(helper_binding.local_name, "helper");
    }

    #[test]
    fn handles_include_and_extend() {
        let source = r#"
module Loggable
end

module Cacheable
end

class Service
  include Loggable
  extend Cacheable
end
"#;
        let path = PathBuf::from("test.rb");
        let (_, edges, _, _, _) = index_file(&path, source).unwrap();

        let include_edges: Vec<_> = edges.iter().filter(|e| e.kind == "implements").collect();
        assert_eq!(include_edges.len(), 1);

        let extend_edges: Vec<_> = edges.iter().filter(|e| e.kind == "extends").collect();
        assert_eq!(extend_edges.len(), 1);
    }

    #[test]
    fn collects_references() {
        let source = r#"
class Greeter
  def greet(name)
    "Hello, #{name}"
  end
end

def main
  g = Greeter.new
  puts g.greet("World")
end
"#;
        let path = PathBuf::from("test.rb");
        let (_, _, refs, _, _) = index_file(&path, source).unwrap();

        // Should have references to Greeter and greet
        assert!(!refs.is_empty());
    }

    #[test]
    fn registry_finds_ruby_parser() {
        use crate::languages::ParserRegistry;

        let registry = ParserRegistry::new();
        assert!(registry.is_supported(&PathBuf::from("test.rb")));
    }
}
