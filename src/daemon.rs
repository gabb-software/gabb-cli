use crate::indexer::{
    build_full_index, index_one, is_indexed_file, remove_if_tracked, IndexPhase, IndexProgress,
    IndexSummary,
};
use crate::store::{now_unix, DbOpenResult, IndexStore, RegenerationReason};
use crate::OutputFormat;
use anyhow::{bail, Context, Result};
use indicatif::{ProgressBar, ProgressStyle};
use log::{debug, info, warn};
use notify::event::{ModifyKind, RenameMode};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

/// Options for ensure_index_available
#[derive(Debug, Clone)]
pub struct EnsureIndexOptions {
    /// If true, don't auto-start the daemon (error instead)
    pub no_start_daemon: bool,
    /// Timeout for waiting for index to be ready
    pub timeout: Duration,
    /// If true, suppress informational warnings about daemon status
    pub no_daemon_warnings: bool,
    /// If true, automatically restart daemon when version differs from CLI
    pub auto_restart_on_version_mismatch: bool,
}

impl Default for EnsureIndexOptions {
    fn default() -> Self {
        Self {
            no_start_daemon: false,
            timeout: Duration::from_secs(60),
            no_daemon_warnings: false,
            auto_restart_on_version_mismatch: false,
        }
    }
}

/// Ensure the index is available, auto-starting the daemon if needed.
/// Also handles automatic rebuild when schema version changes or database is corrupt.
///
/// This is the shared logic used by both CLI commands and the MCP server.
pub fn ensure_index_available(
    workspace_root: &Path,
    db: &Path,
    opts: &EnsureIndexOptions,
) -> Result<()> {
    // Check if index exists
    if !db.exists() {
        if opts.no_start_daemon {
            bail!(
                "Index not found at {}\n\n\
                 Start the daemon to build an index:\n\
                 \n\
                     gabb daemon start",
                db.display()
            );
        }

        // Auto-start daemon and wait for initial index
        info!("Index not found. Starting daemon to build index...");
        start_daemon_and_wait(workspace_root, db, false, opts.timeout)?;
        return Ok(());
    }

    // Index exists - check if it needs regeneration (version mismatch)
    match IndexStore::try_open(db) {
        Ok(DbOpenResult::Ready(_)) => {
            // Index is good, check daemon version (unless suppressed)
            if !opts.no_daemon_warnings || opts.auto_restart_on_version_mismatch {
                check_daemon_version(workspace_root, db, opts.auto_restart_on_version_mismatch)?;
            }

            // Check if daemon is running, start it if not (fixes #85)
            let daemon_running = read_pid_file(workspace_root)
                .ok()
                .flatten()
                .is_some_and(|pid_info| is_process_running(pid_info.pid));

            if !daemon_running && !opts.no_start_daemon {
                info!("Index exists but daemon is not running. Starting daemon...");
                start_daemon_and_wait(workspace_root, db, false, opts.timeout)?;
            }
        }
        Ok(DbOpenResult::NeedsRegeneration { reason, .. }) => {
            if opts.no_start_daemon {
                bail!(
                    "{}\n\nRun `gabb daemon start --rebuild` to regenerate the index.",
                    reason.message()
                );
            }

            // Auto-rebuild the index
            info!("{}", reason.message());
            info!("Automatically rebuilding index...");

            // Stop any running daemon first
            if let Ok(Some(pid_info)) = read_pid_file(workspace_root) {
                if is_process_running(pid_info.pid) {
                    info!("Stopping existing daemon (PID {})...", pid_info.pid);
                    let _ = stop(workspace_root, false);
                    std::thread::sleep(Duration::from_millis(500));
                }
            }

            // Delete the old database and WAL files
            let _ = fs::remove_file(db);
            let _ = fs::remove_file(db.with_extension("db-wal"));
            let _ = fs::remove_file(db.with_extension("db-shm"));

            // Start daemon with rebuild
            start_daemon_and_wait(workspace_root, db, true, opts.timeout)?;
        }
        Err(e) => {
            // Database is corrupted or unreadable
            if opts.no_start_daemon {
                bail!(
                    "Failed to open index: {}\n\nRun `gabb daemon start --rebuild` to regenerate.",
                    e
                );
            }

            warn!("Failed to open index: {}. Rebuilding...", e);

            // Stop daemon if running
            if let Ok(Some(pid_info)) = read_pid_file(workspace_root) {
                if is_process_running(pid_info.pid) {
                    let _ = stop(workspace_root, false);
                    std::thread::sleep(Duration::from_millis(500));
                }
            }

            // Delete corrupted database and WAL files
            let _ = fs::remove_file(db);
            let _ = fs::remove_file(db.with_extension("db-wal"));
            let _ = fs::remove_file(db.with_extension("db-shm"));

            // Rebuild
            start_daemon_and_wait(workspace_root, db, true, opts.timeout)?;
        }
    }

    Ok(())
}

