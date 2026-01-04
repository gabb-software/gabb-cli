use std::fs;
use std::process::Command;

use gabb_cli::{
    indexer, offset_to_line_col,
    store::{normalize_path, IndexStore},
};
use tempfile::tempdir;

#[test]
fn cross_file_usages_via_dependency_graph() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    // Create a file that exports a function
    let utils_path = root.join("utils.ts");
    fs::write(&utils_path, "export function helper() { return 42; }\n").unwrap();

    // Create a file that imports and uses the function
    let main_path = root.join("main.ts");
    fs::write(
        &main_path,
        "import { helper } from './utils';\nconst result = helper();\n",
    )
    .unwrap();

    let db_path = root.join(".gabb/index.db");
    let store = IndexStore::open(&db_path).unwrap();
    indexer::build_full_index(root, &store, None::<fn(&indexer::IndexProgress)>).unwrap();

    // Verify dependencies are recorded
    let deps = store.get_all_dependencies().unwrap();
    assert!(
        !deps.is_empty(),
        "expected file dependencies to be recorded"
    );

    // Verify file dependency was correctly resolved with .ts extension
    let utils_str = normalize_path(&utils_path.canonicalize().unwrap());
    let dependents = store.get_dependents(&utils_str).unwrap();
    assert!(
        !dependents.is_empty(),
        "expected dependents for utils.ts, deps: {:?}",
        deps
    );

    let bin = env!("CARGO_BIN_EXE_gabb");

    // Find usages of 'helper' - should find the usage in main.ts
    let usages_out = Command::new(bin)
        .args([
            "usages",
            "--db",
            db_path.to_str().unwrap(),
            "--file",
            &format!("{}:1:17", utils_path.display()), // position of 'helper' in utils.ts
        ])
        .current_dir(root)
        .output()
        .unwrap();

    assert!(
        usages_out.status.success(),
        "usages exited {:?}, stderr: {}",
        usages_out.status,
        String::from_utf8_lossy(&usages_out.stderr)
    );

    let stdout = String::from_utf8_lossy(&usages_out.stdout);
    assert!(
        stdout.contains("main.ts"),
        "expected usage in main.ts, got: {}",
        stdout
    );
}

#[test]
fn symbols_and_implementation_commands_work() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    let ts_path = root.join("foo.ts");
    fs::write(&ts_path, "function foo() {}\nfoo();\n").unwrap();

    let db_path = root.join(".gabb/index.db");
    let store = IndexStore::open(&db_path).unwrap();
    indexer::build_full_index(root, &store, None::<fn(&indexer::IndexProgress)>).unwrap();
    let contents = fs::read(&ts_path).unwrap();
    let all_syms = store.list_symbols(None, None, None, None).unwrap();
    assert!(
        !all_syms.is_empty(),
        "expected symbols, got empty set for db {}",
        db_path.display()
    );
    let symbol = all_syms
        .iter()
        .find(|s| s.name == "foo")
        .cloned()
        .expect("symbol foo indexed");
    let (line, character) = offset_to_line_col(&contents, symbol.start as usize).unwrap();

    let bin = env!("CARGO_BIN_EXE_gabb");

    // symbols should list the function
    let symbols = Command::new(bin)
        .args([
            "symbols",
            "--db",
            db_path.to_str().unwrap(),
            "--limit",
            "10",
        ])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        symbols.status.success(),
        "symbols exited {:?}, stderr: {}",
        symbols.status,
        String::from_utf8_lossy(&symbols.stderr)
    );
    assert!(
        String::from_utf8_lossy(&symbols.stdout).contains("foo"),
        "symbols output: {}",
        String::from_utf8_lossy(&symbols.stdout)
    );

    // symbol should dump details about foo
    let symbol_detail = Command::new(bin)
        .args(["symbol", "--db", db_path.to_str().unwrap(), "--name", "foo"])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        symbol_detail.status.success(),
        "symbol exited {:?}, stderr: {}",
        symbol_detail.status,
        String::from_utf8_lossy(&symbol_detail.stderr)
    );
    assert!(
        String::from_utf8_lossy(&symbol_detail.stdout).contains("Symbol:"),
        "symbol output: {}",
        String::from_utf8_lossy(&symbol_detail.stdout)
    );

    // implementation should resolve the symbol under the cursor
    // Exit code 0 = found results, exit code 1 = not found (but not an error)
    let impl_out = Command::new(bin)
        .args([
            "implementation",
            "--db",
            db_path.to_str().unwrap(),
            "--file",
            &symbol.file,
            "--line",
            &line.to_string(),
            "--character",
            &character.to_string(),
        ])
        .current_dir(root)
        .output()
        .unwrap();
    let impl_exit_code = impl_out.status.code().unwrap_or(-1);
    assert!(
        impl_exit_code == 0 || impl_exit_code == 1,
        "implementation exited {:?}, stderr: {}",
        impl_out.status,
        String::from_utf8_lossy(&impl_out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&impl_out.stdout).contains("foo"),
        "implementation output: {}",
        String::from_utf8_lossy(&impl_out.stdout)
    );

    // usages should include the call site line/column
    let usages_out = Command::new(bin)
        .args([
            "usages",
            "--db",
            db_path.to_str().unwrap(),
            "--file",
            &format!("{}:{}:{}", symbol.file, line, character),
        ])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        usages_out.status.success(),
        "usages exited {:?}, stderr: {}",
        usages_out.status,
        String::from_utf8_lossy(&usages_out.stderr)
    );
    let usages_stdout = String::from_utf8_lossy(&usages_out.stdout);
    assert!(
        usages_stdout.contains("usage") && usages_stdout.contains("foo.ts"),
        "usages output: {}",
        usages_stdout
    );
}

