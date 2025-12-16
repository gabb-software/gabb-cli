/// Tests demonstrating the test harness functionality
mod support;

use support::*;

#[test]
fn test_workspace_builder_with_inline_files() {
    let ws = TestWorkspace::builder()
        .with_file("hello.ts", "export function greet() { return 'hi'; }")
        .with_file("main.ts", "import { greet } from './hello';\ngreet();")
        .build()
        .unwrap();

    // Verify files exist
    assert!(ws.root().join("hello.ts").exists());
    assert!(ws.root().join("main.ts").exists());

    // Verify index contains symbols
    let symbols = ws.store().list_symbols(None, None, None, None).unwrap();
    assert!(symbols.iter().any(|s| s.name == "greet"));
}

#[test]
fn test_workspace_builder_with_ts_file_builder() {
    let ws = TestWorkspace::builder()
        .with_ts_file("utils.ts")
        .with_function("helper", "return 42")
        .with_interface("Config", "debug: boolean")
        .done()
        .with_ts_file("main.ts")
        .importing("./utils", &["helper", "Config"])
        .with_body("const x = helper();")
        .done()
        .build()
        .unwrap();

    let snapshot = ws.snapshot();

    // Check symbols were indexed
    assert!(snapshot.has_symbol("helper", "function"));
    assert!(snapshot.has_symbol("Config", "interface"));
}

#[test]
fn test_workspace_builder_with_rs_file_builder() {
    let ws = TestWorkspace::builder()
        .with_rs_file("lib.rs")
        .with_pub_fn("calculate", "42")
        .with_struct("Config", "pub debug: bool")
        .done()
        .build()
        .unwrap();

    let snapshot = ws.snapshot();

    // Check symbols were indexed
    assert!(snapshot.has_symbol("calculate", "function"));
    assert!(snapshot.has_symbol("Config", "struct"));
}

#[test]
fn test_cli_runner_symbols_command() {
    let ws = TestWorkspace::builder()
        .with_file("test.ts", "function foo() {}\nfunction bar() {}")
        .build()
        .unwrap();

    let output = ws.cli().symbols().run().unwrap();

    output
        .assert_success()
        .assert_stdout_contains("foo")
        .assert_stdout_contains("bar");
}

#[test]
fn test_cli_runner_json_output() {
    let ws = TestWorkspace::builder()
        .with_file("test.ts", "export function myFunc() { return 1; }")
        .build()
        .unwrap();

    let output = ws.cli().symbols().json().run().unwrap();

    output.assert_success();

    let json: serde_json::Value = output.json_value().unwrap();
    assert!(json.is_array());

    let arr = json.as_array().unwrap();
    assert!(arr.iter().any(|s| s["name"] == "myFunc"));
}

#[test]
fn test_fixture_loading() {
    let ws = TestWorkspace::builder()
        .from_fixture("cross_file_imports")
        .build()
        .unwrap();

    // Verify fixture files were created
    assert!(ws.root().join("utils.ts").exists());
    assert!(ws.root().join("main.ts").exists());
    assert!(ws.root().join("aliased.ts").exists());

    // Verify symbols
    let snapshot = ws.snapshot();
    assert!(snapshot.has_symbol("helper", "function"));
    assert!(snapshot.has_symbol("Config", "interface"));
}

#[test]
fn test_snapshot_capture() {
    let ws = TestWorkspace::builder()
        .with_ts_file("api.ts")
        .with_function("fetchData", "return []")
        .with_interface("User", "id: number; name: string")
        .done()
        .build()
        .unwrap();

    let snapshot = ws.snapshot();

    // Check symbol names
    let names = snapshot.symbol_names();
    assert!(names.contains(&"fetchData"));
    assert!(names.contains(&"User"));

    // Serialize to YAML and back
    let yaml = snapshot.to_yaml();
    assert!(yaml.contains("fetchData"));
    assert!(yaml.contains("User"));
}

#[test]
fn test_workspace_write_and_reindex() {
    let ws = TestWorkspace::builder()
        .with_file("initial.ts", "function first() {}")
        .build()
        .unwrap();

    // Initial state
    let snapshot1 = ws.snapshot();
    assert!(snapshot1.has_symbol("first", "function"));

    // Add a new file
    ws.write_file("second.ts", "function second() {}").unwrap();
    ws.reindex().unwrap();

    // New state
    let snapshot2 = ws.snapshot();
    assert!(snapshot2.has_symbol("first", "function"));
    assert!(snapshot2.has_symbol("second", "function"));
}

#[test]
fn test_without_auto_index() {
    let ws = TestWorkspace::builder()
        .with_file("test.ts", "function noIndex() {}")
        .without_auto_index()
        .build()
        .unwrap();

    // Should have no symbols since auto-indexing was disabled
    let snapshot = ws.snapshot();
    assert!(snapshot.symbols.is_empty());

    // Manual index
    ws.reindex().unwrap();
    let snapshot2 = ws.snapshot();
    assert!(snapshot2.has_symbol("noIndex", "function"));
}
