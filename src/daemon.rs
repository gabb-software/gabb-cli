use crate::indexer::{build_full_index, index_one, is_indexed_file, remove_if_tracked};
use crate::store::{DbOpenResult, IndexStore, RegenerationReason};
use crate::OutputFormat;
use anyhow::{bail, Context, Result};
use log::{debug, info, warn};
use notify::event::{ModifyKind, RenameMode};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

/// PID file content structure
#[derive(Debug, Serialize, Deserialize)]
pub struct PidFile {
    pub pid: u32,
    pub version: String,
    pub schema_version: String,
    pub started_at: String,
}

impl PidFile {
    fn new(pid: u32) -> Self {
        Self {
            pid,
            version: env!("CARGO_PKG_VERSION").to_string(),
            schema_version: format!(
                "{}.{}",
                crate::store::SCHEMA_MAJOR,
                crate::store::SCHEMA_MINOR
            ),
            started_at: chrono_lite_now(),
        }
    }
}

/// Simple ISO 8601 timestamp without chrono dependency
fn chrono_lite_now() -> String {
    use std::time::SystemTime;
    let duration = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    // Basic ISO format: we'll just use seconds since epoch for simplicity
    format!("{}", duration.as_secs())
}

/// Get the path to the PID file for a workspace
fn pid_file_path(root: &Path) -> PathBuf {
    root.join(".gabb").join("daemon.pid")
}

/// Read the PID file for a workspace
fn read_pid_file(root: &Path) -> Result<Option<PidFile>> {
    let path = pid_file_path(root);
    if !path.exists() {
        return Ok(None);
    }
    let mut file = fs::File::open(&path)?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;
    let pid_file: PidFile = serde_json::from_str(&contents)?;
    Ok(Some(pid_file))
}

/// Write the PID file for a workspace
fn write_pid_file(root: &Path, pid_file: &PidFile) -> Result<()> {
    let path = pid_file_path(root);
    // Ensure .gabb directory exists
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = fs::File::create(&path)?;
    let contents = serde_json::to_string_pretty(pid_file)?;
    file.write_all(contents.as_bytes())?;
    Ok(())
}

/// Remove the PID file for a workspace
fn remove_pid_file(root: &Path) -> Result<()> {
    let path = pid_file_path(root);
    if path.exists() {
        fs::remove_file(&path)?;
    }
    Ok(())
}

/// Check if a process with the given PID is running
fn is_process_running(pid: u32) -> bool {
    // Use kill with signal 0 to check if process exists
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

/// Start the indexing daemon
pub fn start(
    root: &Path,
    db_path: &Path,
    rebuild: bool,
    background: bool,
    log_file: Option<&Path>,
) -> Result<()> {
    if background {
        return start_background(root, db_path, rebuild, log_file);
    }
    run_foreground(root, db_path, rebuild)
}

/// Start daemon in background (daemonize)
fn start_background(
    root: &Path,
    db_path: &Path,
    rebuild: bool,
    log_file: Option<&Path>,
) -> Result<()> {
    let root = root
        .canonicalize()
        .with_context(|| format!("failed to canonicalize root {}", root.display()))?;

    // Check if daemon is already running
    if let Some(pid_info) = read_pid_file(&root)? {
        if is_process_running(pid_info.pid) {
            bail!(
                "Daemon already running (PID {}). Use 'gabb daemon stop' first.",
                pid_info.pid
            );
        }
        // Stale PID file - remove it
        remove_pid_file(&root)?;
    }

    // Determine log file path
    let log_path = log_file
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| root.join(".gabb").join("daemon.log"));

    // Ensure .gabb directory exists
    fs::create_dir_all(root.join(".gabb"))?;

    // Fork the process
    use std::process::Command;
    let db_arg = if db_path.is_absolute() {
        db_path.to_path_buf()
    } else {
        root.join(db_path)
    };

    let exe = std::env::current_exe()?;
    let mut cmd = Command::new(exe);
    cmd.arg("daemon")
        .arg("start")
        .arg("--root")
        .arg(&root)
        .arg("--db")
        .arg(&db_arg);

    if rebuild {
        cmd.arg("--rebuild");
    }

    // Redirect stdout/stderr to log file
    let log_file_handle = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("failed to open log file {}", log_path.display()))?;

    cmd.stdout(log_file_handle.try_clone()?);
    cmd.stderr(log_file_handle);

    // Detach from terminal
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    let child = cmd.spawn().context("failed to spawn daemon process")?;

    // Give the daemon a moment to start
    std::thread::sleep(Duration::from_millis(100));

    info!(
        "Daemon started in background (PID {}). Log: {}",
        child.id(),
        log_path.display()
    );

    Ok(())
}