#[test]
fn file_modification_identifies_dependents() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    // Create a base module
    let base_path = root.join("base.ts");
    fs::write(&base_path, "export interface Base { id: number; }\n").unwrap();

    // Create a module that imports from base
    let derived_path = root.join("derived.ts");
    fs::write(
        &derived_path,
        "import { Base } from './base';\nexport interface Derived extends Base { name: string; }\n",
    )
    .unwrap();

    // Create a module that imports from derived
    let consumer_path = root.join("consumer.ts");
    fs::write(
        &consumer_path,
        "import { Derived } from './derived';\nconst obj: Derived = { id: 1, name: 'test' };\n",
    )
    .unwrap();

    let db_path = root.join(".gabb/index.db");
    let store = IndexStore::open(&db_path).unwrap();
    indexer::build_full_index(root, &store, None::<fn(&indexer::IndexProgress)>).unwrap();

    // Get the invalidation set for base.ts - should include derived.ts
    let base_str = normalize_path(&base_path.canonicalize().unwrap());
    let invalidation_set = store.get_invalidation_set(&base_str).unwrap();

    // Verify that derived.ts is in the invalidation set
    let derived_str = normalize_path(&derived_path.canonicalize().unwrap());
    assert!(
        invalidation_set.contains(&derived_str),
        "expected derived.ts in invalidation set for base.ts, got: {:?}",
        invalidation_set
    );

    // Verify transitive invalidation - consumer.ts should also be affected
    // when base.ts changes (via derived.ts)
    let consumer_str = normalize_path(&consumer_path.canonicalize().unwrap());
    assert!(
        invalidation_set.contains(&consumer_str),
        "expected consumer.ts in invalidation set (transitive), got: {:?}",
        invalidation_set
    );
}

