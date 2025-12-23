use crate::languages::{slice, ImportBindingInfo, ResolvedTarget, SymbolBinding};
use crate::store::{normalize_path, EdgeRecord, FileDependency, ReferenceRecord, SymbolRecord};
use anyhow::{Context, Result};
use once_cell::sync::Lazy;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use tree_sitter::{Language, Node, Parser, TreeCursor};

static RUST_LANGUAGE: Lazy<Language> = Lazy::new(|| tree_sitter_rust::LANGUAGE.into());

/// Index a Rust file, returning symbols, edges, references, file dependencies, and import bindings.
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
        .set_language(&RUST_LANGUAGE)
        .context("failed to set Rust language")?;
    let tree = parser
        .parse(source, None)
        .context("failed to parse Rust file")?;

    let mut symbols = Vec::new();
    let mut edges = Vec::new();
    let mut declared_spans: HashSet<(usize, usize)> = HashSet::new();
    let mut symbol_by_name: HashMap<String, SymbolBinding> = HashMap::new();

    // Extract file dependencies and import bindings FIRST (needed for reference collection)
    let (dependencies, import_bindings) = collect_dependencies(path, source, &tree.root_node());

    // Build imports map from import bindings for cross-file reference resolution
    let imports: HashMap<String, &ImportBindingInfo> = import_bindings
        .iter()
        .map(|b| (b.local_name.clone(), b))
        .collect();

    {
        let mut cursor = tree.walk();
        walk_symbols(
            path,
            source,
            &mut cursor,
            None,
            &[],
            None,
            false, // is_test_context starts as false
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
        &imports,
    );

    // Collect call edges (caller -> callee relationships)
    edges.extend(collect_call_edges(
        path,
        source,
        &tree.root_node(),
        &symbol_by_name,
        &imports,
    ));

    Ok((symbols, edges, references, dependencies, import_bindings))
}

