//! Smoke test to ensure the daemon can start, index a workspace, and notice file changes.
//! This uses a temp directory and the compiled binary; it does not assert timing,
//! only that the daemon can create the DB and exit cleanly when killed.

use std::fs;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use tempfile::tempdir;

#[test]
fn daemon_creates_db_and_handles_updates() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    let db_path = root.join(".gabb/index.db");
    let bin = env!("CARGO_BIN_EXE_gabb");

    // Seed a TypeScript file.
    let file_path = root.join("foo.ts");
    fs::write(&file_path, "function foo() {}\n").unwrap();

    // Start the daemon.
    let mut child = Command::new(bin)
        .args([
            "daemon",
            "start",
            "--root",
            root.to_str().unwrap(),
            "--db",
            db_path.to_str().unwrap(),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to start daemon");

    // Give it a moment to start and index.
    thread::sleep(Duration::from_secs(2));
    assert!(
        db_path.exists(),
        "daemon did not create db at {}",
        db_path.display()
    );

    // Modify the file to trigger a watcher event.
    fs::write(&file_path, "function foo() {}\nfunction bar() {}\n").unwrap();
    thread::sleep(Duration::from_secs(2));

    // Shut down the daemon.
    let _ = child.kill();
    let _ = child.wait();

    // Ensure the DB is still present after shutdown.
    assert!(db_path.exists(), "db missing after daemon shutdown");
}