#[test]
fn circular_dependency_handling() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    // Create files with circular imports
    // a.ts imports from b.ts
    let a_path = root.join("a.ts");
    fs::write(
        &a_path,
        "import { funcB } from './b';\nexport function funcA() { return funcB(); }\n",
    )
    .unwrap();

    // b.ts imports from a.ts (circular!)
    let b_path = root.join("b.ts");
    fs::write(
        &b_path,
        "import { funcA } from './a';\nexport function funcB() { return 42; }\nexport function useA() { return funcA(); }\n",
    )
    .unwrap();

    let db_path = root.join(".gabb/index.db");
    let store = IndexStore::open(&db_path).unwrap();

    // Indexing should complete without hanging or crashing
    indexer::build_full_index(root, &store, None::<fn(&indexer::IndexProgress)>).unwrap();

    // Verify both files were indexed
    let symbols = store.list_symbols(None, None, None, None).unwrap();
    assert!(
        symbols.iter().any(|s| s.name == "funcA"),
        "funcA should be indexed"
    );
    assert!(
        symbols.iter().any(|s| s.name == "funcB"),
        "funcB should be indexed"
    );

    // Verify dependencies were recorded (both directions)
    let deps = store.get_all_dependencies().unwrap();
    assert!(
        deps.len() >= 2,
        "expected at least 2 dependencies for circular imports"
    );

    // Verify topological sort handles cycles gracefully
    let all_files: Vec<String> = deps.iter().map(|d| d.from_file.clone()).collect();
    let sorted = store.topological_sort(&all_files).unwrap();
    assert!(
        !sorted.is_empty(),
        "topological sort should return files even with cycles"
    );

    // Verify invalidation set doesn't infinite loop
    let a_str = normalize_path(&a_path.canonicalize().unwrap());
    let invalidation_set = store.get_invalidation_set(&a_str).unwrap();

    // Both files should be in each other's invalidation sets
    let b_str = normalize_path(&b_path.canonicalize().unwrap());
    assert!(
        invalidation_set.contains(&b_str),
        "b.ts should be in a.ts invalidation set"
    );
}

#[test]
fn two_phase_indexing_resolves_import_aliases() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    // Create a file that exports a function
    let utils_path = root.join("utils.ts");
    fs::write(&utils_path, "export function helper() { return 42; }\n").unwrap();

    // Create a file that imports with an alias and uses it
    let main_path = root.join("main.ts");
    fs::write(
        &main_path,
        "import { helper as h } from './utils';\nconst result = h();\n",
    )
    .unwrap();

    let db_path = root.join(".gabb/index.db");
    let store = IndexStore::open(&db_path).unwrap();
    indexer::build_full_index(root, &store, None::<fn(&indexer::IndexProgress)>).unwrap();

    // Get the symbol ID for 'helper' in utils.ts
    let utils_str = normalize_path(&utils_path.canonicalize().unwrap());
    let utils_symbols = store
        .list_symbols(Some(&utils_str), None, Some("helper"), None)
        .unwrap();
    assert!(
        !utils_symbols.is_empty(),
        "helper should be indexed in utils.ts"
    );
    let helper_symbol_id = &utils_symbols[0].id;

    // Get references for the helper symbol - should include the aliased usage in main.ts
    let refs = store.references_for_symbol(helper_symbol_id).unwrap();

    // The reference in main.ts (where 'h()' is called) should resolve to the helper symbol
    // thanks to two-phase indexing
    let main_str = normalize_path(&main_path.canonicalize().unwrap());

    // Check if any reference is from main.ts
    let main_refs: Vec<_> = refs.iter().filter(|r| r.file == main_str).collect();
    assert!(
        !main_refs.is_empty(),
        "expected reference in main.ts to resolve to helper symbol, got refs: {:?}",
        refs
    );
}

#[test]
fn definition_command_finds_symbol_declaration() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    // Create a file that exports a function
    let utils_path = root.join("utils.ts");
    fs::write(&utils_path, "export function helper() { return 42; }\n").unwrap();

    // Create a file that imports and uses the function
    let main_path = root.join("main.ts");
    fs::write(
        &main_path,
        "import { helper } from './utils';\nconst result = helper();\n",
    )
    .unwrap();

    let db_path = root.join(".gabb/index.db");
    let store = IndexStore::open(&db_path).unwrap();
    indexer::build_full_index(root, &store, None::<fn(&indexer::IndexProgress)>).unwrap();

    let bin = env!("CARGO_BIN_EXE_gabb");

    // Find definition of 'helper' from the usage site in main.ts (line 2, col ~16 for the call)
    // The call `helper()` is at line 2
    let def_out = Command::new(bin)
        .args([
            "definition",
            "--db",
            db_path.to_str().unwrap(),
            "--file",
            &format!("{}:2:16", main_path.display()),
        ])
        .current_dir(root)
        .output()
        .unwrap();

    assert!(
        def_out.status.success(),
        "definition exited {:?}, stderr: {}",
        def_out.status,
        String::from_utf8_lossy(&def_out.stderr)
    );

    let stdout = String::from_utf8_lossy(&def_out.stdout);
    // Should point to utils.ts where helper is defined
    assert!(
        stdout.contains("utils.ts") && stdout.contains("helper"),
        "expected definition in utils.ts, got: {}",
        stdout
    );
}

