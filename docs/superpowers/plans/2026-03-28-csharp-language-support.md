# C# Language Support Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add C# language support to gabb-cli's code indexer, following the established tree-sitter parser pattern.

**Architecture:** A new `src/languages/csharp.rs` parser module that implements the `LanguageParser` trait using the `tree-sitter-c-sharp` crate. The parser extracts symbols (classes, structs, interfaces, enums, methods, properties, fields, namespaces), edges (extends, implements, calls), references, and `using` directive dependencies. It is registered in the `ParserRegistry` for `.cs` files.

**Tech Stack:** Rust, tree-sitter, tree-sitter-c-sharp 0.23.1, SQLite (existing store)

---

### Task 0: Create feature branch and add dependency

**Files:**
- Modify: `Cargo.toml`

- [ ] **Step 0.1: Create feature branch**

```bash
git checkout -b feat/csharp-language-support
```

- [ ] **Step 0.2: Add tree-sitter-c-sharp dependency to Cargo.toml**

In `Cargo.toml`, add `tree-sitter-c-sharp` after the `tree-sitter-ruby` line in `[dependencies]`:

```toml
tree-sitter-c-sharp = "0.23.1"
```

- [ ] **Step 0.3: Verify it compiles**

Run: `cargo check`
Expected: Compiles successfully with the new dependency.

- [ ] **Step 0.4: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "feat(languages): add tree-sitter-c-sharp dependency for C# support"
```

---

### Task 1: Scaffold the C# parser module with struct and registration

**Files:**
- Create: `src/languages/csharp.rs`
- Modify: `src/languages/mod.rs`
- Modify: `src/languages/registry.rs`

- [ ] **Step 1.1: Write the failing test for registry integration**

Create `src/languages/csharp.rs` with just enough to write the test:

```rust
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
    let _tree = parser
        .parse(source, None)
        .context("failed to parse C# file")?;

    Ok((Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new()))
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn registry_finds_csharp_parser() {
        use crate::languages::ParserRegistry;

        let registry = ParserRegistry::new();
        assert!(registry.is_supported(&PathBuf::from("test.cs")));
    }
}
```

- [ ] **Step 1.2: Register the module in mod.rs**

In `src/languages/mod.rs`, add `pub mod csharp;` in alphabetical order (after `cpp`, before `go`):

```rust
pub mod cpp;
pub mod csharp;
pub mod go;
```

- [ ] **Step 1.3: Register the parser in registry.rs**

In `src/languages/registry.rs`, add the import at the top alongside the others:

```rust
use super::{cpp, csharp, go, kotlin, python, ruby, rust, typescript};
```

Then add the registration block in the `new()` method, after the Ruby parser registration:

```rust
        // Register C# parser
        let csharp_parser = csharp::CSharpParser::new();
        for ext in csharp_parser.config().extensions {
            registry
                .parsers
                .insert(ext, Box::new(csharp_parser.clone()));
        }
```

- [ ] **Step 1.4: Run the test to verify it passes**

Run: `cargo test registry_finds_csharp_parser -- --nocapture`
Expected: PASS

- [ ] **Step 1.5: Add registry test for registered_languages**

In `src/languages/registry.rs`, update the `registered_languages_returns_all` test to include C#. Add this assertion:

```rust
        assert!(languages.contains(&"C#"));
```

- [ ] **Step 1.6: Run all registry tests**

Run: `cargo test languages::registry -- --nocapture`
Expected: All PASS

- [ ] **Step 1.7: Commit**

```bash
git add src/languages/csharp.rs src/languages/mod.rs src/languages/registry.rs
git commit -m "feat(languages): scaffold C# parser with registry integration"
```

---

### Task 2: Implement class and struct extraction

**Files:**
- Modify: `src/languages/csharp.rs`

- [ ] **Step 2.1: Write the failing test for class extraction**

Add to the `tests` module in `src/languages/csharp.rs`:

```rust
    #[test]
    fn extracts_classes() {
        let source = r#"
namespace MyApp
{
    public class Animal
    {
        public void Speak() { }
    }

    public class Dog : Animal
    {
        public void Speak() { }
    }
}
"#;
        let path = PathBuf::from("test.cs");
        let (symbols, edges, _, _, _) = index_file(&path, source).unwrap();

        let animal = symbols.iter().find(|s| s.name == "Animal").unwrap();
        assert_eq!(animal.kind, "class");
        assert_eq!(animal.visibility, Some("public".to_string()));
        assert_eq!(animal.qualifier, Some("MyApp".to_string()));

        let dog = symbols.iter().find(|s| s.name == "Dog").unwrap();
        assert_eq!(dog.kind, "class");

        // Dog should have an extends edge to Animal
        let extends_edges: Vec<_> = edges.iter().filter(|e| e.kind == "extends").collect();
        assert!(!extends_edges.is_empty());
        assert_eq!(extends_edges[0].src, dog.id);
    }

    #[test]
    fn extracts_structs() {
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
    }

    #[test]
    fn extracts_records() {
        let source = r#"
public record Person(string Name, int Age);

public record struct Coordinate(double X, double Y);
"#;
        let path = PathBuf::from("test.cs");
        let (symbols, _, _, _, _) = index_file(&path, source).unwrap();

        let person = symbols.iter().find(|s| s.name == "Person").unwrap();
        assert_eq!(person.kind, "class");

        let coord = symbols.iter().find(|s| s.name == "Coordinate").unwrap();
        assert_eq!(coord.kind, "struct");
    }