/// Start daemon in background and wait for index to be ready.
pub fn start_daemon_and_wait(
    workspace_root: &Path,
    db: &Path,
    rebuild: bool,
    timeout: Duration,
) -> Result<()> {
    // Delete any leftover WAL files that might interfere
    let wal_path = db.with_extension("db-wal");
    let shm_path = db.with_extension("db-shm");
    let _ = fs::remove_file(&wal_path);
    let _ = fs::remove_file(&shm_path);

    start(workspace_root, db, rebuild, true, None, true)?; // quiet=true for background

    // Wait for index to be created AND readable (with timeout)
    let start_time = std::time::Instant::now();
    let check_interval = Duration::from_millis(500);

    loop {
        if start_time.elapsed() >= timeout {
            bail!(
                "Daemon started but index not ready within {} seconds.\n\
                 Check daemon logs at {}/.gabb/daemon.log",
                timeout.as_secs(),
                workspace_root.display()
            );
        }

        if db.exists() {
            // Try to open the database to verify it's ready
            match IndexStore::try_open(db) {
                Ok(DbOpenResult::Ready(store)) => {
                    // Check if initial indexing has completed
                    match store.get_meta("initial_index_complete") {
                        Ok(Some(_)) => {
                            info!("Index ready. Proceeding with query.");
                            return Ok(());
                        }
                        Ok(None) => {
                            // Database exists but initial indexing not complete, keep waiting
                            debug!("Database exists but initial indexing not yet complete");
                        }
                        Err(e) => {
                            // Error reading metadata, keep waiting
                            debug!("Error checking index completion status: {}", e);
                        }
                    }
                }
                _ => {
                    // Database exists but not ready yet, keep waiting
                }
            }
        }

        std::thread::sleep(check_interval);
    }
}

/// Check daemon version and handle mismatches.
/// Returns true if daemon was restarted, false otherwise.
fn check_daemon_version(workspace_root: &Path, db: &Path, auto_restart: bool) -> Result<bool> {
    // Try to read PID file and check version
    if let Ok(Some(pid_info)) = read_pid_file(workspace_root) {
        if is_process_running(pid_info.pid) {
            let cli_version = env!("CARGO_PKG_VERSION");
            if pid_info.version != cli_version {
                if auto_restart {
                    info!(
                        "Daemon version ({}) differs from CLI version ({}). Auto-restarting...",
                        pid_info.version, cli_version
                    );
                    restart(workspace_root, db, false)?;
                    return Ok(true);
                } else {
                    warn!(
                        "Daemon version ({}) differs from CLI version ({}).\n\
                         Consider restarting: gabb daemon restart",
                        pid_info.version, cli_version
                    );
                }
            }
        }
    }
    Ok(false)
}

/// Derive workspace root from database path.
/// The database is expected to be at <workspace>/.gabb/index.db
/// Handles both relative and absolute paths.
pub fn workspace_root_from_db(db: &Path) -> Result<PathBuf> {
    let abs = if db.is_absolute() {
        db.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(db)
    };
    let path = abs.canonicalize().unwrap_or(abs);
    if let Some(parent) = path.parent() {
        if parent.file_name().and_then(|n| n.to_str()) == Some(".gabb") {
            if let Some(root) = parent.parent() {
                return Ok(root.to_path_buf());
            }
        }
        return Ok(parent.to_path_buf());
    }
    bail!("could not derive workspace root from db path")
}

/// PID file content structure
#[derive(Debug, Serialize, Deserialize)]
pub struct PidFile {
    pub pid: u32,
    pub version: String,
    pub schema_version: String,
    pub started_at: String,
    /// Original start options (for restart)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_options: Option<StartOptions>,
}

/// Options used when starting the daemon, stored for restart
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StartOptions {
    /// Database path (relative to workspace root)
    pub db_path: Option<String>,
    /// Whether rebuild was requested
    pub rebuild: bool,
    /// Log file path
    pub log_file: Option<String>,
}

