use std::fs;
use std::process::Command;

use gabb_cli::{indexer, store::IndexStore};
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
    indexer::build_full_index(root, &store).unwrap();

    // Verify dependencies are recorded
    let deps = store.get_all_dependencies().unwrap();
    assert!(
        !deps.is_empty(),
        "expected file dependencies to be recorded"
    );

    // Verify file dependency was correctly resolved with .ts extension
    let utils_canonical = utils_path.canonicalize().unwrap();
    let utils_str = utils_canonical.to_string_lossy().to_string();
    let dependents = store.get_dependents(&utils_str).unwrap();
    assert!(
        !dependents.is_empty(),
        "expected dependents for utils.ts, deps: {:?}",
        deps
    );

    let bin = env!("CARGO_BIN_EXE_gabb-cli");

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

fn offset_to_line_char(buf: &[u8], offset: usize) -> Option<(usize, usize)> {
    let mut line = 1usize;
    let mut col = 1usize;
    for (i, b) in buf.iter().enumerate() {
        if i == offset {
            return Some((line, col));
        }
        if *b == b'\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    if offset == buf.len() {
        Some((line, col))
    } else {
        None
    }
}

#[test]
fn symbols_and_implementation_commands_work() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    let ts_path = root.join("foo.ts");
    fs::write(&ts_path, "function foo() {}\nfoo();\n").unwrap();

    let db_path = root.join(".gabb/index.db");
    let store = IndexStore::open(&db_path).unwrap();
    indexer::build_full_index(root, &store).unwrap();
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
    let (line, character) = offset_to_line_char(&contents, symbol.start as usize).unwrap();

    let bin = env!("CARGO_BIN_EXE_gabb-cli");

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
    assert!(
        impl_out.status.success(),
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
    indexer::build_full_index(root, &store).unwrap();

    // Get the invalidation set for base.ts - should include derived.ts
    let base_canonical = base_path.canonicalize().unwrap();
    let base_str = base_canonical.to_string_lossy().to_string();
    let invalidation_set = store.get_invalidation_set(&base_str).unwrap();

    // Verify that derived.ts is in the invalidation set
    let derived_canonical = derived_path.canonicalize().unwrap();
    let derived_str = derived_canonical.to_string_lossy().to_string();
    assert!(
        invalidation_set.contains(&derived_str),
        "expected derived.ts in invalidation set for base.ts, got: {:?}",
        invalidation_set
    );

    // Verify transitive invalidation - consumer.ts should also be affected
    // when base.ts changes (via derived.ts)
    let consumer_canonical = consumer_path.canonicalize().unwrap();
    let consumer_str = consumer_canonical.to_string_lossy().to_string();
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
    indexer::build_full_index(root, &store).unwrap();

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
    assert!(deps.len() >= 2, "expected at least 2 dependencies for circular imports");

    // Verify topological sort handles cycles gracefully
    let all_files: Vec<String> = deps.iter().map(|d| d.from_file.clone()).collect();
    let sorted = store.topological_sort(&all_files).unwrap();
    assert!(
        !sorted.is_empty(),
        "topological sort should return files even with cycles"
    );

    // Verify invalidation set doesn't infinite loop
    let a_canonical = a_path.canonicalize().unwrap();
    let a_str = a_canonical.to_string_lossy().to_string();
    let invalidation_set = store.get_invalidation_set(&a_str).unwrap();

    // Both files should be in each other's invalidation sets
    let b_canonical = b_path.canonicalize().unwrap();
    let b_str = b_canonical.to_string_lossy().to_string();
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
    indexer::build_full_index(root, &store).unwrap();

    // Get the symbol ID for 'helper' in utils.ts
    let utils_symbols = store.list_symbols(
        Some(&utils_path.canonicalize().unwrap().to_string_lossy()),
        None,
        Some("helper"),
        None,
    ).unwrap();
    assert!(!utils_symbols.is_empty(), "helper should be indexed in utils.ts");
    let helper_symbol_id = &utils_symbols[0].id;

    // Get references for the helper symbol - should include the aliased usage in main.ts
    let refs = store.references_for_symbol(helper_symbol_id).unwrap();

    // The reference in main.ts (where 'h()' is called) should resolve to the helper symbol
    // thanks to two-phase indexing
    let main_canonical = main_path.canonicalize().unwrap();
    let main_str = main_canonical.to_string_lossy().to_string();

    // Check if any reference is from main.ts
    let main_refs: Vec<_> = refs.iter().filter(|r| r.file == main_str).collect();
    assert!(
        !main_refs.is_empty(),
        "expected reference in main.ts to resolve to helper symbol, got refs: {:?}",
        refs
    );
}
