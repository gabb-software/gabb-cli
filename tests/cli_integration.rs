use std::fs;
use std::process::Command;

use gabb_cli::{indexer, store::IndexStore};
use tempfile::tempdir;

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