/// Extract file dependencies from `mod` and `use` declarations.
/// - `mod foo;` indicates dependency on foo.rs or foo/mod.rs
/// - `use crate::foo::Bar;` indicates dependency on the foo module
fn collect_dependencies(
    path: &Path,
    source: &str,
    root: &Node,
) -> (Vec<FileDependency>, Vec<ImportBindingInfo>) {
    let mut dependencies = Vec::new();
    let mut import_bindings = Vec::new();
    let mut seen = HashSet::new();
    let from_file = normalize_path(path);
    let parent = path.parent();

    // Find the crate root directory (where Cargo.toml or lib.rs/main.rs is)
    let crate_root = find_crate_root(path);

    let mut stack = vec![*root];
    while let Some(node) = stack.pop() {
        // Handle `mod foo;` declarations (without body)
        if node.kind() == "mod_item" {
            let has_body = node
                .children(&mut node.walk())
                .any(|c| c.kind() == "declaration_list");
            if !has_body {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let mod_name = slice(source, &name_node);
                    let key = format!("mod:{}", mod_name);
                    if !mod_name.is_empty() && !seen.contains(&key) {
                        seen.insert(key);
                        if let Some(to_file) = resolve_mod_path(parent, &mod_name) {
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

        // Handle `use` declarations
        if node.kind() == "use_declaration" {
            if let Some(use_path) = extract_use_path(source, &node) {
                // Only handle crate-local paths (crate::, super::, self::)
                if let Some(resolved) = resolve_use_path(&use_path, path, crate_root.as_deref()) {
                    let key = format!("use:{}", resolved);
                    if !seen.contains(&key) {
                        seen.insert(key.clone());
                        dependencies.push(FileDependency {
                            from_file: from_file.clone(),
                            to_file: resolved.clone(),
                            kind: "use".to_string(),
                        });
                    }
                    // Extract import bindings for two-phase resolution
                    let import_text = slice(source, &node);
                    let bindings = extract_use_bindings(source, &node, &resolved, &import_text);
                    import_bindings.extend(bindings);
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

/// Extract import bindings from a use declaration
fn extract_use_bindings(
    source: &str,
    node: &Node,
    source_file: &str,
    import_text: &str,
) -> Vec<ImportBindingInfo> {
    let mut bindings = Vec::new();
    let mut stack = vec![*node];

    while let Some(n) = stack.pop() {
        match n.kind() {
            "use_as_clause" => {
                // `use foo::bar as baz;` - bar aliased as baz
                if let Some(path_node) = n.child_by_field_name("path") {
                    let original_name = extract_last_path_segment(source, &path_node);
                    if let Some(alias_node) = n.child_by_field_name("alias") {
                        let local_name = slice(source, &alias_node);
                        if !local_name.is_empty() && !original_name.is_empty() {
                            bindings.push(ImportBindingInfo {
                                local_name,
                                source_file: source_file.to_string(),
                                original_name,
                                import_text: import_text.to_string(),
                            });
                        }
                    }
                }
            }
            "scoped_identifier" | "identifier" => {
                // Simple use without alias: local_name == original_name
                let name = extract_last_path_segment(source, &n);
                if !name.is_empty() && n.parent().map(|p| p.kind()) != Some("use_as_clause") {
                    bindings.push(ImportBindingInfo {
                        local_name: name.clone(),
                        source_file: source_file.to_string(),
                        original_name: name,
                        import_text: import_text.to_string(),
                    });
                }
            }
            _ => {
                let mut cursor = n.walk();
                for child in n.children(&mut cursor) {
                    stack.push(child);
                }
            }
        }
    }

    bindings
}

/// Extract the last segment of a path (e.g., "bar" from "foo::bar")
fn extract_last_path_segment(source: &str, node: &Node) -> String {
    let text = slice(source, node);
    text.rsplit("::").next().unwrap_or(&text).to_string()
}

/// Resolve a mod declaration to a file path
fn resolve_mod_path(parent: Option<&Path>, mod_name: &str) -> Option<String> {
    let parent_dir = parent?;
    let mod_file = parent_dir.join(format!("{}.rs", mod_name));
    let mod_dir_file = parent_dir.join(mod_name).join("mod.rs");

    if mod_file.exists() {
        Some(normalize_path(&mod_file))
    } else if mod_dir_file.exists() {
        Some(normalize_path(&mod_dir_file))
    } else {
        // Don't return fake paths for non-existent files
        None
    }
}

/// Extract the path from a use declaration
fn extract_use_path(source: &str, node: &Node) -> Option<String> {
    // Find the use path - could be scoped_identifier, identifier, or use_wildcard
    let mut stack = vec![*node];
    while let Some(n) = stack.pop() {
        match n.kind() {
            "scoped_identifier" | "identifier" | "scoped_use_list" => {
                return Some(slice(source, &n));
            }
            _ => {
                let mut cursor = n.walk();
                for child in n.children(&mut cursor) {
                    stack.push(child);
                }
            }
        }
    }
    None
}

/// Resolve a use path to a file path
/// Handles crate::, super::, self::, and bare module paths (e.g., `use lib::Foo`)
fn resolve_use_path(
    use_path: &str,
    current_file: &Path,
    crate_root: Option<&Path>,
) -> Option<String> {
    let parts: Vec<&str> = use_path.split("::").collect();
    if parts.is_empty() {
        return None;
    }

    let first = parts[0];
    let parent = current_file.parent()?;

    match first {
        "crate" => {
            // crate:: paths start from crate root
            let root = crate_root?;
            if parts.len() < 2 {
                // Just `use crate::*` - depend on lib.rs
                let lib_file = root.join("lib.rs");
                if lib_file.exists() {
                    return Some(normalize_path(&lib_file));
                }
                return None;
            }
            // Take the first module after crate::
            let module_name = parts[1];
            resolve_mod_path(Some(root), module_name)
        }
        "super" => {
            // super:: paths go up one directory
            let grandparent = parent.parent()?;
            if parts.len() < 2 {
                // Just `use super::*` - depend on parent mod.rs
                let mod_file = grandparent.join("mod.rs");
                if mod_file.exists() {
                    return Some(normalize_path(&mod_file));
                }
                return None;
            }
            let module_name = parts[1];
            resolve_mod_path(Some(grandparent), module_name)
        }
        "self" => {
            // self:: paths are in current module - no external dependency
            None
        }
        _ => {
            // Bare module path (e.g., `use lib::Foo` after `mod lib;`)
            // Try to resolve as a sibling module in the same directory
            if let Some(resolved) = resolve_mod_path(Some(parent), first) {
                return Some(resolved);
            }

            // Check if this is the current crate name (use crate_name::Foo from main.rs)
            // The crate name is typically the parent directory of src/
            if let Some(root) = crate_root {
                let crate_name = root
                    .parent()
                    .and_then(|p| p.file_name())
                    .and_then(|n| n.to_str())
                    .map(|s| s.replace('-', "_")); // Cargo converts hyphens to underscores

                if crate_name.as_deref() == Some(first) {
                    // This is `use crate_name::*` - resolve to lib.rs
                    let lib_file = root.join("lib.rs");
                    if lib_file.exists() {
                        return Some(normalize_path(&lib_file));
                    }
                }
            }

            // External crate or other - no local file dependency
            None
        }
    }
}

/// Find the crate root directory (where src/lib.rs or src/main.rs is)
fn find_crate_root(path: &Path) -> Option<std::path::PathBuf> {
    let mut current = path.parent()?;

    // Walk up looking for src directory with lib.rs or main.rs
    for _ in 0..10 {
        // Check if we're in a src directory
        if current.file_name().and_then(|n| n.to_str()) == Some("src") {
            return Some(current.to_path_buf());
        }

        // Check if there's a Cargo.toml here (we're at crate root)
        if current.join("Cargo.toml").exists() {
            let src = current.join("src");
            if src.exists() {
                return Some(src);
            }
            return Some(current.to_path_buf());
        }

        current = current.parent()?;
    }
    None
}

#[allow(clippy::too_many_arguments, clippy::only_used_in_recursion)]
fn walk_symbols(
    path: &Path,
    source: &str,
    cursor: &mut TreeCursor,
    container: Option<String>,
    module_path: &[String],
    impl_trait: Option<ResolvedTarget>,
    is_test_context: bool,
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
                    // Function is test if in test context or has #[test] attribute
                    let is_test = is_test_context || has_test_attr(source, &node);
                    let sym = make_symbol(
                        path,
                        module_path,
                        &node,
                        &name,
                        "function",
                        container.clone(),
                        source.as_bytes(),
                        is_test,
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
                    let sym = make_symbol(
                        path,
                        module_path,
                        &node,
                        &name,
                        "struct",
                        container.clone(),
                        source.as_bytes(),
                        is_test_context,
                    );
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
                    let sym = make_symbol(
                        path,
                        module_path,
                        &node,
                        &name,
                        "enum",
                        container.clone(),
                        source.as_bytes(),
                        is_test_context,
                    );
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
                    let sym = make_symbol(
                        path,
                        module_path,
                        &node,
                        &name,
                        "trait",
                        container.clone(),
                        source.as_bytes(),
                        is_test_context,
                    );
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
                    // Check if this module has #[cfg(test)] attribute
                    let mod_is_test = is_test_context || has_cfg_test_attr(source, &node);
                    if cursor.goto_first_child() {
                        walk_symbols(
                            path,
                            source,
                            cursor,
                            container.clone(),
                            &mod_path,
                            None,
                            mod_is_test,
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
                is_test_context,
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
    imports: &HashMap<String, &ImportBindingInfo>,
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
                    // Cross-file reference via import - create placeholder
                    // Format: {source_file}::{original_name}
                    // This will be resolved in phase 2 by resolve_and_store_references
                    let placeholder_id =
                        format!("{}::{}", import.source_file, import.original_name);
                    refs.push(ReferenceRecord {
                        file: file.clone(),
                        start: node.start_byte() as i64,
                        end: node.end_byte() as i64,
                        symbol_id: placeholder_id,
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

/// Collect call edges: edges from caller (function/method) to callee (function being called)
fn collect_call_edges(
    path: &Path,
    source: &str,
    root: &Node,
    symbol_by_name: &HashMap<String, SymbolBinding>,
    imports: &HashMap<String, &ImportBindingInfo>,
) -> Vec<EdgeRecord> {
    let mut edges = Vec::new();
    let mut stack = vec![*root];

    while let Some(node) = stack.pop() {
        // Look for call expressions and method call expressions
        if node.kind() == "call_expression" || node.kind() == "method_call_expression" {
            // Find the enclosing function (the caller)
            if let Some(caller_id) = find_enclosing_function_id(path, source, &node) {
                // Get the function being called (the callee)
                if let Some(callee_id) =
                    resolve_call_target(path, source, &node, symbol_by_name, imports)
                {
                    edges.push(EdgeRecord {
                        src: caller_id,
                        dst: callee_id,
                        kind: "calls".to_string(),
                    });
                }
            }
        }

        // Continue traversing
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            stack.push(child);
        }
    }

    edges
}

/// Find the enclosing function/method and return its symbol ID
fn find_enclosing_function_id(path: &Path, source: &str, node: &Node) -> Option<String> {
    let mut current = node.parent();

    while let Some(n) = current {
        match n.kind() {
            "function_item" => {
                if let Some(name_node) = n.child_by_field_name("name") {
                    let name = slice(source, &name_node);
                    return Some(format!("{}#{}", normalize_path(path), name));
                }
            }
            "function_signature_item" => {
                if let Some(name_node) = n.child_by_field_name("name") {
                    let name = slice(source, &name_node);
                    return Some(format!("{}#{}", normalize_path(path), name));
                }
            }
            "impl_item" => {
                // Method in an impl block - try to get both impl type and method name
                if let Some(fn_node) = find_containing_function(&n, node) {
                    if let Some(fn_name) = fn_node.child_by_field_name("name") {
                        let method_name = slice(source, &fn_name);
                        if let Some(type_node) = n.child_by_field_name("type") {
                            let type_name = slice(source, &type_node);
                            return Some(format!(
                                "{}#{}::{}",
                                normalize_path(path),
                                type_name,
                                method_name
                            ));
                        }
                        return Some(format!("{}#{}", normalize_path(path), method_name));
                    }
                }
            }
            "closure_expression" => {
                // Anonymous closure - use position-based ID
                return Some(format!(
                    "{}#closure@{}",
                    normalize_path(path),
                    n.start_byte()
                ));
            }
            _ => {}
        }
        current = n.parent();
    }

    None
}

/// Find the function_item that contains the given node within an impl block
fn find_containing_function<'a>(impl_node: &'a Node<'a>, target: &Node) -> Option<Node<'a>> {
    let mut cursor = impl_node.walk();
    let mut stack = vec![*impl_node];

    while let Some(node) = stack.pop() {
        if node.kind() == "function_item" {
            // Check if this function contains our target node
            if node.start_byte() <= target.start_byte() && node.end_byte() >= target.end_byte() {
                return Some(node);
            }
        }
        for child in node.children(&mut cursor) {
            stack.push(child);
        }
    }
    None
}

/// Resolve the target of a call expression to a symbol ID
fn resolve_call_target(
    path: &Path,
    source: &str,
    call_node: &Node,
    symbol_by_name: &HashMap<String, SymbolBinding>,
    imports: &HashMap<String, &ImportBindingInfo>,
) -> Option<String> {
    match call_node.kind() {
        "call_expression" => {
            // Function call: foo() or path::foo()
            let function_node = call_node.child_by_field_name("function")?;
            resolve_function_path(path, source, &function_node, symbol_by_name, imports)
        }
        "method_call_expression" => {
            // Method call: obj.method()
            if let Some(name_node) = call_node.child_by_field_name("name") {
                let method_name = slice(source, &name_node);
                // Try to get the receiver type for better resolution
                if let Some(receiver) = call_node.child_by_field_name("value") {
                    let receiver_text = slice(source, &receiver);
                    // Check if receiver is a known type
                    if let Some(sym) = symbol_by_name.get(&receiver_text) {
                        if let Some(q) = &sym.qualifier {
                            return Some(format!("{}::{}", q, method_name));
                        }
                        return Some(format!("{}::{}", sym.id, method_name));
                    }
                }
                // Unresolved method - use placeholder
                Some(format!("{}::{}", normalize_path(path), method_name))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Resolve a function path (identifier or scoped_identifier) to a symbol ID
fn resolve_function_path(
    path: &Path,
    source: &str,
    node: &Node,
    symbol_by_name: &HashMap<String, SymbolBinding>,
    imports: &HashMap<String, &ImportBindingInfo>,
) -> Option<String> {
    match node.kind() {
        "identifier" => {
            let name = slice(source, node);
            // Try to resolve to a known local symbol
            if let Some(sym) = symbol_by_name.get(&name) {
                return Some(sym.id.clone());
            }
            // Try to resolve via imports (cross-file call)
            if let Some(import) = imports.get(&name) {
                return Some(format!("{}::{}", import.source_file, import.original_name));
            }
            // Unresolved - use placeholder
            Some(format!("{}::{}", normalize_path(path), name))
        }
        "scoped_identifier" | "field_expression" => {
            // Path like std::io::write or module::function
            let full_path = slice(source, node);
            // Check if the full path matches a known symbol
            if let Some(sym) = symbol_by_name.get(&full_path) {
                return Some(sym.id.clone());
            }
            // Use the path as-is
            Some(full_path)
        }
        _ => None,
    }
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

#[allow(clippy::too_many_arguments)]
fn make_symbol(
    path: &Path,
    module_path: &[String],
    node: &Node,
    name: &str,
    kind: &str,
    container: Option<String>,
    source: &[u8],
    is_test: bool,
) -> SymbolRecord {
    let qualifier = Some(module_qualifier(path, module_path, &container));
    let visibility = visibility(node, path);
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

/// Check if a node has `#[cfg(test)]` attribute.
/// Looks at both children (for inner attributes) and previous siblings (for outer attributes).
fn has_cfg_test_attr(source: &str, node: &Node) -> bool {
    // Check children (inner attributes)
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "attribute_item" {
            let attr_text = slice(source, &child);
            // Match #[cfg(test)] and variations like #[cfg(all(test, ...))]
            if attr_text.contains("cfg") && attr_text.contains("test") {
                return true;
            }
        }
    }

    // Check previous siblings (outer attributes)
    let mut prev = node.prev_sibling();
    while let Some(sibling) = prev {
        if sibling.kind() == "attribute_item" {
            let attr_text = slice(source, &sibling);
            if attr_text.contains("cfg") && attr_text.contains("test") {
                return true;
            }
            prev = sibling.prev_sibling();
        } else {
            // Stop when we hit a non-attribute sibling
            break;
        }
    }

    false
}

/// Check if a node has `#[test]` attribute (including `#[tokio::test]`, `#[rstest]`, etc.).
/// Looks at both children (for inner attributes) and previous siblings (for outer attributes).
fn has_test_attr(source: &str, node: &Node) -> bool {
    // Check children (inner attributes)
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "attribute_item" {
            let attr_text = slice(source, &child);
            // Match #[test], #[tokio::test], #[rstest], #[async_std::test], etc.
            if attr_text.contains("test") {
                return true;
            }
        }
    }

    // Check previous siblings (outer attributes)
    let mut prev = node.prev_sibling();
    while let Some(sibling) = prev {
        if sibling.kind() == "attribute_item" {
            let attr_text = slice(source, &sibling);
            if attr_text.contains("test") {
                return true;
            }
            prev = sibling.prev_sibling();
        } else {
            // Stop when we hit a non-attribute sibling
            break;
        }
    }
    false
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

fn slice_file(path: &Path, node: &Node) -> String {
    // Best-effort visibility slice using the file contents; if missing, fall back to node text.
    let source = fs::read_to_string(path).unwrap_or_default();
    slice(&source, node)
}

// ============================================================================
// LanguageParser trait implementation
// ============================================================================

use super::traits::{LanguageConfig, LanguageParser, ParseResult};

/// Rust language parser implementing the `LanguageParser` trait.
#[derive(Clone)]
pub struct RustParser;

impl RustParser {
    /// Create a new Rust parser.
    pub fn new() -> Self {
        Self
    }
}

impl Default for RustParser {
    fn default() -> Self {
        Self::new()
    }
}

impl LanguageParser for RustParser {
    fn config(&self) -> LanguageConfig {
        LanguageConfig {
            name: "Rust",
            extensions: &["rs"],
        }
    }

    fn language(&self) -> &Language {
        &RUST_LANGUAGE
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

        let (symbols, edges, _refs, _deps, _imports) = index_file(&path, source).unwrap();
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

        let (symbols, edges, _refs, _deps, _imports) = index_file(&path, source).unwrap();
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

    #[test]
    fn detects_cfg_test_module() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("lib.rs");
        let source = r#"
            fn production_code() {}

            #[cfg(test)]
            mod tests {
                use super::*;

                fn test_helper() {}

                #[test]
                fn test_something() {}
            }
        "#;
        fs::write(&path, source).unwrap();

        let (symbols, _edges, _refs, _deps, _imports) = index_file(&path, source).unwrap();

        // Production function should not be marked as test
        let prod = symbols
            .iter()
            .find(|s| s.name == "production_code")
            .unwrap();
        assert!(
            !prod.is_test,
            "production_code should not be marked as test"
        );

        // Helper function inside #[cfg(test)] module should be marked as test
        let helper = symbols.iter().find(|s| s.name == "test_helper").unwrap();
        assert!(
            helper.is_test,
            "test_helper should be marked as test (inside #[cfg(test)] module)"
        );

        // Test function should be marked as test
        let test_fn = symbols.iter().find(|s| s.name == "test_something").unwrap();
        assert!(test_fn.is_test, "test_something should be marked as test");
    }

    #[test]
    fn detects_test_attribute_on_function() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("lib.rs");
        let source = r#"
            fn production_code() {}

            #[test]
            fn test_standalone() {}

            #[tokio::test]
            async fn test_async() {}

            #[rstest]
            fn test_rstest() {}
        "#;
        fs::write(&path, source).unwrap();

        let (symbols, _edges, _refs, _deps, _imports) = index_file(&path, source).unwrap();

        // Production function should not be marked as test
        let prod = symbols
            .iter()
            .find(|s| s.name == "production_code")
            .unwrap();
        assert!(
            !prod.is_test,
            "production_code should not be marked as test"
        );

        // #[test] function should be marked as test
        let test_fn = symbols
            .iter()
            .find(|s| s.name == "test_standalone")
            .unwrap();
        assert!(test_fn.is_test, "test_standalone should be marked as test");

        // #[tokio::test] function should be marked as test
        let async_test = symbols.iter().find(|s| s.name == "test_async").unwrap();
        assert!(async_test.is_test, "test_async should be marked as test");

        // #[rstest] function should be marked as test
        let rstest_fn = symbols.iter().find(|s| s.name == "test_rstest").unwrap();
        assert!(rstest_fn.is_test, "test_rstest should be marked as test");
    }

    #[test]
    fn structs_in_test_module_marked_as_test() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("lib.rs");
        let source = r#"
            struct ProdStruct;

            #[cfg(test)]
            mod tests {
                struct TestFixture {
                    value: i32,
                }

                enum TestEnum {
                    A,
                    B,
                }
            }
        "#;
        fs::write(&path, source).unwrap();

        let (symbols, _edges, _refs, _deps, _imports) = index_file(&path, source).unwrap();

        // Production struct should not be marked as test
        let prod = symbols.iter().find(|s| s.name == "ProdStruct").unwrap();
        assert!(!prod.is_test, "ProdStruct should not be marked as test");

        // Struct inside #[cfg(test)] module should be marked as test
        let fixture = symbols.iter().find(|s| s.name == "TestFixture").unwrap();
        assert!(fixture.is_test, "TestFixture should be marked as test");

        // Enum inside #[cfg(test)] module should be marked as test
        let test_enum = symbols.iter().find(|s| s.name == "TestEnum").unwrap();
        assert!(test_enum.is_test, "TestEnum should be marked as test");
    }
}