```

- [ ] **Step 2.2: Run tests to verify they fail**

Run: `cargo test languages::csharp::tests::extracts_classes -- --nocapture`
Expected: FAIL (no symbols returned yet)

- [ ] **Step 2.3: Implement the core parsing infrastructure and class/struct extraction**

Replace the `index_file` function body and add the helper functions in `src/languages/csharp.rs`. The full implementation follows the Ruby parser pattern:

```rust
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

    {
        let mut cursor = tree.walk();
        walk_symbols(
            path,
            source,
            &mut cursor,
            None, // container
            None, // namespace
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

/// Check if a file path looks like a test file
fn is_test_path(path: &Path) -> bool {
    let path_str = path.to_string_lossy();
    path_str.contains("/Tests/")
        || path_str.contains("/Test/")
        || path_str.ends_with("Tests.cs")
        || path_str.ends_with("Test.cs")
}

/// Determine visibility from modifier nodes. Returns the explicit visibility
/// or the appropriate C# default.
fn get_visibility(node: &Node, source: &str, is_type_level: bool) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "modifier" || child.kind() == "modifier_list" {
            let text = slice(source, &child);
            if text.contains("public") {
                return Some("public".to_string());
            }
            if text.contains("protected") && text.contains("internal") {
                return Some("protected internal".to_string());
            }
            if text.contains("protected") {
                return Some("protected".to_string());
            }
            if text.contains("private") {
                return Some("private".to_string());
            }
            if text.contains("internal") {
                return Some("internal".to_string());
            }
        }
        // Also check individual modifier keywords
        let text = slice(source, &child);
        match text.as_str() {
            "public" => return Some("public".to_string()),
            "protected" => return Some("protected".to_string()),
            "private" => return Some("private".to_string()),
            "internal" => return Some("internal".to_string()),
            _ => {}
        }
    }
    // C# defaults: top-level types are internal, members are private
    if is_type_level {
        Some("internal".to_string())
    } else {
        Some("private".to_string())
    }
}

/// Build the qualifier string from namespace and container
fn build_qualifier(namespace: &Option<String>, container: &Option<String>) -> Option<String> {
    match (namespace, container) {
        (Some(ns), Some(c)) => Some(format!("{}.{}", ns, c)),
        (Some(ns), None) => Some(ns.clone()),
        (None, Some(c)) => Some(c.clone()),
        (None, None) => None,
    }
}