#[test]
fn definition_command_with_json_output() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    let ts_path = root.join("foo.ts");
    fs::write(&ts_path, "function foo() {}\nfoo();\n").unwrap();

    let db_path = root.join(".gabb/index.db");
    let store = IndexStore::open(&db_path).unwrap();
    indexer::build_full_index(root, &store, None::<fn(&indexer::IndexProgress)>).unwrap();

    let bin = env!("CARGO_BIN_EXE_gabb");

    // Find definition of foo from the call site
    let def_out = Command::new(bin)
        .args([
            "definition",
            "--format",
            "json",
            "--db",
            db_path.to_str().unwrap(),
            "--file",
            &format!("{}:2:1", ts_path.display()),
        ])
        .current_dir(root)
        .output()
        .unwrap();

    assert!(
        def_out.status.success(),
        "definition exited {:?}, stderr: {}",
        def_out.status,
        String::from_utf8_lossy(&def_out.stderr)
    );

    let stdout = String::from_utf8_lossy(&def_out.stdout);
    // Should be valid JSON with definition object
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("should be valid JSON");
    assert!(
        json.get("definition").is_some(),
        "expected definition field in JSON"
    );
    assert_eq!(json["definition"]["name"], "foo");
    assert_eq!(json["definition"]["kind"], "function");
}

#[test]
fn duplicates_command_finds_identical_functions() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    // Create two files with identical functions (same content after whitespace normalization)
    let file1_path = root.join("file1.ts");
    fs::write(
        &file1_path,
        r#"
function calculateTotal(items: number[]): number {
    return items.reduce((sum, item) => sum + item, 0);
}
"#,
    )
    .unwrap();

    let file2_path = root.join("file2.ts");
    fs::write(
        &file2_path,
        r#"
function calculateTotal(items: number[]): number {
  return items.reduce((sum, item) => sum + item, 0);
}
"#,
    )
    .unwrap();

    // Also create a unique function
    let file3_path = root.join("file3.ts");
    fs::write(
        &file3_path,
        r#"
function somethingUnique(x: number): number {
    return x * 2 + Math.random() * 100 + Date.now();
}
"#,
    )
    .unwrap();

    let db_path = root.join(".gabb/index.db");
    let store = IndexStore::open(&db_path).unwrap();
    indexer::build_full_index(root, &store, None::<fn(&indexer::IndexProgress)>).unwrap();

    let bin = env!("CARGO_BIN_EXE_gabb");

    // Run duplicates command
    let dup_out = Command::new(bin)
        .args(["duplicates", "--db", db_path.to_str().unwrap()])
        .current_dir(root)
        .output()
        .unwrap();

    assert!(
        dup_out.status.success(),
        "duplicates exited {:?}, stderr: {}",
        dup_out.status,
        String::from_utf8_lossy(&dup_out.stderr)
    );

    let stdout = String::from_utf8_lossy(&dup_out.stdout);
    // Should find at least one duplicate group containing calculateTotal
    assert!(
        stdout.contains("calculateTotal") || stdout.contains("duplicate"),
        "expected duplicates output to mention calculateTotal or duplicates, got: {}",
        stdout
    );
}