/// Run the daemon in the foreground
fn run_foreground(root: &Path, db_path: &Path, rebuild: bool) -> Result<()> {
    let root = root
        .canonicalize()
        .with_context(|| format!("failed to canonicalize root {}", root.display()))?;

    // Check if daemon is already running
    if let Some(pid_info) = read_pid_file(&root)? {
        if is_process_running(pid_info.pid) {
            bail!(
                "Daemon already running (PID {}). Use 'gabb daemon stop' first.",
                pid_info.pid
            );
        }
        // Stale PID file - remove it
        remove_pid_file(&root)?;
    }

    // Write PID file
    let pid = std::process::id();
    let pid_file = PidFile::new(pid);
    write_pid_file(&root, &pid_file)?;
    info!("Daemon started (PID {})", pid);

    // Set up cleanup on exit
    let root_for_cleanup = root.clone();

    // Set up signal handling for graceful shutdown
    let (shutdown_tx, shutdown_rx) = mpsc::channel();
    #[cfg(unix)]
    {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        let running = Arc::new(AtomicBool::new(true));
        let r = running.clone();

        ctrlc::set_handler(move || {
            r.store(false, Ordering::SeqCst);
            let _ = shutdown_tx.send(());
        })
        .ok();
    }

    info!("Opening index at {}", db_path.display());

    // Handle explicit rebuild request
    if rebuild && db_path.exists() {
        info!("{}", RegenerationReason::UserRequested.message());
        info!("Regenerating index...");
        let _ = fs::remove_file(db_path);
    }

    // Try to open with version checking
    let store = if rebuild {
        // After explicit rebuild, just open fresh
        IndexStore::open(db_path)?
    } else {
        match IndexStore::try_open(db_path)? {
            DbOpenResult::Ready(store) => store,
            DbOpenResult::NeedsRegeneration { reason, path } => {
                warn!("{}", reason.message());
                info!("Regenerating index (this may take a minute for large codebases)...");
                if path.exists() {
                    let _ = fs::remove_file(&path);
                }
                IndexStore::open(db_path)?
            }
        }
    };

    build_full_index(&root, &store)?;

    let (tx, rx) = mpsc::channel();
    let mut watcher: RecommendedWatcher = notify::recommended_watcher(move |res| {
        if tx.send(res).is_err() {
            eprintln!("watcher channel closed");
        }
    })?;
    watcher.watch(&root, RecursiveMode::Recursive)?;

    info!("Watching {} for changes", root.display());
    loop {
        // Check for shutdown signal
        if shutdown_rx.try_recv().is_ok() {
            info!("Received shutdown signal");
            break;
        }

        match rx.recv_timeout(Duration::from_secs(1)) {
            Ok(Ok(event)) => {
                if let Err(err) = handle_event(&root, &store, event) {
                    warn!("failed to handle event: {err:#}");
                }
            }
            Ok(Err(err)) => warn!("watch error: {err}"),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // continue loop to keep watcher alive
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    // Clean up PID file on exit
    remove_pid_file(&root_for_cleanup)?;
    info!("Daemon stopped");

    Ok(())
}

/// Stop a running daemon
pub fn stop(root: &Path, force: bool) -> Result<()> {
    let root = root
        .canonicalize()
        .with_context(|| format!("failed to canonicalize root {}", root.display()))?;

    let pid_info = read_pid_file(&root)?;
    match pid_info {
        None => {
            info!("No daemon running (no PID file found)");
            std::process::exit(1);
        }
        Some(pid_info) => {
            if !is_process_running(pid_info.pid) {
                info!("Daemon not running (stale PID file). Cleaning up.");
                remove_pid_file(&root)?;
                std::process::exit(1);
            }

            let signal = if force {
                info!("Forcefully killing daemon (PID {})", pid_info.pid);
                libc::SIGKILL
            } else {
                info!("Sending shutdown signal to daemon (PID {})", pid_info.pid);
                libc::SIGTERM
            };

            unsafe {
                libc::kill(pid_info.pid as i32, signal);
            }

            // Wait for process to exit (with timeout)
            let max_wait = if force {
                Duration::from_secs(2)
            } else {
                Duration::from_secs(10)
            };
            let start = std::time::Instant::now();

            while is_process_running(pid_info.pid) && start.elapsed() < max_wait {
                std::thread::sleep(Duration::from_millis(100));
            }

            if is_process_running(pid_info.pid) {
                if !force {
                    warn!("Daemon did not stop gracefully. Use --force to kill immediately.");
                    std::process::exit(1);
                }
            } else {
                info!("Daemon stopped");
                // Clean up PID file if still present
                remove_pid_file(&root)?;
            }
        }
    }

    Ok(())
}

/// Restart the daemon
pub fn restart(root: &Path, db_path: &Path, rebuild: bool) -> Result<()> {
    let root = root
        .canonicalize()
        .with_context(|| format!("failed to canonicalize root {}", root.display()))?;

    // Try to stop existing daemon
    if let Some(pid_info) = read_pid_file(&root)? {
        if is_process_running(pid_info.pid) {
            info!("Stopping existing daemon (PID {})", pid_info.pid);
            stop(&root, false).ok();

            // Wait a bit for clean shutdown
            std::thread::sleep(Duration::from_millis(500));
        } else {
            // Stale PID file
            remove_pid_file(&root)?;
        }
    }

    // Start new daemon in background
    start(&root, db_path, rebuild, true, None)
}

/// Show daemon status
pub fn status(root: &Path, format: OutputFormat) -> Result<()> {
    let root = root
        .canonicalize()
        .with_context(|| format!("failed to canonicalize root {}", root.display()))?;

    let pid_info = read_pid_file(&root)?;

    #[derive(Serialize)]
    struct StatusOutput {
        running: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        pid: Option<u32>,
        workspace: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        database: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        version: Option<VersionInfo>,
    }

    #[derive(Serialize)]
    struct VersionInfo {
        daemon: String,
        cli: String,
        #[serde(rename = "match")]
        matches: bool,
        action: String,
    }

    let cli_version = env!("CARGO_PKG_VERSION").to_string();
    let db_path = root.join(".gabb").join("index.db");

    let status = match pid_info {
        Some(pid_info) if is_process_running(pid_info.pid) => {
            let version_match = pid_info.version == cli_version;
            let action = if version_match {
                "none"
            } else {
                "suggest_restart"
            }
            .to_string();

            StatusOutput {
                running: true,
                pid: Some(pid_info.pid),
                workspace: root.to_string_lossy().to_string(),
                database: if db_path.exists() {
                    Some(db_path.to_string_lossy().to_string())
                } else {
                    None
                },
                version: Some(VersionInfo {
                    daemon: pid_info.version,
                    cli: cli_version,
                    matches: version_match,
                    action,
                }),
            }
        }
        _ => StatusOutput {
            running: false,
            pid: None,
            workspace: root.to_string_lossy().to_string(),
            database: if db_path.exists() {
                Some(db_path.to_string_lossy().to_string())
            } else {
                None
            },
            version: None,
        },
    };

    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&status)?);
        }
        OutputFormat::Jsonl => {
            println!("{}", serde_json::to_string(&status)?);
        }
        OutputFormat::Text | OutputFormat::Csv | OutputFormat::Tsv => {
            if status.running {
                println!("Daemon: running (PID {})", status.pid.unwrap_or(0));
                if let Some(ref ver) = status.version {
                    println!("Version: {} (CLI: {})", ver.daemon, ver.cli);
                    if !ver.matches {
                        println!("Warning: version mismatch - consider restarting daemon");
                    }
                }
            } else {
                println!("Daemon: not running");
            }
            println!("Workspace: {}", status.workspace);
            if let Some(ref db) = status.database {
                println!("Database: {}", db);
            } else {
                println!("Database: not found (index not created)");
            }
        }
    }

    // Exit with code 1 if not running (for scripting)
    if !status.running {
        std::process::exit(1);
    }

    Ok(())
}

fn handle_event(root: &Path, store: &IndexStore, event: Event) -> Result<()> {
    let paths: Vec<PathBuf> = event
        .paths
        .into_iter()
        .filter_map(|p| normalize_event_path(root, p))
        .collect();

    match event.kind {
        EventKind::Modify(ModifyKind::Name(RenameMode::From)) | EventKind::Remove(_) => {
            for path in paths {
                remove_if_tracked(&path, store)?;
            }
        }
        EventKind::Modify(ModifyKind::Name(RenameMode::To))
        | EventKind::Create(_)
        | EventKind::Modify(_) => {
            for path in paths {
                if is_indexed_file(&path) && path.is_file() {
                    index_one(&path, store)?;
                }
            }
        }
        _ => debug!("ignoring event {:?}", event.kind),
    }
    Ok(())
}

fn normalize_event_path(root: &Path, path: PathBuf) -> Option<PathBuf> {
    if path.is_absolute() {
        Some(path)
    } else {
        Some(root.join(path))
    }
}