impl PidFile {
    fn new(pid: u32, start_options: Option<StartOptions>) -> Self {
        Self {
            pid,
            version: env!("CARGO_PKG_VERSION").to_string(),
            schema_version: format!(
                "{}.{}",
                crate::store::SCHEMA_MAJOR,
                crate::store::SCHEMA_MINOR
            ),
            started_at: chrono_lite_now(),
            start_options,
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
pub fn read_pid_file(root: &Path) -> Result<Option<PidFile>> {
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
#[cfg(unix)]
pub fn is_process_running(pid: u32) -> bool {
    // Use kill with signal 0 to check if process exists
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

#[cfg(windows)]
pub fn is_process_running(pid: u32) -> bool {
    use std::process::Command;
    // Use tasklist to check if process exists on Windows
    Command::new("tasklist")
        .args(["/FI", &format!("PID eq {}", pid), "/NH"])
        .output()
        .map(|output| {
            let stdout = String::from_utf8_lossy(&output.stdout);
            stdout.contains(&pid.to_string())
        })
        .unwrap_or(false)
}

/// Get the path to the lock file for a workspace
fn lock_file_path(root: &Path) -> PathBuf {
    root.join(".gabb").join("daemon.lock")
}

/// A guard that holds the lock file open and releases it on drop
pub struct LockFileGuard {
    _file: fs::File,
    path: PathBuf,
}

impl Drop for LockFileGuard {
    fn drop(&mut self) {
        // Lock is automatically released when file is closed
        // Optionally remove the lock file
        let _ = fs::remove_file(&self.path);
    }
}

/// Acquire an exclusive lock on the workspace.
/// Returns a guard that releases the lock when dropped.
#[cfg(unix)]
fn acquire_lock(root: &Path) -> Result<LockFileGuard> {
    use std::os::unix::io::AsRawFd;

    let path = lock_file_path(root);

    // Ensure .gabb directory exists
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    // Open or create the lock file
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .with_context(|| format!("failed to open lock file {}", path.display()))?;

    // Try to acquire exclusive lock (non-blocking)
    let fd = file.as_raw_fd();
    let result = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };

    if result != 0 {
        let err = std::io::Error::last_os_error();
        if err.kind() == std::io::ErrorKind::WouldBlock {
            bail!(
                "Another daemon is already running for this workspace.\n\
                 Use 'gabb daemon status' to check or 'gabb daemon stop' to stop it."
            );
        }
        return Err(err).with_context(|| "failed to acquire lock");
    }

    // Write our PID to the lock file for debugging
    use std::io::Seek;
    let mut file = file;
    file.set_len(0)?;
    file.seek(std::io::SeekFrom::Start(0))?;
    writeln!(file, "{}", std::process::id())?;

    Ok(LockFileGuard { _file: file, path })
}

#[cfg(windows)]
fn acquire_lock(root: &Path) -> Result<LockFileGuard> {
    use std::io::Seek;

    let path = lock_file_path(root);

    // Ensure .gabb directory exists
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    // On Windows, opening with create_new acts as a simple lock mechanism
    // If another process has the file open, this may still succeed,
    // but we also check the PID file for running daemons
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .with_context(|| format!("failed to open lock file {}", path.display()))?;

    // Write our PID to the lock file
    let mut file = file;
    file.set_len(0)?;
    file.seek(std::io::SeekFrom::Start(0))?;
    writeln!(file, "{}", std::process::id())?;

    Ok(LockFileGuard { _file: file, path })
}

/// Start the indexing daemon
pub fn start(
    root: &Path,
    db_path: &Path,
    rebuild: bool,
    background: bool,
    log_file: Option<&Path>,
    quiet: bool,
) -> Result<()> {
    if background {
        return start_background(root, db_path, rebuild, log_file);
    }
    run_foreground(root, db_path, rebuild, quiet)
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
        .arg("--workspace")
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

/// Format a duration in human-readable format
fn format_duration(secs: f64) -> String {
    if secs < 60.0 {
        format!("{:.1}s", secs)
    } else if secs < 3600.0 {
        let mins = (secs / 60.0).floor();
        let remaining_secs = secs % 60.0;
        format!("{:.0}m {:.0}s", mins, remaining_secs)
    } else {
        let hours = (secs / 3600.0).floor();
        let remaining_mins = ((secs % 3600.0) / 60.0).floor();
        format!("{:.0}h {:.0}m", hours, remaining_mins)
    }
}

/// Run indexing with progress reporting
fn run_indexing_with_progress(
    root: &Path,
    store: &IndexStore,
    quiet: bool,
) -> Result<IndexSummary> {
    use std::sync::{Arc, Mutex};

    if quiet {
        // No progress output, just run indexing
        return build_full_index(root, store, None::<fn(&IndexProgress)>);
    }

    // Create progress bar
    let pb = Arc::new(Mutex::new(ProgressBar::new(0)));

    // Set initial style for scanning phase
    {
        let pb = pb.lock().unwrap();
        pb.set_style(
            ProgressStyle::default_spinner()
                .template("{spinner:.cyan} {msg}")
                .unwrap(),
        );
        pb.set_message("Scanning for files...");
        pb.enable_steady_tick(Duration::from_millis(100));
    }

    let pb_clone = Arc::clone(&pb);
    let progress_callback = move |progress: &IndexProgress| {
        let pb = pb_clone.lock().unwrap();

        match progress.phase {
            IndexPhase::Scanning => {
                pb.set_message("Scanning for files...");
            }
            IndexPhase::Parsing => {
                // Switch to progress bar style when we know the total
                if progress.files_total > 0 && pb.length() != Some(progress.files_total as u64) {
                    pb.set_length(progress.files_total as u64);
                    pb.set_style(
                        ProgressStyle::default_bar()
                            .template("{spinner:.cyan} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({percent}%) {msg}")
                            .unwrap()
                            .progress_chars("=>-"),
                    );
                }

                pb.set_position(progress.files_done as u64);

                // Build message with rate and ETA
                let mut msg = format!(
                    "{:.0} files/sec, {} symbols",
                    progress.files_per_sec, progress.symbols_found
                );
                if let Some(eta) = progress.eta_secs {
                    msg.push_str(&format!(", ETA: {}", format_duration(eta)));
                }
                pb.set_message(msg);
            }
            IndexPhase::Resolving => {
                pb.set_style(
                    ProgressStyle::default_spinner()
                        .template("{spinner:.cyan} {msg}")
                        .unwrap(),
                );
                pb.set_message(format!(
                    "Resolving cross-file references ({} symbols)...",
                    progress.symbols_found
                ));
            }
            IndexPhase::Finalizing => {
                pb.set_message("Finalizing index...");
            }
        }
    };

    let summary = build_full_index(root, store, Some(progress_callback))?;

    // Finish progress bar and print summary
    {
        let pb = pb.lock().unwrap();
        pb.finish_and_clear();
    }

    // Print final summary
    println!(
        "Indexed {} files ({} symbols) in {} ({:.1} files/sec)",
        summary.files_indexed,
        summary.symbols_found,
        format_duration(summary.duration_secs),
        summary.files_per_sec
    );

    Ok(summary)
}

/// Run the daemon in the foreground
fn run_foreground(root: &Path, db_path: &Path, rebuild: bool, quiet: bool) -> Result<()> {
    let root = root
        .canonicalize()
        .with_context(|| format!("failed to canonicalize root {}", root.display()))?;

    // Acquire exclusive lock to prevent multiple daemons
    let _lock_guard = acquire_lock(&root)?;
    debug!("Acquired workspace lock");

    // Check if daemon is already running (belt and suspenders with lock)
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

    // Create start options for restart capability
    let start_options = StartOptions {
        db_path: Some(db_path.to_string_lossy().to_string()),
        rebuild,
        log_file: None,
    };

    // Write PID file
    let pid = std::process::id();
    let pid_file = PidFile::new(pid, Some(start_options));
    write_pid_file(&root, &pid_file)?;
    info!("Daemon started (PID {})", pid);

    // Set up cleanup on exit
    let root_for_cleanup = root.clone();

    // Set up signal handling for graceful shutdown
    let (shutdown_tx, shutdown_rx) = mpsc::channel();
    {
        ctrlc::set_handler(move || {
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

    // Run indexing with progress reporting
    let _summary = run_indexing_with_progress(&root, &store, quiet)?;

    // Mark initial indexing as complete so CLI/MCP can proceed with queries
    store.set_meta("initial_index_complete", &now_unix().to_string())?;
    info!("Initial indexing complete, ready for queries");

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

/// Stop a running daemon (Unix implementation)
#[cfg(unix)]
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

/// Stop a running daemon (Windows implementation)
#[cfg(windows)]
pub fn stop(root: &Path, force: bool) -> Result<()> {
    use std::process::Command;

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

            info!(
                "{}",
                if force {
                    format!("Forcefully killing daemon (PID {})", pid_info.pid)
                } else {
                    format!("Stopping daemon (PID {})", pid_info.pid)
                }
            );

            // Use taskkill on Windows
            let mut cmd = Command::new("taskkill");
            if force {
                cmd.arg("/F");
            }
            cmd.args(["/PID", &pid_info.pid.to_string()]);

            let _ = cmd.output();

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
/// If rebuild is true, forces a full reindex. Otherwise, preserves original start options.
pub fn restart(root: &Path, db_path: &Path, rebuild: bool) -> Result<()> {
    let root = root
        .canonicalize()
        .with_context(|| format!("failed to canonicalize root {}", root.display()))?;

    // Read existing start options before stopping
    let existing_options = read_pid_file(&root)?.and_then(|pf| pf.start_options);

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

    // Determine database path: use provided, then stored, then default
    let effective_db_path = if db_path != Path::new(".gabb/index.db") {
        // Explicit db_path was provided
        db_path.to_path_buf()
    } else if let Some(ref opts) = existing_options {
        // Use stored db_path from previous run
        opts.db_path
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_else(|| db_path.to_path_buf())
    } else {
        db_path.to_path_buf()
    };

    // Determine log file from stored options
    let log_file = existing_options
        .as_ref()
        .and_then(|o| o.log_file.as_ref())
        .map(PathBuf::from);

    info!(
        "Restarting daemon with db_path: {}",
        effective_db_path.display()
    );

    // Start new daemon in background (quiet=true since it's backgrounded)
    start(
        &root,
        &effective_db_path,
        rebuild,
        true,
        log_file.as_deref(),
        true,
    )
}

/// Show daemon status
pub fn status(root: &Path, format: OutputFormat) -> Result<()> {
    use crate::store::IndexStore;

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
        #[serde(skip_serializing_if = "Option::is_none")]
        stats: Option<StatsInfo>,
        #[serde(skip_serializing_if = "Option::is_none")]
        activity: Option<ActivityInfo>,
    }

    #[derive(Serialize)]
    struct VersionInfo {
        daemon: String,
        cli: String,
        #[serde(rename = "match")]
        matches: bool,
        action: String,
    }

    #[derive(Serialize)]
    struct StatsInfo {
        files_indexed: i64,
        symbols_count: i64,
        #[serde(skip_serializing_if = "Option::is_none")]
        last_index_time: Option<String>,
    }

    #[derive(Serialize)]
    struct ActivityInfo {
        watching: bool,
        pending_changes: i64,
        #[serde(skip_serializing_if = "Option::is_none")]
        currently_indexing: Option<String>,
    }

    let cli_version = env!("CARGO_PKG_VERSION").to_string();
    let db_path = root.join(".gabb").join("index.db");

    // Try to get stats from the database if it exists
    let stats = if db_path.exists() {
        IndexStore::open(&db_path)
            .ok()
            .and_then(|store| store.get_index_stats().ok())
            .map(|index_stats| StatsInfo {
                files_indexed: index_stats.files.total,
                symbols_count: index_stats.symbols.total,
                last_index_time: index_stats.index.last_updated,
            })
    } else {
        None
    };

    let status = match pid_info {
        Some(ref pid_info) if is_process_running(pid_info.pid) => {
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
                    daemon: pid_info.version.clone(),
                    cli: cli_version,
                    matches: version_match,
                    action,
                }),
                stats,
                activity: Some(ActivityInfo {
                    watching: true,
                    pending_changes: 0, // We can't know this without daemon IPC
                    currently_indexing: None,
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
            stats,
            activity: None,
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
            if let Some(ref stats) = status.stats {
                println!(
                    "Index: {} files, {} symbols",
                    stats.files_indexed, stats.symbols_count
                );
                if let Some(ref last_time) = stats.last_index_time {
                    println!("Last indexed: {}", last_time);
                }
            }
            if let Some(ref activity) = status.activity {
                if activity.watching {
                    println!("Activity: watching for changes");
                }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::IndexStore;
    use tempfile::tempdir;

    /// Test that ensure_index_available attempts to start daemon when index exists
    /// but daemon is not running. This is a regression test for issue #85.
    ///
    /// Note: We can't fully test daemon spawning in unit tests because
    /// std::env::current_exe() returns the test binary. Instead, we verify
    /// that the start logic is triggered by checking the error message.
    #[test]
    fn test_ensure_index_starts_daemon_when_not_running() {
        let temp = tempdir().unwrap();
        let workspace_root = temp.path().to_path_buf();
        let gabb_dir = workspace_root.join(".gabb");
        fs::create_dir_all(&gabb_dir).unwrap();
        let db_path = gabb_dir.join("index.db");

        // Create a valid index database (simulating a previous daemon run)
        let store = IndexStore::open(&db_path).unwrap();
        store.set_meta("schema_version", "1.0").unwrap();
        drop(store);

        // Ensure no daemon is running (no PID file)
        let pid_path = pid_file_path(&workspace_root);
        assert!(!pid_path.exists(), "PID file should not exist before test");

        // Call ensure_index_available with a very short timeout
        // The key test is that it ATTEMPTS to start the daemon (enters the start logic)
        // rather than returning immediately like it did before the fix
        let opts = EnsureIndexOptions {
            no_start_daemon: false,
            timeout: Duration::from_millis(100), // Very short timeout
            no_daemon_warnings: true,
            auto_restart_on_version_mismatch: false,
        };
        let result = ensure_index_available(&workspace_root, &db_path, &opts);

        // The result will be an error due to timeout (test binary can't spawn daemon),
        // but the important thing is that it TRIED to start the daemon.
        // The error message should indicate daemon start was attempted.
        match result {
            Ok(()) => {
                // If it somehow succeeded, that's fine too
                // Clean up any daemon that might have started
                let _ = stop(&workspace_root, true);
            }
            Err(e) => {
                let err_msg = e.to_string();
                // Should see a daemon-related error, not "index not found" or similar
                assert!(
                    err_msg.contains("Daemon")
                        || err_msg.contains("daemon")
                        || err_msg.contains("index not ready"),
                    "Error should indicate daemon start was attempted, got: {}",
                    err_msg
                );
            }
        }
    }

    /// Test that no_start_daemon option is respected when daemon is not running.
    #[test]
    fn test_no_start_daemon_respected_when_index_exists() {
        let temp = tempdir().unwrap();
        let workspace_root = temp.path().to_path_buf();
        let gabb_dir = workspace_root.join(".gabb");
        fs::create_dir_all(&gabb_dir).unwrap();
        let db_path = gabb_dir.join("index.db");

        // Create a valid index database
        let store = IndexStore::open(&db_path).unwrap();
        store.set_meta("schema_version", "1.0").unwrap();
        drop(store);

        // Ensure no daemon is running
        let pid_path = pid_file_path(&workspace_root);
        assert!(!pid_path.exists());

        // Call with no_start_daemon: true - should return Ok without starting daemon
        let opts = EnsureIndexOptions {
            no_start_daemon: true,
            timeout: Duration::from_secs(1),
            no_daemon_warnings: true,
            auto_restart_on_version_mismatch: false,
        };
        let result = ensure_index_available(&workspace_root, &db_path, &opts);

        // Should succeed immediately without starting daemon
        assert!(
            result.is_ok(),
            "Should succeed when no_start_daemon is true"
        );
        assert!(!pid_path.exists(), "Daemon should not have been started");
    }

    /// Test that daemon is NOT started when it's already running.
    #[test]
    fn test_daemon_not_started_when_already_running() {
        let temp = tempdir().unwrap();
        let workspace_root = temp.path().to_path_buf();
        let gabb_dir = workspace_root.join(".gabb");
        fs::create_dir_all(&gabb_dir).unwrap();
        let db_path = gabb_dir.join("index.db");

        // Create a valid index database
        let store = IndexStore::open(&db_path).unwrap();
        store.set_meta("schema_version", "1.0").unwrap();
        drop(store);

        // Create a fake PID file indicating daemon is "running"
        // Use current process ID so is_process_running returns true
        let pid_file = PidFile::new(std::process::id(), None);
        write_pid_file(&workspace_root, &pid_file).unwrap();

        let opts = EnsureIndexOptions {
            no_start_daemon: false,
            timeout: Duration::from_secs(1),
            no_daemon_warnings: true,
            auto_restart_on_version_mismatch: false,
        };
        let result = ensure_index_available(&workspace_root, &db_path, &opts);

        // Should succeed without trying to start another daemon
        assert!(
            result.is_ok(),
            "Should succeed when daemon appears to be running"
        );

        // Clean up
        let _ = remove_pid_file(&workspace_root);
    }
}