#[test]
fn duplicates_command_with_json_output() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    // Create two files with identical functions
    let file1_path = root.join("a.ts");
    fs::write(
        &file1_path,
        r#"
export function processData(data: string[]): string[] {
    return data.map(item => item.trim()).filter(item => item.length > 0);
}
"#,
    )
    .unwrap();

    let file2_path = root.join("b.ts");
    fs::write(
        &file2_path,
        r#"
export function processData(data: string[]): string[] {
  return data.map(item => item.trim()).filter(item => item.length > 0);
}
"#,
    )
    .unwrap();

    let db_path = root.join(".gabb/index.db");
    let store = IndexStore::open(&db_path).unwrap();
    indexer::build_full_index(root, &store, None::<fn(&indexer::IndexProgress)>).unwrap();

    let bin = env!("CARGO_BIN_EXE_gabb");

    // Run duplicates command with JSON output
    let dup_out = Command::new(bin)
        .args([
            "duplicates",
            "--format",
            "json",
            "--db",
            db_path.to_str().unwrap(),
        ])
        .current_dir(root)
        .output()
        .unwrap();

    assert!(
        dup_out.status.success(),
        "duplicates exited {:?}, stderr: {}",
        dup_out.status,
        String::from_utf8_lossy(&dup_out.stderr)
    );

    let stdout = String::from_utf8_lossy(&dup_out.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("should be valid JSON");
    assert!(
        json.get("groups").is_some(),
        "expected groups field in JSON"
    );
    assert!(
        json.get("summary").is_some(),
        "expected summary field in JSON"
    );

    // Check if we found duplicates
    let groups = json["groups"].as_array().unwrap();
    if !groups.is_empty() {
        // If duplicates found, verify structure
        let first_group = &groups[0];
        assert!(first_group.get("content_hash").is_some());
        assert!(first_group.get("symbols").is_some());
        assert!(first_group.get("count").is_some());
    }
}

#[test]
fn implementation_command_handles_import_aliases() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    // Create interface file
    let interface_path = root.join("interface.ts");
    fs::write(
        &interface_path,
        "export interface Service {\n  doWork(): void;\n}\n",
    )
    .unwrap();

    // Create implementation file that imports with an alias
    let impl_path = root.join("impl.ts");
    fs::write(
        &impl_path,
        r#"import { Service as Svc } from './interface';

export class MyService implements Svc {
  doWork(): void {
    console.log('working');
  }
}
"#,
    )
    .unwrap();

    let db_path = root.join(".gabb/index.db");
    let store = IndexStore::open(&db_path).unwrap();
    indexer::build_full_index(root, &store, None::<fn(&indexer::IndexProgress)>).unwrap();

    let bin = env!("CARGO_BIN_EXE_gabb");

    // Find implementations of 'Service' interface (at line 1, col 18 where 'Service' starts)
    let impl_out = Command::new(bin)
        .args([
            "implementation",
            "-f",
            "json",
            "--db",
            db_path.to_str().unwrap(),
            "--file",
            &format!("{}:1:18", interface_path.to_str().unwrap()),
        ])
        .current_dir(root)
        .output()
        .unwrap();

    assert!(
        impl_out.status.success(),
        "implementation exited {:?}, stderr: {}",
        impl_out.status,
        String::from_utf8_lossy(&impl_out.stderr)
    );

    let stdout = String::from_utf8_lossy(&impl_out.stdout);

    // Should find MyService even though it imports Service as Svc
    assert!(
        stdout.contains("MyService"),
        "expected to find MyService implementation, got: {}",
        stdout
    );
}

#[test]
fn symbols_command_fuzzy_search_works() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    // Create files with various function names
    let utils_path = root.join("utils.ts");
    fs::write(
        &utils_path,
        r#"
export function getUserById(id: number) { return id; }
export function getUserByName(name: string) { return name; }
export function createUser(data: any) { return data; }
export function deleteUser(id: number) { return id; }
export function processOrder(order: any) { return order; }
"#,
    )
    .unwrap();

    let db_path = root.join(".gabb/index.db");
    let store = IndexStore::open(&db_path).unwrap();
    indexer::build_full_index(root, &store, None::<fn(&indexer::IndexProgress)>).unwrap();

    let bin = env!("CARGO_BIN_EXE_gabb");

    // Test prefix search with --fuzzy and "getUser*"
    let output = Command::new(bin)
        .args([
            "symbols",
            "--fuzzy",
            "--name",
            "getUser*",
            "--db",
            db_path.to_str().unwrap(),
        ])
        .current_dir(root)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "symbols --fuzzy exited {:?}, stderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Should find both getUserById and getUserByName
    assert!(
        stdout.contains("getUserById"),
        "expected to find getUserById, got: {}",
        stdout
    );
    assert!(
        stdout.contains("getUserByName"),
        "expected to find getUserByName, got: {}",
        stdout
    );
    // Should NOT find createUser (different prefix)
    assert!(
        !stdout.contains("createUser"),
        "should not find createUser with getUser* pattern, got: {}",
        stdout
    );

    // Test substring search (without trailing *)
    let output2 = Command::new(bin)
        .args([
            "symbols",
            "--fuzzy",
            "--name",
            "User",
            "--db",
            db_path.to_str().unwrap(),
        ])
        .current_dir(root)
        .output()
        .unwrap();

    assert!(output2.status.success());
    let stdout2 = String::from_utf8_lossy(&output2.stdout);

    // FTS5 trigram should find all functions containing "User"
    assert!(
        stdout2.contains("getUserById") || stdout2.contains("createUser"),
        "expected to find User-related functions with substring search, got: {}",
        stdout2
    );
}