/// Walk the AST and extract symbols
#[allow(clippy::too_many_arguments)]
fn walk_symbols(
    path: &Path,
    source: &str,
    cursor: &mut TreeCursor,
    container: Option<String>,
    namespace: Option<String>,
    symbols: &mut Vec<SymbolRecord>,
    edges: &mut Vec<EdgeRecord>,
    declared_spans: &mut HashSet<(usize, usize)>,
    symbol_by_name: &mut HashMap<String, String>,
    is_test_file: bool,
) {
    loop {
        let node = cursor.node();
        match node.kind() {
            "namespace_declaration" | "file_scoped_namespace_declaration" => {
                handle_namespace(
                    path,
                    source,
                    &node,
                    container.clone(),
                    namespace.clone(),
                    symbols,
                    edges,
                    declared_spans,
                    symbol_by_name,
                    is_test_file,
                );
            }
            "class_declaration" | "record_declaration" => {
                handle_class(
                    path,
                    source,
                    &node,
                    container.clone(),
                    namespace.clone(),
                    symbols,
                    edges,
                    declared_spans,
                    symbol_by_name,
                    is_test_file,
                );
            }
            "struct_declaration" | "record_struct_declaration" => {
                handle_struct(
                    path,
                    source,
                    &node,
                    container.clone(),
                    namespace.clone(),
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
                    namespace.clone(),
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
                    namespace.clone(),
                    symbols,
                    declared_spans,
                    symbol_by_name,
                    is_test_file,
                );
            }
            "method_declaration" | "constructor_declaration" => {
                handle_method(
                    path,
                    source,
                    &node,
                    container.clone(),
                    namespace.clone(),
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
                    namespace.clone(),
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
                    namespace.clone(),
                    symbols,
                    declared_spans,
                    symbol_by_name,
                    is_test_file,
                );
            }
            _ => {}
        }

        // Recurse into children, but skip type/namespace bodies since they handle their own traversal
        if !matches!(
            node.kind(),
            "class_declaration"
                | "record_declaration"
                | "struct_declaration"
                | "record_struct_declaration"
                | "interface_declaration"
                | "namespace_declaration"
                | "file_scoped_namespace_declaration"
        ) && cursor.goto_first_child()
        {
            walk_symbols(
                path,
                source,
                cursor,
                container.clone(),
                namespace.clone(),
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

/// Handle namespace declarations
#[allow(clippy::too_many_arguments)]
fn handle_namespace(
    path: &Path,
    source: &str,
    node: &Node,
    container: Option<String>,
    parent_namespace: Option<String>,
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

    let name = match node.child_by_field_name("name") {
        Some(name_node) => slice(source, &name_node),
        None => return,
    };
    if name.is_empty() {
        return;
    }

    let full_namespace = match &parent_namespace {
        Some(parent) => format!("{}.{}", parent, name),
        None => name.clone(),
    };

    let qualifier = parent_namespace.clone();

    let sym = make_symbol(
        path,
        node,
        &name,
        "namespace",
        qualifier,
        container.clone(),
        source.as_bytes(),
        is_test_file,
        None,
    );
    declared_spans.insert(span);
    symbol_by_name.insert(name.clone(), sym.id.clone());
    symbols.push(sym);

    // Walk the body with updated namespace
    if let Some(body) = node.child_by_field_name("body") {
        let mut body_cursor = body.walk();
        walk_symbols(
            path,
            source,
            &mut body_cursor,
            container,
            Some(full_namespace.clone()),
            symbols,
            edges,
            declared_spans,
            symbol_by_name,
            is_test_file,
        );
    }

    // For file-scoped namespaces, walk remaining siblings
    if node.kind() == "file_scoped_namespace_declaration" {
        // File-scoped namespaces don't have a body node; their scope extends
        // to the end of the file. The remaining siblings are handled by the
        // parent walk_symbols call with the updated namespace context.
        // We need to walk the children of this node that aren't the name.
        let mut child_cursor = node.walk();
        if child_cursor.goto_first_child() {
            walk_symbols(
                path,
                source,
                &mut child_cursor,
                container,
                Some(full_namespace),
                symbols,
                edges,
                declared_spans,
                symbol_by_name,
                is_test_file,
            );
        }
    }
}

/// Handle class and record declarations
#[allow(clippy::too_many_arguments)]
fn handle_class(
    path: &Path,
    source: &str,
    node: &Node,
    container: Option<String>,
    namespace: Option<String>,
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

    let name = match node.child_by_field_name("name") {
        Some(name_node) => slice(source, &name_node),
        None => return,
    };
    if name.is_empty() {
        return;
    }

    // Strip generic type parameters (e.g., "List<T>" -> "List")
    let name = strip_generics(&name);

    let is_type_level = container.is_none();
    let visibility = get_visibility(node, source, is_type_level);
    let qualifier = build_qualifier(&namespace, &container);

    let sym = make_symbol(
        path,
        node,
        &name,
        "class",
        qualifier,
        container.clone(),
        source.as_bytes(),
        is_test_file,
        visibility,
    );
    declared_spans.insert(span);
    symbol_by_name.insert(name.clone(), sym.id.clone());

    // Handle base types (inheritance and interface implementation)
    extract_base_types(path, source, node, &sym.id, edges, symbol_by_name, true);

    symbols.push(sym);

    // Walk body with this class as container
    if let Some(body) = node.child_by_field_name("body") {
        let mut body_cursor = body.walk();
        walk_symbols(
            path,
            source,
            &mut body_cursor,
            Some(name),
            namespace,
            symbols,
            edges,
            declared_spans,
            symbol_by_name,
            is_test_file,
        );
    }
}

/// Handle struct and record struct declarations
#[allow(clippy::too_many_arguments)]
fn handle_struct(
    path: &Path,
    source: &str,
    node: &Node,
    container: Option<String>,
    namespace: Option<String>,
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

    let name = match node.child_by_field_name("name") {
        Some(name_node) => slice(source, &name_node),
        None => return,
    };
    if name.is_empty() {
        return;
    }

    let name = strip_generics(&name);
    let is_type_level = container.is_none();
    let visibility = get_visibility(node, source, is_type_level);
    let qualifier = build_qualifier(&namespace, &container);

    let sym = make_symbol(
        path,
        node,
        &name,
        "struct",
        qualifier,
        container.clone(),
        source.as_bytes(),
        is_test_file,
        visibility,
    );
    declared_spans.insert(span);
    symbol_by_name.insert(name.clone(), sym.id.clone());

    // Handle interface implementation
    extract_base_types(path, source, node, &sym.id, edges, symbol_by_name, false);

    symbols.push(sym);

    // Walk body
    if let Some(body) = node.child_by_field_name("body") {
        let mut body_cursor = body.walk();
        walk_symbols(
            path,
            source,
            &mut body_cursor,
            Some(name),
            namespace,
            symbols,
            edges,
            declared_spans,
            symbol_by_name,
            is_test_file,
        );
    }
}

/// Extract base types from a class/struct/interface base_list.
/// For classes: first base type is extends (if class), rest are implements.
/// For structs/interfaces: all base types are implements.
fn extract_base_types(
    path: &Path,
    source: &str,
    node: &Node,
    src_id: &str,
    edges: &mut Vec<EdgeRecord>,
    symbol_by_name: &HashMap<String, String>,
    is_class: bool,
) {
    if let Some(bases) = node.child_by_field_name("bases") {
        let mut cursor = bases.walk();
        let mut is_first = true;
        for child in bases.children(&mut cursor) {
            if !child.is_named() {
                continue;
            }
            let base_name = strip_generics(&slice(source, &child));
            if base_name.is_empty() {
                continue;
            }

            let dst_id = symbol_by_name
                .get(&base_name)
                .cloned()
                .unwrap_or_else(|| format!("{}#{}", normalize_path(path), base_name));

            // For classes, first base type could be a class (extends) or interface (implements).
            // We use a simple heuristic: if name starts with 'I' followed by uppercase, it's an interface.
            let edge_kind = if is_class && is_first && !looks_like_interface(&base_name) {
                "extends"
            } else {
                "implements"
            };

            edges.push(EdgeRecord {
                src: src_id.to_string(),
                dst: dst_id,
                kind: edge_kind.to_string(),
            });

            is_first = false;
        }
    }
}

/// Heuristic: names starting with 'I' followed by an uppercase letter are likely interfaces.
fn looks_like_interface(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some('I')) && matches!(chars.next(), Some(c) if c.is_uppercase())
}

/// Strip generic type parameters from a name (e.g., "List<T>" -> "List")
fn strip_generics(name: &str) -> String {
    if let Some(idx) = name.find('<') {
        name[..idx].trim().to_string()
    } else {
        name.to_string()
    }
}
```

Also add stub implementations for the functions referenced but not yet implemented. These will be filled in by later tasks:

```rust
/// Handle interface declarations
#[allow(clippy::too_many_arguments)]
fn handle_interface(
    path: &Path,
    source: &str,
    node: &Node,
    container: Option<String>,
    namespace: Option<String>,
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

    let name = match node.child_by_field_name("name") {
        Some(name_node) => slice(source, &name_node),
        None => return,
    };
    if name.is_empty() {
        return;
    }

    let name = strip_generics(&name);
    let is_type_level = container.is_none();
    let visibility = get_visibility(node, source, is_type_level);
    let qualifier = build_qualifier(&namespace, &container);

    let sym = make_symbol(
        path,
        node,
        &name,
        "interface",
        qualifier,
        container.clone(),
        source.as_bytes(),
        is_test_file,
        visibility,
    );
    declared_spans.insert(span);
    symbol_by_name.insert(name.clone(), sym.id.clone());

    // Handle base interfaces
    extract_base_types(path, source, node, &sym.id, edges, symbol_by_name, false);

    symbols.push(sym);

    // Walk body
    if let Some(body) = node.child_by_field_name("body") {
        let mut body_cursor = body.walk();
        walk_symbols(
            path,
            source,
            &mut body_cursor,
            Some(name),
            namespace,
            symbols,
            edges,
            declared_spans,
            symbol_by_name,
            is_test_file,
        );
    }
}

/// Handle enum declarations
#[allow(clippy::too_many_arguments)]
fn handle_enum(
    path: &Path,
    source: &str,
    node: &Node,
    container: Option<String>,
    namespace: Option<String>,
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

    let is_type_level = container.is_none();
    let visibility = get_visibility(node, source, is_type_level);
    let qualifier = build_qualifier(&namespace, &container);

    let sym = make_symbol(
        path,
        node,
        &name,
        "enum",
        qualifier,
        container,
        source.as_bytes(),
        is_test_file,
        visibility,
    );
    declared_spans.insert(span);
    symbol_by_name.insert(name.clone(), sym.id.clone());
    symbols.push(sym);
}

/// Handle method and constructor declarations
#[allow(clippy::too_many_arguments)]
fn handle_method(
    path: &Path,
    source: &str,
    node: &Node,
    container: Option<String>,
    namespace: Option<String>,
    symbols: &mut Vec<SymbolRecord>,
    declared_spans: &mut HashSet<(usize, usize)>,
    symbol_by_name: &mut HashMap<String, String>,
    is_test_file: bool,
) {
    let span = (node.start_byte(), node.end_byte());
    if declared_spans.contains(&span) {
        return;
    }

    let name = if node.kind() == "constructor_declaration" {
        // Constructor name is the class name, use it directly
        match node.child_by_field_name("name") {
            Some(name_node) => slice(source, &name_node),
            None => return,
        }
    } else {
        match node.child_by_field_name("name") {
            Some(name_node) => slice(source, &name_node),
            None => return,
        }
    };
    if name.is_empty() {
        return;
    }

    let kind = if container.is_some() { "method" } else { "function" };
    let visibility = get_visibility(node, source, false);
    let qualifier = build_qualifier(&namespace, &container);

    // Check for test attributes
    let is_test = is_test_file || has_test_attribute(node, source);

    let sym = make_symbol(
        path,
        node,
        &name,
        kind,
        qualifier,
        container,
        source.as_bytes(),
        is_test,
        visibility,
    );
    declared_spans.insert(span);
    symbol_by_name.insert(name.clone(), sym.id.clone());
    symbols.push(sym);
}

/// Handle property declarations
#[allow(clippy::too_many_arguments)]
fn handle_property(
    path: &Path,
    source: &str,
    node: &Node,
    container: Option<String>,
    namespace: Option<String>,
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

    let visibility = get_visibility(node, source, false);
    let qualifier = build_qualifier(&namespace, &container);

    let sym = make_symbol(
        path,
        node,
        &name,
        "property",
        qualifier,
        container,
        source.as_bytes(),
        is_test_file,
        visibility,
    );
    declared_spans.insert(span);
    symbol_by_name.insert(name.clone(), sym.id.clone());
    symbols.push(sym);
}

/// Handle field declarations
#[allow(clippy::too_many_arguments)]
fn handle_field(
    path: &Path,
    source: &str,
    node: &Node,
    container: Option<String>,
    namespace: Option<String>,
    symbols: &mut Vec<SymbolRecord>,
    declared_spans: &mut HashSet<(usize, usize)>,
    symbol_by_name: &mut HashMap<String, String>,
    is_test_file: bool,
) {
    let span = (node.start_byte(), node.end_byte());
    if declared_spans.contains(&span) {
        return;
    }

    // Field declarations contain variable_declaration with variable_declarator children
    let mut field_cursor = node.walk();
    for child in node.children(&mut field_cursor) {
        if child.kind() == "variable_declaration" {
            let mut var_cursor = child.walk();
            for var_child in child.children(&mut var_cursor) {
                if var_child.kind() == "variable_declarator" {
                    if let Some(name_node) = var_child.child_by_field_name("name") {
                        let name = slice(source, &name_node);
                        if name.is_empty() {
                            continue;
                        }

                        // Check if it's a const field
                        let node_text = slice(source, node);
                        let kind = if node_text.contains("const ") {
                            "const"
                        } else {
                            "field"
                        };

                        let visibility = get_visibility(node, source, false);
                        let qualifier = build_qualifier(&namespace, &container);

                        let sym = make_symbol(
                            path,
                            node,
                            &name,
                            kind,
                            qualifier,
                            container.clone(),
                            source.as_bytes(),
                            is_test_file,
                            visibility,
                        );
                        declared_spans.insert(span);
                        symbol_by_name.insert(name.clone(), sym.id.clone());
                        symbols.push(sym);
                        return; // Only first declarator for now
                    }
                }
            }
        }
    }
}

/// Check if a method has test-related attributes ([Test], [Fact], [Theory], [TestMethod])
fn has_test_attribute(node: &Node, source: &str) -> bool {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "attribute_list" {
            let attr_text = slice(source, &child);
            if attr_text.contains("Test")
                || attr_text.contains("Fact")
                || attr_text.contains("Theory")
                || attr_text.contains("TestMethod")
            {
                return true;
            }
        }
    }
    false
}

/// Collect references to declared symbols
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

/// Collect using directives as dependencies and import bindings
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
        if node.kind() == "using_directive" {
            let import_text = slice(source, &node);
            // Extract the namespace name from the using directive
            if let Some(name_node) = node.child_by_field_name("name") {
                let namespace_name = slice(source, &name_node);
                if !namespace_name.is_empty() {
                    dependencies.push(FileDependency {
                        from_file: from_file.clone(),
                        to_file: namespace_name.clone(),
                        kind: "using".to_string(),
                    });

                    // Local name is the last component
                    let local_name = namespace_name
                        .rsplit('.')
                        .next()
                        .unwrap_or(&namespace_name)
                        .to_string();

                    import_bindings.push(ImportBindingInfo {
                        local_name,
                        source_file: from_file.clone(),
                        original_name: namespace_name,
                        import_text,
                    });
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
```

- [ ] **Step 2.4: Run the class and struct tests**

Run: `cargo test languages::csharp::tests::extracts_classes -- --nocapture`
Run: `cargo test languages::csharp::tests::extracts_structs -- --nocapture`
Run: `cargo test languages::csharp::tests::extracts_records -- --nocapture`
Expected: All PASS

If tests fail, debug by examining tree-sitter node kinds. Use a helper test to print the AST:

```rust
    #[test]
    fn debug_ast() {
        let source = r#"public class Foo { }"#;
        let mut parser = Parser::new();
        parser.set_language(&CSHARP_LANGUAGE).unwrap();
        let tree = parser.parse(source, None).unwrap();
        print_tree(&tree.root_node(), source, 0);
    }

    fn print_tree(node: &Node, source: &str, indent: usize) {
        let prefix = " ".repeat(indent);
        let text = if node.child_count() == 0 {
            format!(" \"{}\"", &source[node.byte_range()])
        } else {
            String::new()
        };
        eprintln!(
            "{}{} [{}-{}]{}",
            prefix,
            node.kind(),
            node.start_byte(),
            node.end_byte(),
            text
        );
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            print_tree(&child, source, indent + 2);
        }
    }
```

Adapt the node kind strings and field names based on what the AST actually looks like. The tree-sitter-c-sharp grammar may use slightly different field names than documented. Common variations to check:
- `bases` vs `base_list` for inheritance
- `name` vs `identifier` for the type name
- `body` vs `declaration_list` for the class body

- [ ] **Step 2.5: Commit**

```bash
git add src/languages/csharp.rs
git commit -m "feat(languages): implement C# class, struct, interface, enum extraction"
```

---

### Task 3: Implement method, property, and field extraction

**Files:**
- Modify: `src/languages/csharp.rs`

- [ ] **Step 3.1: Write the failing tests**

Add to the `tests` module:

```rust
    #[test]
    fn extracts_methods_and_constructors() {
        let source = r#"
public class Calculator
{
    public Calculator() { }

    public int Add(int a, int b)
    {
        return a + b;
    }

    private int Subtract(int a, int b)
    {
        return a - b;
    }
}
"#;
        let path = PathBuf::from("test.cs");
        let (symbols, _, _, _, _) = index_file(&path, source).unwrap();

        let ctor = symbols.iter().find(|s| s.name == "Calculator" && s.kind == "method").unwrap();
        assert_eq!(ctor.kind, "method");
        assert_eq!(ctor.qualifier, Some("Calculator".to_string()));

        let add = symbols.iter().find(|s| s.name == "Add").unwrap();
        assert_eq!(add.kind, "method");
        assert_eq!(add.visibility, Some("public".to_string()));
        assert_eq!(add.qualifier, Some("Calculator".to_string()));

        let sub = symbols.iter().find(|s| s.name == "Subtract").unwrap();
        assert_eq!(sub.visibility, Some("private".to_string()));
    }

    #[test]
    fn extracts_properties() {
        let source = r#"
public class Person
{
    public string Name { get; set; }
    private int Age { get; }
    protected string Address { get; set; }
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

        let addr = symbols.iter().find(|s| s.name == "Address").unwrap();
        assert_eq!(addr.visibility, Some("protected".to_string()));
    }

    #[test]
    fn extracts_fields() {
        let source = r#"
public class Config
{
    public const int MaxSize = 100;
    private string _name;
    internal int Count;
}
"#;
        let path = PathBuf::from("test.cs");
        let (symbols, _, _, _, _) = index_file(&path, source).unwrap();

        let max_size = symbols.iter().find(|s| s.name == "MaxSize").unwrap();
        assert_eq!(max_size.kind, "const");
        assert_eq!(max_size.visibility, Some("public".to_string()));

        let name_field = symbols.iter().find(|s| s.name == "_name").unwrap();
        assert_eq!(name_field.kind, "field");
        assert_eq!(name_field.visibility, Some("private".to_string()));
    }
```

- [ ] **Step 3.2: Run tests to verify they fail**

Run: `cargo test languages::csharp::tests::extracts_methods -- --nocapture`
Expected: FAIL

- [ ] **Step 3.3: Fix any issues with the handler implementations**

The implementations were provided in Task 2 Step 2.3. If tests fail, debug using the `debug_ast` helper test from Task 2 Step 2.4 to determine the correct node kinds and field names.

Common adjustments needed:
- The field name for a method's name might be `name` or another field
- Constructor node kind might be `constructor_declaration`
- Field declarations in C# have a `variable_declaration` child containing `variable_declarator` children

- [ ] **Step 3.4: Run all tests**

Run: `cargo test languages::csharp -- --nocapture`
Expected: All PASS

- [ ] **Step 3.5: Commit**

```bash
git add src/languages/csharp.rs
git commit -m "feat(languages): add C# method, property, and field extraction tests"
```

---

### Task 4: Implement interface, enum, and namespace handling

**Files:**
- Modify: `src/languages/csharp.rs`

- [ ] **Step 4.1: Write the failing tests**

Add to the `tests` module:

```rust
    #[test]
    fn extracts_interfaces() {
        let source = r#"
public interface IAnimal
{
    void Speak();
    string Name { get; }
}

public interface IDomestic : IAnimal
{
    string Owner { get; set; }
}
"#;
        let path = PathBuf::from("test.cs");
        let (symbols, edges, _, _, _) = index_file(&path, source).unwrap();

        let animal = symbols.iter().find(|s| s.name == "IAnimal").unwrap();
        assert_eq!(animal.kind, "interface");
        assert_eq!(animal.visibility, Some("public".to_string()));

        let domestic = symbols.iter().find(|s| s.name == "IDomestic").unwrap();
        assert_eq!(domestic.kind, "interface");

        // IDomestic implements IAnimal
        let impl_edges: Vec<_> = edges.iter().filter(|e| e.kind == "implements").collect();
        assert!(!impl_edges.is_empty());
    }

    #[test]
    fn extracts_enums() {
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
    fn handles_namespaces() {
        let source = r#"
namespace MyApp.Models
{
    public class User
    {
        public string Name { get; set; }
    }
}
"#;
        let path = PathBuf::from("test.cs");
        let (symbols, _, _, _, _) = index_file(&path, source).unwrap();

        let ns = symbols.iter().find(|s| s.name == "MyApp.Models" && s.kind == "namespace");
        assert!(ns.is_some() || symbols.iter().any(|s| s.kind == "namespace"));

        let user = symbols.iter().find(|s| s.name == "User").unwrap();
        assert_eq!(user.kind, "class");
        // User should have namespace as qualifier
        assert!(user.qualifier.is_some());
    }

    #[test]
    fn handles_file_scoped_namespaces() {
        let source = r#"
namespace MyApp.Services;

public class UserService
{
    public void CreateUser() { }
}
"#;
        let path = PathBuf::from("test.cs");
        let (symbols, _, _, _, _) = index_file(&path, source).unwrap();

        let service = symbols.iter().find(|s| s.name == "UserService").unwrap();
        assert_eq!(service.kind, "class");
    }

    #[test]
    fn handles_nested_types() {
        let source = r#"
public class Outer
{
    public class Inner
    {
        public void DoWork() { }
    }
}
"#;
        let path = PathBuf::from("test.cs");
        let (symbols, _, _, _, _) = index_file(&path, source).unwrap();

        let inner = symbols.iter().find(|s| s.name == "Inner").unwrap();
        assert_eq!(inner.kind, "class");
        assert_eq!(inner.qualifier, Some("Outer".to_string()));

        let do_work = symbols.iter().find(|s| s.name == "DoWork").unwrap();
        assert_eq!(do_work.qualifier, Some("Inner".to_string()));
    }
```

- [ ] **Step 4.2: Run tests to verify they fail or pass**

Run: `cargo test languages::csharp::tests::extracts_interfaces -- --nocapture`
Run: `cargo test languages::csharp::tests::handles_namespaces -- --nocapture`
Expected: Should pass since implementations were added in Task 2. If they fail, debug and fix.

- [ ] **Step 4.3: Fix any issues found**

Debug using the `debug_ast` helper if needed. Adapt field names and node kinds to match the actual tree-sitter-c-sharp grammar output.

- [ ] **Step 4.4: Run all C# tests**

Run: `cargo test languages::csharp -- --nocapture`
Expected: All PASS

- [ ] **Step 4.5: Commit**

```bash
git add src/languages/csharp.rs
git commit -m "feat(languages): add C# interface, enum, namespace, and nested type tests"
```

---

### Task 5: Implement inheritance edges, using directives, test detection, and references

**Files:**
- Modify: `src/languages/csharp.rs`

- [ ] **Step 5.1: Write the failing tests**

Add to the `tests` module:

```rust
    #[test]
    fn handles_class_inheritance_and_interfaces() {
        let source = r#"
public interface IMovable
{
    void Move();
}

public class Vehicle
{
    public virtual void Start() { }
}

public class Car : Vehicle, IMovable
{
    public override void Start() { }
    public void Move() { }
}
"#;
        let path = PathBuf::from("test.cs");
        let (symbols, edges, _, _, _) = index_file(&path, source).unwrap();

        let car = symbols.iter().find(|s| s.name == "Car").unwrap();

        // Car extends Vehicle
        let extends: Vec<_> = edges
            .iter()
            .filter(|e| e.src == car.id && e.kind == "extends")
            .collect();
        assert_eq!(extends.len(), 1);

        // Car implements IMovable
        let implements: Vec<_> = edges
            .iter()
            .filter(|e| e.src == car.id && e.kind == "implements")
            .collect();
        assert_eq!(implements.len(), 1);
    }

    #[test]
    fn extracts_using_directives() {
        let source = r#"
using System;
using System.Collections.Generic;
using System.Linq;

public class MyClass { }
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
        let linq_binding = import_bindings
            .iter()
            .find(|b| b.original_name == "System.Linq")
            .unwrap();
        assert_eq!(linq_binding.local_name, "Linq");
    }

    #[test]
    fn detects_test_files_by_path() {
        let source = r#"
public class UserTests
{
    public void TestCreateUser() { }
}
"#;
        let path = PathBuf::from("Tests/UserTests.cs");
        let (symbols, _, _, _, _) = index_file(&path, source).unwrap();

        for sym in &symbols {
            assert!(sym.is_test, "Symbol {} should be marked as test", sym.name);
        }
    }

    #[test]
    fn detects_test_methods_by_attribute() {
        let source = r#"
public class CalculatorTests
{
    [Fact]
    public void Add_ReturnsSum()
    {
        Assert.Equal(3, 1 + 2);
    }

    [Theory]
    public void Add_WithMultipleInputs(int a, int b) { }

    public void HelperMethod() { }
}
"#;
        let path = PathBuf::from("test.cs");
        let (symbols, _, _, _, _) = index_file(&path, source).unwrap();

        let add_test = symbols.iter().find(|s| s.name == "Add_ReturnsSum").unwrap();
        assert!(add_test.is_test);

        let theory_test = symbols
            .iter()
            .find(|s| s.name == "Add_WithMultipleInputs")
            .unwrap();
        assert!(theory_test.is_test);

        let helper = symbols.iter().find(|s| s.name == "HelperMethod").unwrap();
        assert!(!helper.is_test);
    }

    #[test]
    fn collects_references() {
        let source = r#"
public class Greeter
{
    public string Greet(string name)
    {
        return "Hello, " + name;
    }
}

public class App
{
    public void Main()
    {
        var g = new Greeter();
        g.Greet("World");
    }
}
"#;
        let path = PathBuf::from("test.cs");
        let (_, _, refs, _, _) = index_file(&path, source).unwrap();

        // Should have references to Greeter and Greet
        assert!(!refs.is_empty());
    }

    #[test]
    fn handles_visibility_defaults() {
        let source = r#"
class InternalClass
{
    int privateField;
    void PrivateMethod() { }
    public void PublicMethod() { }
}
"#;
        let path = PathBuf::from("test.cs");
        let (symbols, _, _, _, _) = index_file(&path, source).unwrap();

        let internal_class = symbols.iter().find(|s| s.name == "InternalClass").unwrap();
        assert_eq!(internal_class.visibility, Some("internal".to_string()));

        let public_method = symbols.iter().find(|s| s.name == "PublicMethod").unwrap();
        assert_eq!(public_method.visibility, Some("public".to_string()));

        let private_method = symbols.iter().find(|s| s.name == "PrivateMethod").unwrap();
        assert_eq!(private_method.visibility, Some("private".to_string()));
    }
```

- [ ] **Step 5.2: Run the new tests**

Run: `cargo test languages::csharp -- --nocapture`
Expected: Should mostly pass since implementations were added in Task 2. Fix any failures.

- [ ] **Step 5.3: Fix any issues found**

Debug and fix as needed. Common issues:
- `using_directive` name field might need adjustment
- Test attribute detection might need different node traversal
- Base type list field name might differ

- [ ] **Step 5.4: Run full test suite**

Run: `cargo test`
Expected: All PASS (including existing tests for other languages)

- [ ] **Step 5.5: Run linting**

Run: `cargo clippy --all-targets --all-features`
Expected: No warnings

Run: `cargo fmt --check`
Expected: No formatting issues (run `cargo fmt` if needed)

- [ ] **Step 5.6: Commit**

```bash
git add src/languages/csharp.rs
git commit -m "feat(languages): add C# edge, using directive, test detection, and reference tests"
```

---

### Task 6: Update documentation

**Files:**
- Modify: `assets/SKILL.md`
- Modify: `.claude/skills/gabb/SKILL.md`
- Modify: `README.md`
- Modify: `CLAUDE.md`
- Modify: `Cargo.toml` (description)

- [ ] **Step 6.1: Update assets/SKILL.md**

In `assets/SKILL.md`, add C# to the Supported Languages table (after C++, before Go, or at the end):

```
| C#         | `.cs`                                  |
```

- [ ] **Step 6.2: Update .claude/skills/gabb/SKILL.md**

Same change in `.claude/skills/gabb/SKILL.md` -- add C# to the Supported Languages table:

```
| C#         | `.cs`                                  |
```

- [ ] **Step 6.3: Update README.md**

Update all language list mentions in `README.md`:
- Status line: change "Indexes TypeScript/TSX, Rust, Kotlin, C++, Python, Go, and Ruby" to "Indexes TypeScript/TSX, Rust, Kotlin, C++, C#, Python, Go, and Ruby"
- Language parsers line: change "language parsers (TypeScript, Rust, Kotlin, C++, Python, Go, Ruby)" to "language parsers (TypeScript, Rust, Kotlin, C++, C#, Python, Go, Ruby)"

- [ ] **Step 6.4: Update CLAUDE.md**

In `CLAUDE.md`, add the C# parser to the Language Parsers list under Architecture Overview:

```
  - `csharp.rs`: C# parser (`.cs`)
```

Also update the `Cargo.toml` description line reference if it mentions specific languages.

- [ ] **Step 6.5: Update Cargo.toml description**

Update the `description` field in `Cargo.toml`:

```toml
description = "Fast local code indexing CLI for TypeScript, Rust, Kotlin, C++, C#, Python, Go, and Ruby projects"
```

- [ ] **Step 6.6: Commit**

```bash
git add assets/SKILL.md .claude/skills/gabb/SKILL.md README.md CLAUDE.md Cargo.toml
git commit -m "docs: add C# to supported languages documentation"
```

---

### Task 7: Final verification

**Files:** None (verification only)

- [ ] **Step 7.1: Run full test suite**

Run: `cargo test`
Expected: All tests PASS

- [ ] **Step 7.2: Run linting**

Run: `cargo clippy --all-targets --all-features`
Expected: No warnings

Run: `cargo fmt --check`
Expected: No formatting issues

- [ ] **Step 7.3: Build release binary**

Run: `cargo build --release`
Expected: Builds successfully

- [ ] **Step 7.4: Verify C# indexing works end-to-end**

Create a temporary C# file and test indexing:

```bash
mkdir -p /tmp/csharp-test
cat > /tmp/csharp-test/Program.cs << 'EOF'
using System;

namespace MyApp
{
    public class Program
    {
        public static void Main(string[] args)
        {
            Console.WriteLine("Hello, World!");
        }
    }
}
EOF

cargo run -- daemon start --workspace /tmp/csharp-test --db /tmp/csharp-test/.gabb/index.db &
sleep 2
cargo run -- symbols --db /tmp/csharp-test/.gabb/index.db
cargo run -- daemon stop --workspace /tmp/csharp-test
rm -rf /tmp/csharp-test
```

Expected: Should list `Program` class and `Main` method symbols.

- [ ] **Step 7.5: Verify branch is ready for PR**

```bash
git log --oneline main..HEAD
```

Expected: Clean commit history with all feature commits on the branch.