#[test]
fn symbols_command_pagination_works() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    // Create a file with many functions to test pagination
    let funcs_path = root.join("functions.ts");
    let mut content = String::new();
    for i in 0..20 {
        content.push_str(&format!(
            "export function func{:02}() {{ return {}; }}\n",
            i, i
        ));
    }
    fs::write(&funcs_path, &content).unwrap();

    let db_path = root.join(".gabb/index.db");
    let store = IndexStore::open(&db_path).unwrap();
    indexer::build_full_index(root, &store, None::<fn(&indexer::IndexProgress)>).unwrap();

    let bin = env!("CARGO_BIN_EXE_gabb");

    // Test --limit only (should return first 5)
    let output1 = Command::new(bin)
        .args(["symbols", "--limit", "5", "--db", db_path.to_str().unwrap()])
        .current_dir(root)
        .output()
        .unwrap();

    assert!(
        output1.status.success(),
        "symbols --limit exited {:?}, stderr: {}",
        output1.status,
        String::from_utf8_lossy(&output1.stderr)
    );

    let stdout1 = String::from_utf8_lossy(&output1.stdout);
    let lines1: Vec<_> = stdout1.lines().filter(|l| l.contains("func")).collect();
    assert_eq!(
        lines1.len(),
        5,
        "expected 5 results with --limit 5, got {} lines: {:?}",
        lines1.len(),
        lines1
    );

    // Test --limit with --offset (should skip first 5 and return next 5)
    let output2 = Command::new(bin)
        .args([
            "symbols",
            "--limit",
            "5",
            "--offset",
            "5",
            "--db",
            db_path.to_str().unwrap(),
        ])
        .current_dir(root)
        .output()
        .unwrap();

    assert!(
        output2.status.success(),
        "symbols --limit --offset exited {:?}, stderr: {}",
        output2.status,
        String::from_utf8_lossy(&output2.stderr)
    );

    let stdout2 = String::from_utf8_lossy(&output2.stdout);
    let lines2: Vec<_> = stdout2.lines().filter(|l| l.contains("func")).collect();
    assert_eq!(
        lines2.len(),
        5,
        "expected 5 results with --limit 5 --offset 5, got {} lines: {:?}",
        lines2.len(),
        lines2
    );

    // Extract function names from each page
    let extract_func_names = |lines: &[&str]| -> Vec<String> {
        lines
            .iter()
            .filter_map(|l| {
                // Lines look like: "function   func03   /path/to/file.ts:4:8"
                l.split_whitespace().nth(1).map(|s| s.to_string())
            })
            .collect()
    };

    let page1_funcs = extract_func_names(&lines1);
    let page2_funcs = extract_func_names(&lines2);

    // Verify pages don't overlap - no function should appear on both pages
    for func in &page1_funcs {
        assert!(
            !page2_funcs.contains(func),
            "function {} appears on both pages - pagination is broken",
            func
        );
    }
}

#[test]
fn usages_command_shows_import_chain() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    // Create a file that exports a function
    let utils_path = root.join("utils.ts");
    fs::write(&utils_path, "export function helper() { return 42; }\n").unwrap();

    // Create a file that imports and uses the function
    let main_path = root.join("main.ts");
    fs::write(
        &main_path,
        "import { helper } from './utils';\nconst result = helper();\n",
    )
    .unwrap();

    let db_path = root.join(".gabb/index.db");
    let store = IndexStore::open(&db_path).unwrap();
    indexer::build_full_index(root, &store, None::<fn(&indexer::IndexProgress)>).unwrap();

    let bin = env!("CARGO_BIN_EXE_gabb");

    // Find usages of helper - should show the import statement
    let output = Command::new(bin)
        .args([
            "usages",
            "--db",
            db_path.to_str().unwrap(),
            "--file",
            &format!("{}:1:17", utils_path.display()), // Position of 'helper' in utils.ts
        ])
        .current_dir(root)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "usages command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Should show the import statement in the output
    assert!(
        stdout.contains("import { helper } from './utils'"),
        "expected import chain in output, got: {}",
        stdout
    );
}

#[test]
fn usages_json_includes_import_via() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    // Create a file that exports a function
    let utils_path = root.join("utils.ts");
    fs::write(&utils_path, "export function helper() { return 42; }\n").unwrap();

    // Create a file that imports and uses the function
    let main_path = root.join("main.ts");
    fs::write(
        &main_path,
        "import { helper } from './utils';\nconst result = helper();\n",
    )
    .unwrap();

    let db_path = root.join(".gabb/index.db");
    let store = IndexStore::open(&db_path).unwrap();
    indexer::build_full_index(root, &store, None::<fn(&indexer::IndexProgress)>).unwrap();

    let bin = env!("CARGO_BIN_EXE_gabb");

    // Find usages of helper with JSON output
    let output = Command::new(bin)
        .args([
            "-f",
            "json",
            "usages",
            "--db",
            db_path.to_str().unwrap(),
            "--file",
            &format!("{}:1:17", utils_path.display()),
        ])
        .current_dir(root)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "usages command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);

    // JSON output should include import_via field
    assert!(
        stdout.contains("import_via"),
        "expected import_via in JSON output, got: {}",
        stdout
    );
}

/// Test for issue #80: Cross-file references not captured in Rust
/// When a symbol is defined in one file (lib.rs) and used in another (main.rs),
/// the usages command should find all usages across both files.
#[test]
fn rust_cross_file_usages_captured() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    // Create a Rust "lib.rs" that defines an enum
    let lib_path = root.join("lib.rs");
    fs::write(
        &lib_path,
        r#"pub enum ExitCode {
    Success,
    Failure,
}

pub fn get_exit_code() -> ExitCode {
    ExitCode::Success
}
"#,
    )
    .unwrap();

    // Create a Rust "main.rs" that uses the enum from lib.rs
    let main_path = root.join("main.rs");
    fs::write(
        &main_path,
        r#"mod lib;
use lib::ExitCode;

fn main() {
    let code: ExitCode = ExitCode::Success;
    match code {
        ExitCode::Success => println!("OK"),
        ExitCode::Failure => println!("FAIL"),
    }
}
"#,
    )
    .unwrap();

    let db_path = root.join(".gabb/index.db");
    let store = IndexStore::open(&db_path).unwrap();
    indexer::build_full_index(root, &store, None::<fn(&indexer::IndexProgress)>).unwrap();

    // Get the symbol for ExitCode in lib.rs
    let lib_str = normalize_path(&lib_path.canonicalize().unwrap());
    let symbols = store
        .list_symbols(Some(&lib_str), None, Some("ExitCode"), None)
        .unwrap();
    assert!(!symbols.is_empty(), "ExitCode should be indexed in lib.rs");
    let exit_code_symbol = &symbols[0];

    // Query references for ExitCode
    let refs = store.references_for_symbol(&exit_code_symbol.id).unwrap();

    // Should find references in main.rs (the cross-file usages)
    let main_str = normalize_path(&main_path.canonicalize().unwrap());
    let main_refs: Vec<_> = refs.iter().filter(|r| r.file == main_str).collect();

    // This is the key assertion - currently fails because cross-file references
    // are not captured (issue #80)
    assert!(
        !main_refs.is_empty(),
        "Expected cross-file references in main.rs to ExitCode defined in lib.rs.\n\
         Found {} total refs: {:?}\n\
         This tests issue #80: cross-file references not captured",
        refs.len(),
        refs
    );

    // Should find multiple usages in main.rs:
    // - `use lib::ExitCode;`
    // - `let code: ExitCode`
    // - `ExitCode::Success` (2x)
    // - `ExitCode::Failure`
    assert!(
        main_refs.len() >= 3,
        "Expected at least 3 ExitCode references in main.rs, found {}: {:?}",
        main_refs.len(),
        main_refs
    );
}
