use anyhow::{anyhow, bail, Context, Result};
use clap::{Parser, Subcommand};
use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use gabb_cli::daemon;
use gabb_cli::mcp;
use gabb_cli::store;
use gabb_cli::store::{normalize_path, DbOpenResult, IndexStore, SymbolRecord};
use gabb_cli::OutputFormat;

/// Open index store for query commands with version checking.
/// Returns a helpful error if the database needs regeneration.
fn open_store_for_query(db: &Path) -> Result<IndexStore> {
    match IndexStore::try_open(db)? {
        DbOpenResult::Ready(store) => Ok(store),
        DbOpenResult::NeedsRegeneration { reason, .. } => {
            bail!(
                "{}\n\nRun `gabb daemon start --db {} --rebuild` to regenerate the index.",
                reason.message(),
                db.display()
            )
        }
    }
}

/// Ensure the index is available, auto-starting the daemon if needed.
/// Also handles automatic rebuild when schema version changes.
fn ensure_index_available(db: &Path, opts: &DaemonOptions) -> Result<()> {
    // Derive workspace root from db path
    let workspace_root = workspace_root_from_db(db)
        .unwrap_or_else(|_| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

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
        log::info!("Index not found. Starting daemon to build index...");
        start_daemon_and_wait(&workspace_root, db, false)?;
        return Ok(());
    }

    // Index exists - check if it needs regeneration (version mismatch)
    match IndexStore::try_open(db) {
        Ok(DbOpenResult::Ready(_)) => {
            // Index is good, check daemon version (unless suppressed)
            if !opts.no_daemon {
                check_daemon_version(&workspace_root);
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
            log::info!("{}", reason.message());
            log::info!("Automatically rebuilding index...");

            // Stop any running daemon first
            if let Ok(Some(pid_info)) = daemon::read_pid_file(&workspace_root) {
                if daemon::is_process_running(pid_info.pid) {
                    log::info!("Stopping existing daemon (PID {})...", pid_info.pid);
                    let _ = daemon::stop(&workspace_root, false);
                    std::thread::sleep(std::time::Duration::from_millis(500));
                }
            }

            // Delete the old database and WAL files
            let _ = fs::remove_file(db);
            let _ = fs::remove_file(db.with_extension("db-wal"));
            let _ = fs::remove_file(db.with_extension("db-shm"));

            // Start daemon with rebuild
            start_daemon_and_wait(&workspace_root, db, true)?;
        }
        Err(e) => {
            // Database is corrupted or unreadable
            if opts.no_start_daemon {
                bail!(
                    "Failed to open index: {}\n\nRun `gabb daemon start --rebuild` to regenerate.",
                    e
                );
            }

            log::warn!("Failed to open index: {}. Rebuilding...", e);

            // Stop daemon if running
            if let Ok(Some(pid_info)) = daemon::read_pid_file(&workspace_root) {
                if daemon::is_process_running(pid_info.pid) {
                    let _ = daemon::stop(&workspace_root, false);
                    std::thread::sleep(std::time::Duration::from_millis(500));
                }
            }

            // Delete corrupted database and WAL files
            let _ = fs::remove_file(db);
            let _ = fs::remove_file(db.with_extension("db-wal"));
            let _ = fs::remove_file(db.with_extension("db-shm"));

            // Rebuild
            start_daemon_and_wait(&workspace_root, db, true)?;
        }
    }

    Ok(())
}

/// Start daemon in background and wait for index to be ready.
fn start_daemon_and_wait(workspace_root: &Path, db: &Path, rebuild: bool) -> Result<()> {
    // Delete any leftover WAL files that might interfere
    let wal_path = db.with_extension("db-wal");
    let shm_path = db.with_extension("db-shm");
    let _ = fs::remove_file(&wal_path);
    let _ = fs::remove_file(&shm_path);

    daemon::start(workspace_root, db, rebuild, true, None)?;

    // Wait for index to be created AND readable (with timeout)
    let max_wait = std::time::Duration::from_secs(60);
    let start_time = std::time::Instant::now();
    let check_interval = std::time::Duration::from_millis(500);

    loop {
        if start_time.elapsed() >= max_wait {
            bail!(
                "Daemon started but index not ready within 60 seconds.\n\
                 Check daemon logs at {}/.gabb/daemon.log",
                workspace_root.display()
            );
        }

        if db.exists() {
            // Try to open the database to verify it's ready
            match IndexStore::try_open(db) {
                Ok(DbOpenResult::Ready(_)) => {
                    log::info!("Index ready. Proceeding with query.");
                    return Ok(());
                }
                _ => {
                    // Database exists but not ready yet, keep waiting
                }
            }
        }

        std::thread::sleep(check_interval);
    }
}

/// Check daemon version and warn about mismatches.
fn check_daemon_version(workspace_root: &Path) {
    // Try to read PID file and check version
    if let Ok(Some(pid_info)) = daemon::read_pid_file(workspace_root) {
        if daemon::is_process_running(pid_info.pid) {
            let cli_version = env!("CARGO_PKG_VERSION");
            if pid_info.version != cli_version {
                log::warn!(
                    "Daemon version ({}) differs from CLI version ({}).\n\
                     Consider restarting: gabb daemon restart",
                    pid_info.version,
                    cli_version
                );
            }
        }
    }
}

// ==================== Output Formatting ====================

/// A symbol with resolved line/column positions for output
#[derive(serde::Serialize)]
struct SymbolOutput {
    id: String,
    name: String,
    kind: String,
    file: String,
    start: Position,
    end: Position,
    #[serde(skip_serializing_if = "Option::is_none")]
    visibility: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    container: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    qualifier: Option<String>,
}

#[derive(serde::Serialize)]
struct Position {
    line: usize,
    character: usize,
}

impl SymbolOutput {
    fn from_record(sym: &SymbolRecord) -> Option<Self> {
        let (line, col) = offset_to_line_char_in_file(&sym.file, sym.start).ok()?;
        let (end_line, end_col) = offset_to_line_char_in_file(&sym.file, sym.end).ok()?;
        Some(Self {
            id: sym.id.clone(),
            name: sym.name.clone(),
            kind: sym.kind.clone(),
            file: sym.file.clone(),
            start: Position {
                line,
                character: col,
            },
            end: Position {
                line: end_line,
                character: end_col,
            },
            visibility: sym.visibility.clone(),
            container: sym.container.clone(),
            qualifier: sym.qualifier.clone(),
        })
    }

    /// Compact file:line:col format for text output
    fn location(&self) -> String {
        format!("{}:{}:{}", self.file, self.start.line, self.start.character)
    }

    /// CSV/TSV row
    fn to_row(&self) -> Vec<String> {
        vec![
            self.name.clone(),
            self.kind.clone(),
            self.location(),
            self.visibility.clone().unwrap_or_default(),
            self.container.clone().unwrap_or_default(),
        ]
    }
}

/// Format and output a list of symbols
fn output_symbols(symbols: &[SymbolRecord], format: OutputFormat) -> Result<()> {
    let outputs: Vec<SymbolOutput> = symbols
        .iter()
        .filter_map(SymbolOutput::from_record)
        .collect();

    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&outputs)?);
        }
        OutputFormat::Jsonl => {
            for sym in &outputs {
                println!("{}", serde_json::to_string(sym)?);
            }
        }
        OutputFormat::Csv => {
            let mut wtr = csv::Writer::from_writer(std::io::stdout());
            wtr.write_record(["name", "kind", "location", "visibility", "container"])?;
            for sym in &outputs {
                wtr.write_record(sym.to_row())?;
            }
            wtr.flush()?;
        }
        OutputFormat::Tsv => {
            println!("name\tkind\tlocation\tvisibility\tcontainer");
            for sym in &outputs {
                let row = sym.to_row();
                println!("{}", row.join("\t"));
            }
        }
        OutputFormat::Text => {
            for sym in &outputs {
                let container = sym
                    .container
                    .as_deref()
                    .map(|c| format!(" in {c}"))
                    .unwrap_or_default();
                println!(
                    "{:<10} {:<30} {}{}",
                    sym.kind,
                    sym.name,
                    sym.location(),
                    container
                );
            }
        }
    }
    Ok(())
}

/// Output for implementation/usages results with a target symbol
#[derive(serde::Serialize)]
struct TargetedResultOutput {
    target: SymbolOutput,
    results: Vec<SymbolOutput>,
}

/// Format and output implementations for a target symbol
fn output_implementations(
    target: &SymbolRecord,
    implementations: &[SymbolRecord],
    format: OutputFormat,
) -> Result<()> {
    let target_out = SymbolOutput::from_record(target)
        .ok_or_else(|| anyhow!("Failed to resolve target position"))?;
    let impl_outputs: Vec<SymbolOutput> = implementations
        .iter()
        .filter_map(SymbolOutput::from_record)
        .collect();

    match format {
        OutputFormat::Json => {
            let output = TargetedResultOutput {
                target: target_out,
                results: impl_outputs,
            };
            println!("{}", serde_json::to_string_pretty(&output)?);
        }
        OutputFormat::Jsonl => {
            // First line is target, rest are implementations
            println!("{}", serde_json::to_string(&target_out)?);
            for sym in &impl_outputs {
                println!("{}", serde_json::to_string(sym)?);
            }
        }
        OutputFormat::Csv => {
            let mut wtr = csv::Writer::from_writer(std::io::stdout());
            wtr.write_record(["name", "kind", "location", "visibility", "container"])?;
            for sym in &impl_outputs {
                wtr.write_record(sym.to_row())?;
            }
            wtr.flush()?;
        }
        OutputFormat::Tsv => {
            println!("name\tkind\tlocation\tvisibility\tcontainer");
            for sym in &impl_outputs {
                println!("{}", sym.to_row().join("\t"));
            }
        }
        OutputFormat::Text => {
            println!(
                "Target: {} {} {}",
                target_out.kind,
                target_out.name,
                target_out.location()
            );
            for sym in &impl_outputs {
                let container = sym
                    .container
                    .as_deref()
                    .map(|c| format!(" in {c}"))
                    .unwrap_or_default();
                println!(
                    "{:<10} {:<30} {}{}",
                    sym.kind,
                    sym.name,
                    sym.location(),
                    container
                );
            }
        }
    }
    Ok(())
}

#[derive(Parser, Debug)]
#[command(name = "gabb", version, about = "Gabb CLI indexing daemon")]
struct Cli {
    /// Increase output verbosity (-v, -vv)
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    verbose: u8,

    /// Output format (text, json, jsonl, csv, tsv)
    #[arg(long, short = 'f', global = true, value_enum, default_value = "text")]
    format: OutputFormat,

    /// Don't auto-start daemon if index doesn't exist
    #[arg(long, global = true)]
    no_start_daemon: bool,

    /// Suppress daemon-related warnings and status checks
    #[arg(long, global = true)]
    no_daemon: bool,

    #[command(subcommand)]
    command: Commands,
}

/// Options for daemon auto-start behavior
struct DaemonOptions {
    /// If true, don't auto-start daemon (default: false, meaning auto-start is enabled)
    no_start_daemon: bool,
    /// If true, suppress daemon warnings
    no_daemon: bool,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Manage the indexing daemon
    Daemon {
        #[command(subcommand)]
        command: DaemonCommands,
    },
    /// List symbols from an existing index
    Symbols {
        /// Path to the SQLite index database
        #[arg(long, default_value = ".gabb/index.db")]
        db: PathBuf,
        /// Only show symbols from this file
        #[arg(long)]
        file: Option<PathBuf>,
        /// Only show symbols of this kind (function, class, interface, method, struct, enum, trait)
        #[arg(long)]
        kind: Option<String>,
        /// Only show symbols with this exact name
        #[arg(long)]
        name: Option<String>,
        /// Limit the number of results
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Find implementations for symbol at a source position
    Implementation {
        /// Path to the SQLite index database
        #[arg(long, default_value = ".gabb/index.db")]
        db: PathBuf,
        /// Source file containing the reference. You can optionally append :line:character (1-based), e.g. ./src/daemon.rs:18:5
        #[arg(long)]
        file: PathBuf,
        /// 1-based line number within the file (optional if provided in --file)
        #[arg(long)]
        line: Option<usize>,
        /// 1-based character offset within the line (optional if provided in --file)
        #[arg(long, alias = "col")]
        character: Option<usize>,
        /// Limit the number of results
        #[arg(long)]
        limit: Option<usize>,
        /// Only show symbols of this kind (function, class, interface, method, struct, enum, trait)
        #[arg(long)]
        kind: Option<String>,
    },
    /// Find usages of the symbol at a source position
    Usages {
        /// Path to the SQLite index database
        #[arg(long, default_value = ".gabb/index.db")]
        db: PathBuf,
        /// Source file containing the reference. You can optionally append :line:character (1-based), e.g. ./src/daemon.rs:18:5
        #[arg(long)]
        file: PathBuf,
        /// 1-based line number within the file (optional if provided in --file)
        #[arg(long)]
        line: Option<usize>,
        /// 1-based character offset within the line (optional if provided in --file)
        #[arg(long, alias = "col")]
        character: Option<usize>,
        /// Limit the number of results
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Show details for symbols with a given name
    Symbol {
        /// Path to the SQLite index database
        #[arg(long, default_value = ".gabb/index.db")]
        db: PathBuf,
        /// Symbol name to look up
        #[arg(long)]
        name: String,
        /// Only show symbols from this file
        #[arg(long)]
        file: Option<PathBuf>,
        /// Only show symbols of this kind (function, class, interface, method, struct, enum, trait)
        #[arg(long)]
        kind: Option<String>,
        /// Limit the number of results
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Go to definition: find where a symbol is declared
    Definition {
        /// Path to the SQLite index database
        #[arg(long, default_value = ".gabb/index.db")]
        db: PathBuf,
        /// Source file containing the reference. You can optionally append :line:character (1-based), e.g. ./src/daemon.rs:18:5
        #[arg(long)]
        file: PathBuf,
        /// 1-based line number within the file (optional if provided in --file)
        #[arg(long)]
        line: Option<usize>,
        /// 1-based character offset within the line (optional if provided in --file)
        #[arg(long, alias = "col")]
        character: Option<usize>,
    },
    /// Find duplicate code in the codebase
    Duplicates {
        /// Path to the SQLite index database
        #[arg(long, default_value = ".gabb/index.db")]
        db: PathBuf,
        /// Only analyze files with uncommitted changes (git working tree)
        #[arg(long)]
        uncommitted: bool,
        /// Only analyze files in git staging area
        #[arg(long)]
        staged: bool,
        /// Only check specific symbol kinds (function, method, class, etc.)
        #[arg(long)]
        kind: Option<String>,
        /// Minimum number of duplicates to report (default: 2)
        #[arg(long, default_value = "2")]
        min_count: usize,
    },
    /// Start MCP (Model Context Protocol) server for AI assistant integration
    McpServer {
        /// Workspace root to index (auto-detected if not specified)
        #[arg(long, default_value = ".")]
        root: PathBuf,
        /// Path to the SQLite index database
        #[arg(long, default_value = ".gabb/index.db")]
        db: PathBuf,
    },
    /// Manage MCP (Model Context Protocol) configuration for AI assistants
    Mcp {
        #[command(subcommand)]
        command: McpCommands,
    },
}

#[derive(Subcommand, Debug)]
enum McpCommands {
    /// Print MCP configuration JSON for manual setup
    Config {
        /// Workspace root (used in config output)
        #[arg(long, default_value = ".")]
        root: PathBuf,
    },
    /// Install gabb MCP server into Claude Desktop/Code configuration
    Install {
        /// Workspace root to configure
        #[arg(long, default_value = ".")]
        root: PathBuf,
        /// Only install for Claude Desktop
        #[arg(long)]
        claude_desktop: bool,
        /// Only install for Claude Code (project-level .claude/mcp.json)
        #[arg(long)]
        claude_code: bool,
    },
    /// Check MCP configuration status
    Status,
    /// Remove gabb from MCP configuration
    Uninstall {
        /// Only uninstall from Claude Desktop
        #[arg(long)]
        claude_desktop: bool,
        /// Only uninstall from Claude Code
        #[arg(long)]
        claude_code: bool,
    },
}

#[derive(Subcommand, Debug)]
enum DaemonCommands {
    /// Start the indexing daemon
    Start {
        /// Workspace root to index (auto-detected if not specified)
        #[arg(long, default_value = ".")]
        root: PathBuf,
        /// Path to the SQLite index database
        #[arg(long, default_value = ".gabb/index.db")]
        db: PathBuf,
        /// Delete and recreate index on start
        #[arg(long)]
        rebuild: bool,
        /// Run in background (daemonize)
        #[arg(long, short = 'b')]
        background: bool,
        /// Log file path (default: .gabb/daemon.log when backgrounded)
        #[arg(long)]
        log_file: Option<PathBuf>,
    },
    /// Stop a running daemon
    Stop {
        /// Workspace root (to locate PID file)
        #[arg(long, default_value = ".")]
        root: PathBuf,
        /// Force immediate shutdown (SIGKILL)
        #[arg(long)]
        force: bool,
    },
    /// Restart the daemon
    Restart {
        /// Workspace root
        #[arg(long, default_value = ".")]
        root: PathBuf,
        /// Path to the SQLite index database
        #[arg(long, default_value = ".gabb/index.db")]
        db: PathBuf,
        /// Force full reindex on restart
        #[arg(long)]
        rebuild: bool,
    },
    /// Show daemon status
    Status {
        /// Workspace root (to locate PID file)
        #[arg(long, default_value = ".")]
        root: PathBuf,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    init_logging(cli.verbose);
    let format = cli.format;
    let daemon_opts = DaemonOptions {
        no_start_daemon: cli.no_start_daemon,
        no_daemon: cli.no_daemon,
    };

    match cli.command {
        Commands::Daemon { command } => match command {
            DaemonCommands::Start {
                root,
                db,
                rebuild,
                background,
                log_file,
            } => daemon::start(&root, &db, rebuild, background, log_file.as_deref()),
            DaemonCommands::Stop { root, force } => daemon::stop(&root, force),
            DaemonCommands::Restart { root, db, rebuild } => daemon::restart(&root, &db, rebuild),
            DaemonCommands::Status { root } => daemon::status(&root, format),
        },
        Commands::Symbols {
            db,
            file,
            kind,
            name,
            limit,
        } => {
            ensure_index_available(&db, &daemon_opts)?;
            list_symbols(
                &db,
                file.as_ref(),
                kind.as_deref(),
                name.as_deref(),
                limit,
                format,
            )
        }
        Commands::Implementation {
            db,
            file,
            line,
            character,
            limit,
            kind,
        } => {
            ensure_index_available(&db, &daemon_opts)?;
            find_implementation(&db, &file, line, character, limit, kind.as_deref(), format)
        }
        Commands::Usages {
            db,
            file,
            line,
            character,
            limit,
        } => {
            ensure_index_available(&db, &daemon_opts)?;
            find_usages(&db, &file, line, character, limit, format)
        }
        Commands::Symbol {
            db,
            name,
            file,
            kind,
            limit,
        } => {
            ensure_index_available(&db, &daemon_opts)?;
            show_symbol(&db, &name, file.as_ref(), kind.as_deref(), limit, format)
        }
        Commands::Definition {
            db,
            file,
            line,
            character,
        } => {
            ensure_index_available(&db, &daemon_opts)?;
            find_definition(&db, &file, line, character, format)
        }
        Commands::Duplicates {
            db,
            uncommitted,
            staged,
            kind,
            min_count,
        } => {
            ensure_index_available(&db, &daemon_opts)?;
            find_duplicates(&db, uncommitted, staged, kind.as_deref(), min_count, format)
        }
        Commands::McpServer { root, db } => {
            let root = root.canonicalize().unwrap_or(root);
            let db = if db.is_absolute() { db } else { root.join(&db) };
            mcp::run_server(&root, &db)
        }
        Commands::Mcp { command } => match command {
            McpCommands::Config { root } => mcp_config(&root),
            McpCommands::Install {
                root,
                claude_desktop,
                claude_code,
            } => mcp_install(&root, claude_desktop, claude_code),
            McpCommands::Status => mcp_status(),
            McpCommands::Uninstall {
                claude_desktop,
                claude_code,
            } => mcp_uninstall(claude_desktop, claude_code),
        },
    }
}

fn init_logging(verbosity: u8) {
    let level = match verbosity {
        0 => "info",
        1 => "debug",
        _ => "trace",
    };
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(level))
        .format_timestamp_secs()
        .init();
}

fn list_symbols(
    db: &Path,
    file: Option<&PathBuf>,
    kind: Option<&str>,
    name: Option<&str>,
    limit: Option<usize>,
    format: OutputFormat,
) -> Result<()> {
    let store = open_store_for_query(db)?;
    let file_str = file.map(|p| p.to_string_lossy().to_string());
    let symbols: Vec<SymbolRecord> = store.list_symbols(file_str.as_deref(), kind, name, limit)?;
    output_symbols(&symbols, format)
}

fn find_implementation(
    db: &Path,
    file: &Path,
    line: Option<usize>,
    character: Option<usize>,
    limit: Option<usize>,
    kind: Option<&str>,
    format: OutputFormat,
) -> Result<()> {
    let store = open_store_for_query(db)?;
    let (file, line, character) = parse_file_position(file, line, character)?;
    let target = resolve_symbol_at(&store, &file, line, character)?;

    let mut impl_edges = store.edges_to(&target.id)?;
    let impl_ids: Vec<String> = impl_edges.drain(..).map(|e| e.src).collect();
    let mut impl_symbols = store.symbols_by_ids(&impl_ids)?;

    if impl_symbols.is_empty() {
        // Use dependency graph: implementations would be in files that depend on the target's file
        let dependents = store.get_dependents(&target.file)?;
        if dependents.is_empty() {
            // No dependency info - fall back to searching all files
            impl_symbols = store.list_symbols(None, kind, Some(&target.name), limit)?;
        } else {
            // Search only in dependent files plus the target's own file
            for dep_file in dependents.iter().chain(std::iter::once(&target.file)) {
                let file_symbols =
                    store.list_symbols(Some(dep_file), kind, Some(&target.name), limit)?;
                impl_symbols.extend(file_symbols);
            }
        }
    }

    if let Some(k) = kind {
        impl_symbols.retain(|s| s.kind == k);
    }
    impl_symbols.retain(|s| s.id != target.id);
    dedup_symbols(&mut impl_symbols);
    if let Some(lim) = limit {
        impl_symbols.truncate(lim);
    }

    output_implementations(&target, &impl_symbols, format)
}

fn find_usages(
    db: &Path,
    file: &Path,
    line: Option<usize>,
    character: Option<usize>,
    limit: Option<usize>,
    format: OutputFormat,
) -> Result<()> {
    let store = open_store_for_query(db)?;
    let (file, line, character) = parse_file_position(file, line, character)?;
    let target = resolve_symbol_at(&store, &file, line, character)?;
    let workspace_root = workspace_root_from_db(db).unwrap_or_else(|_| {
        file.parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
    });

    let mut refs = store.references_for_symbol(&target.id)?;
    let mut seen = HashSet::new();
    // Filter out the definition span and deduplicate
    refs.retain(|r| {
        if r.file == target.file && r.start >= target.start && r.end <= target.end {
            return false;
        }
        seen.insert((r.file.clone(), r.start, r.end))
    });
    // If no useful references found, try name-based search
    if refs.is_empty() {
        refs = search_usages_by_name(&store, &target, &workspace_root)?;
        // Filter out definition span for fallback results too
        refs.retain(|r| {
            if r.file == target.file && r.start >= target.start && r.end <= target.end {
                return false;
            }
            seen.insert((r.file.clone(), r.start, r.end))
        });
    }
    if let Some(lim) = limit {
        refs.truncate(lim);
    }

    output_usages(&target, &refs, format)
}

/// Output for usages with file grouping
#[derive(serde::Serialize)]
struct UsagesOutput {
    target: SymbolOutput,
    files: Vec<FileUsages>,
    summary: UsagesSummary,
}

#[derive(serde::Serialize)]
struct FileUsages {
    file: String,
    context: String,
    count: usize,
    usages: Vec<UsageLocation>,
}

#[derive(serde::Serialize)]
struct UsageLocation {
    start: Position,
    end: Position,
}

#[derive(serde::Serialize)]
struct UsagesSummary {
    total: usize,
    files: usize,
    prod: usize,
    test: usize,
}

/// Format and output usages for a target symbol
fn output_usages(
    target: &SymbolRecord,
    refs: &[store::ReferenceRecord],
    format: OutputFormat,
) -> Result<()> {
    let target_out = SymbolOutput::from_record(target)
        .ok_or_else(|| anyhow!("Failed to resolve target position"))?;

    // Group references by file
    let mut by_file: std::collections::BTreeMap<String, Vec<&store::ReferenceRecord>> =
        std::collections::BTreeMap::new();
    for r in refs {
        by_file.entry(r.file.clone()).or_default().push(r);
    }

    // Build structured output
    let mut test_count = 0;
    let mut prod_count = 0;
    let file_usages: Vec<FileUsages> = by_file
        .iter()
        .map(|(file, file_refs)| {
            let is_test = is_test_file(file);
            let usages: Vec<UsageLocation> = file_refs
                .iter()
                .filter_map(|r| {
                    let (line, col) = offset_to_line_char_in_file(&r.file, r.start).ok()?;
                    let (end_line, end_col) = offset_to_line_char_in_file(&r.file, r.end).ok()?;
                    Some(UsageLocation {
                        start: Position {
                            line,
                            character: col,
                        },
                        end: Position {
                            line: end_line,
                            character: end_col,
                        },
                    })
                })
                .collect();

            if is_test {
                test_count += usages.len();
            } else {
                prod_count += usages.len();
            }

            FileUsages {
                file: file.clone(),
                context: if is_test { "test" } else { "prod" }.to_string(),
                count: usages.len(),
                usages,
            }
        })
        .collect();

    let total = test_count + prod_count;
    let summary = UsagesSummary {
        total,
        files: by_file.len(),
        prod: prod_count,
        test: test_count,
    };

    match format {
        OutputFormat::Json => {
            let output = UsagesOutput {
                target: target_out,
                files: file_usages,
                summary,
            };
            println!("{}", serde_json::to_string_pretty(&output)?);
        }
        OutputFormat::Jsonl => {
            // First line is target, then each file's usages
            println!("{}", serde_json::to_string(&target_out)?);
            for fu in &file_usages {
                println!("{}", serde_json::to_string(fu)?);
            }
        }
        OutputFormat::Csv => {
            let mut wtr = csv::Writer::from_writer(std::io::stdout());
            wtr.write_record([
                "file",
                "line",
                "character",
                "end_line",
                "end_character",
                "context",
            ])?;
            for fu in &file_usages {
                for u in &fu.usages {
                    wtr.write_record([
                        &fu.file,
                        &u.start.line.to_string(),
                        &u.start.character.to_string(),
                        &u.end.line.to_string(),
                        &u.end.character.to_string(),
                        &fu.context,
                    ])?;
                }
            }
            wtr.flush()?;
        }
        OutputFormat::Tsv => {
            println!("file\tline\tcharacter\tend_line\tend_character\tcontext");
            for fu in &file_usages {
                for u in &fu.usages {
                    println!(
                        "{}\t{}\t{}\t{}\t{}\t{}",
                        fu.file,
                        u.start.line,
                        u.start.character,
                        u.end.line,
                        u.end.character,
                        fu.context
                    );
                }
            }
        }
        OutputFormat::Text => {
            println!(
                "Target: {} {} {}",
                target_out.kind,
                target_out.name,
                target_out.location()
            );
            if refs.is_empty() {
                println!("No usages found.");
            } else {
                for fu in &file_usages {
                    let context = if fu.context == "test" {
                        "[test]"
                    } else {
                        "[prod]"
                    };
                    println!("\n{} {} ({} usages)", context, fu.file, fu.count);
                    for u in &fu.usages {
                        println!(
                            "  {}:{}-{}:{}",
                            u.start.line, u.start.character, u.end.line, u.end.character
                        );
                    }
                }
                println!(
                    "\nSummary: {} usages in {} files ({} prod, {} test)",
                    summary.total, summary.files, summary.prod, summary.test
                );
            }
        }
    }

    Ok(())
}

/// Detailed symbol output with edges and references
#[derive(serde::Serialize)]
struct SymbolDetailOutput {
    #[serde(flatten)]
    base: SymbolOutput,
    outgoing_edges: Vec<EdgeOutput>,
    incoming_edges: Vec<EdgeOutput>,
    references: Vec<ReferenceOutput>,
}

#[derive(serde::Serialize)]
struct EdgeOutput {
    src: String,
    dst: String,
    kind: String,
}

#[derive(serde::Serialize)]
struct ReferenceOutput {
    file: String,
    start: Position,
    end: Position,
}

fn show_symbol(
    db: &Path,
    name: &str,
    file: Option<&PathBuf>,
    kind: Option<&str>,
    limit: Option<usize>,
    format: OutputFormat,
) -> Result<()> {
    let store = open_store_for_query(db)?;
    let file_str = file.map(|p| p.to_string_lossy().to_string());
    let symbols = store.list_symbols(file_str.as_deref(), kind, Some(name), limit)?;

    if symbols.is_empty() {
        match format {
            OutputFormat::Json => {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({ "symbols": [] }))?
                );
            }
            OutputFormat::Jsonl => {} // No output for empty results
            OutputFormat::Csv | OutputFormat::Tsv => {
                // Just headers for empty results
                let sep = if matches!(format, OutputFormat::Csv) {
                    ","
                } else {
                    "\t"
                };
                println!(
                    "name{}kind{}location{}visibility{}container",
                    sep, sep, sep, sep
                );
            }
            OutputFormat::Text => {
                println!("No symbols found for name '{}'.", name);
            }
        }
        return Ok(());
    }

    let workspace_root = workspace_root_from_db(db)
        .unwrap_or_else(|_| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    // Build detailed output for each symbol
    let detailed_symbols: Vec<SymbolDetailOutput> = symbols
        .iter()
        .filter_map(|sym| {
            let base = SymbolOutput::from_record(sym)?;
            let outgoing = store.edges_from(&sym.id).ok()?;
            let incoming = store.edges_to(&sym.id).ok()?;
            let mut refs = store.references_for_symbol(&sym.id).ok()?;
            if refs.is_empty() {
                refs = search_usages_by_name(&store, sym, &workspace_root).ok()?;
            }

            let outgoing_edges: Vec<EdgeOutput> = outgoing
                .iter()
                .map(|e| EdgeOutput {
                    src: e.src.clone(),
                    dst: e.dst.clone(),
                    kind: e.kind.clone(),
                })
                .collect();

            let incoming_edges: Vec<EdgeOutput> = incoming
                .iter()
                .map(|e| EdgeOutput {
                    src: e.src.clone(),
                    dst: e.dst.clone(),
                    kind: e.kind.clone(),
                })
                .collect();

            let references: Vec<ReferenceOutput> = refs
                .iter()
                .filter_map(|r| {
                    let (r_line, r_col) = offset_to_line_char_in_file(&r.file, r.start).ok()?;
                    let (r_end_line, r_end_col) =
                        offset_to_line_char_in_file(&r.file, r.end).ok()?;
                    Some(ReferenceOutput {
                        file: r.file.clone(),
                        start: Position {
                            line: r_line,
                            character: r_col,
                        },
                        end: Position {
                            line: r_end_line,
                            character: r_end_col,
                        },
                    })
                })
                .collect();

            Some(SymbolDetailOutput {
                base,
                outgoing_edges,
                incoming_edges,
                references,
            })
        })
        .collect();

    match format {
        OutputFormat::Json => {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({ "symbols": detailed_symbols }))?
            );
        }
        OutputFormat::Jsonl => {
            for sym in &detailed_symbols {
                println!("{}", serde_json::to_string(sym)?);
            }
        }
        OutputFormat::Csv => {
            let mut wtr = csv::Writer::from_writer(std::io::stdout());
            wtr.write_record([
                "name",
                "kind",
                "location",
                "visibility",
                "container",
                "outgoing_edges",
                "incoming_edges",
                "references_count",
            ])?;
            for sym in &detailed_symbols {
                wtr.write_record([
                    sym.base.name.as_str(),
                    sym.base.kind.as_str(),
                    &sym.base.location(),
                    sym.base.visibility.as_deref().unwrap_or(""),
                    sym.base.container.as_deref().unwrap_or(""),
                    &sym.outgoing_edges.len().to_string(),
                    &sym.incoming_edges.len().to_string(),
                    &sym.references.len().to_string(),
                ])?;
            }
            wtr.flush()?;
        }
        OutputFormat::Tsv => {
            println!("name\tkind\tlocation\tvisibility\tcontainer\toutgoing_edges\tincoming_edges\treferences_count");
            for sym in &detailed_symbols {
                println!(
                    "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                    sym.base.name,
                    sym.base.kind,
                    sym.base.location(),
                    sym.base.visibility.as_deref().unwrap_or(""),
                    sym.base.container.as_deref().unwrap_or(""),
                    sym.outgoing_edges.len(),
                    sym.incoming_edges.len(),
                    sym.references.len()
                );
            }
        }
        OutputFormat::Text => {
            for sym in &detailed_symbols {
                let visibility = sym.base.visibility.as_deref().unwrap_or("");
                let container = sym.base.container.as_deref().unwrap_or("");
                println!(
                    "Symbol: {} {} {} vis={} container={}",
                    sym.base.kind,
                    sym.base.name,
                    sym.base.location(),
                    visibility,
                    container
                );
                if let Some(qualifier) = &sym.base.qualifier {
                    println!("  qualifier: {}", qualifier);
                }
                if !sym.outgoing_edges.is_empty() {
                    println!("  outgoing edges:");
                    for e in &sym.outgoing_edges {
                        println!("    {} -> {} ({})", e.src, e.dst, e.kind);
                    }
                }
                if !sym.incoming_edges.is_empty() {
                    println!("  incoming edges:");
                    for e in &sym.incoming_edges {
                        println!("    {} -> {} ({})", e.src, e.dst, e.kind);
                    }
                }
                if !sym.references.is_empty() {
                    println!("  references:");
                    for r in &sym.references {
                        println!(
                            "    {}:{}:{}-{}:{}",
                            r.file, r.start.line, r.start.character, r.end.line, r.end.character
                        );
                    }
                }
            }
        }
    }

    Ok(())
}

/// Definition output wrapper
#[derive(serde::Serialize)]
struct DefinitionOutput {
    definition: SymbolOutput,
}

fn find_definition(
    db: &Path,
    file: &Path,
    line: Option<usize>,
    character: Option<usize>,
    format: OutputFormat,
) -> Result<()> {
    let store = open_store_for_query(db)?;
    let (file, line, character) = parse_file_position(file, line, character)?;
    let canonical_file = file.canonicalize().unwrap_or_else(|_| file.to_path_buf());
    let file_str = normalize_path(&canonical_file);
    let contents = fs::read(&canonical_file)?;
    let offset = line_char_to_offset(&contents, line, character)
        .ok_or_else(|| anyhow!("could not map line/character to byte offset"))?
        as i64;

    // First, check if cursor is on a recorded reference - if so, look up its target symbol
    let definition = if let Some(ref_record) = store.reference_at_position(&file_str, offset)? {
        // Found a reference - look up the symbol it points to
        let symbols = store.symbols_by_ids(std::slice::from_ref(&ref_record.symbol_id))?;
        if let Some(sym) = symbols.into_iter().next() {
            sym
        } else {
            // Reference exists but symbol not found - fall back to resolve_symbol_at
            resolve_symbol_at(&store, &file, line, character)?
        }
    } else {
        // No reference at position - use standard resolution
        resolve_symbol_at(&store, &file, line, character)?
    };

    let def_out = SymbolOutput::from_record(&definition)
        .ok_or_else(|| anyhow!("Failed to resolve definition position"))?;

    match format {
        OutputFormat::Json => {
            let output = DefinitionOutput {
                definition: def_out,
            };
            println!("{}", serde_json::to_string_pretty(&output)?);
        }
        OutputFormat::Jsonl => {
            println!("{}", serde_json::to_string(&def_out)?);
        }
        OutputFormat::Csv => {
            let mut wtr = csv::Writer::from_writer(std::io::stdout());
            wtr.write_record(["name", "kind", "location", "visibility", "container"])?;
            wtr.write_record(def_out.to_row())?;
            wtr.flush()?;
        }
        OutputFormat::Tsv => {
            println!("name\tkind\tlocation\tvisibility\tcontainer");
            println!("{}", def_out.to_row().join("\t"));
        }
        OutputFormat::Text => {
            let container = def_out
                .container
                .as_deref()
                .map(|c| format!(" in {c}"))
                .unwrap_or_default();
            println!(
                "Definition: {} {} {}{}",
                def_out.kind,
                def_out.name,
                def_out.location(),
                container
            );
        }
    }

    Ok(())
}

/// Duplicates output structure
#[derive(serde::Serialize)]
struct DuplicatesOutput {
    groups: Vec<DuplicateGroupOutput>,
    summary: DuplicatesSummary,
}

#[derive(serde::Serialize)]
struct DuplicateGroupOutput {
    content_hash: String,
    count: usize,
    symbols: Vec<SymbolOutput>,
}

#[derive(serde::Serialize)]
struct DuplicatesSummary {
    total_groups: usize,
    total_duplicates: usize,
}

fn find_duplicates(
    db: &Path,
    uncommitted: bool,
    staged: bool,
    kind: Option<&str>,
    min_count: usize,
    format: OutputFormat,
) -> Result<()> {
    let store = open_store_for_query(db)?;
    let workspace_root = workspace_root_from_db(db)?;

    // Get file filter based on git flags
    let file_filter: Option<Vec<String>> = if uncommitted || staged {
        let files = get_git_changed_files(&workspace_root, uncommitted, staged)?;
        if files.is_empty() {
            output_empty_duplicates(format)?;
            return Ok(());
        }
        Some(files)
    } else {
        None
    };

    let groups = store.find_duplicate_groups(min_count, kind, file_filter.as_deref())?;

    // Build structured output
    let group_outputs: Vec<DuplicateGroupOutput> = groups
        .iter()
        .map(|group| {
            let symbols: Vec<SymbolOutput> = group
                .symbols
                .iter()
                .filter_map(SymbolOutput::from_record)
                .collect();
            DuplicateGroupOutput {
                content_hash: group.content_hash.clone(),
                count: symbols.len(),
                symbols,
            }
        })
        .collect();

    let total_duplicates: usize = group_outputs.iter().map(|g| g.count).sum();
    let summary = DuplicatesSummary {
        total_groups: group_outputs.len(),
        total_duplicates,
    };

    match format {
        OutputFormat::Json => {
            let output = DuplicatesOutput {
                groups: group_outputs,
                summary,
            };
            println!("{}", serde_json::to_string_pretty(&output)?);
        }
        OutputFormat::Jsonl => {
            for group in &group_outputs {
                println!("{}", serde_json::to_string(group)?);
            }
        }
        OutputFormat::Csv => {
            let mut wtr = csv::Writer::from_writer(std::io::stdout());
            wtr.write_record(["group_hash", "name", "kind", "location", "container"])?;
            for group in &group_outputs {
                let short_hash = &group.content_hash[..8.min(group.content_hash.len())];
                for sym in &group.symbols {
                    wtr.write_record([
                        short_hash,
                        &sym.name,
                        &sym.kind,
                        &sym.location(),
                        sym.container.as_deref().unwrap_or(""),
                    ])?;
                }
            }
            wtr.flush()?;
        }
        OutputFormat::Tsv => {
            println!("group_hash\tname\tkind\tlocation\tcontainer");
            for group in &group_outputs {
                let short_hash = &group.content_hash[..8.min(group.content_hash.len())];
                for sym in &group.symbols {
                    println!(
                        "{}\t{}\t{}\t{}\t{}",
                        short_hash,
                        sym.name,
                        sym.kind,
                        sym.location(),
                        sym.container.as_deref().unwrap_or("")
                    );
                }
            }
        }
        OutputFormat::Text => {
            if group_outputs.is_empty() {
                println!("No duplicates found.");
                return Ok(());
            }

            println!(
                "Found {} duplicate groups ({} total symbols)\n",
                summary.total_groups, summary.total_duplicates
            );

            for (i, group) in group_outputs.iter().enumerate() {
                let short_hash = &group.content_hash[..8.min(group.content_hash.len())];
                println!(
                    "Group {} ({} duplicates, hash: {}):",
                    i + 1,
                    group.count,
                    short_hash
                );
                for sym in &group.symbols {
                    let container = sym
                        .container
                        .as_deref()
                        .map(|c| format!(" in {c}"))
                        .unwrap_or_default();
                    println!(
                        "  {:<10} {:<30} {}{}",
                        sym.kind,
                        sym.name,
                        sym.location(),
                        container
                    );
                }
                println!();
            }
        }
    }

    Ok(())
}

fn output_empty_duplicates(format: OutputFormat) -> Result<()> {
    match format {
        OutputFormat::Json => {
            let output = DuplicatesOutput {
                groups: vec![],
                summary: DuplicatesSummary {
                    total_groups: 0,
                    total_duplicates: 0,
                },
            };
            println!("{}", serde_json::to_string_pretty(&output)?);
        }
        OutputFormat::Jsonl => {} // No output for empty results
        OutputFormat::Csv => {
            let mut wtr = csv::Writer::from_writer(std::io::stdout());
            wtr.write_record(["group_hash", "name", "kind", "location", "container"])?;
            wtr.flush()?;
        }
        OutputFormat::Tsv => {
            println!("group_hash\tname\tkind\tlocation\tcontainer");
        }
        OutputFormat::Text => {
            println!("No changed files found.");
        }
    }
    Ok(())
}

/// Get list of changed files from git.
/// If `uncommitted` is true, includes working tree changes (unstaged + staged).
/// If `staged` is true, includes only staged changes.
fn get_git_changed_files(
    workspace_root: &Path,
    uncommitted: bool,
    staged: bool,
) -> Result<Vec<String>> {
    use std::process::Command;

    let mut files = HashSet::new();

    if staged {
        // Get staged files only
        let output = Command::new("git")
            .args(["diff", "--cached", "--name-only"])
            .current_dir(workspace_root)
            .output()
            .context("failed to run git diff --cached")?;

        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                if !line.is_empty() {
                    let full_path = workspace_root.join(line);
                    if let Ok(canonical) = full_path.canonicalize() {
                        files.insert(normalize_path(&canonical));
                    }
                }
            }
        }
    }

    if uncommitted {
        // Get all uncommitted changes (staged + unstaged)
        let output = Command::new("git")
            .args(["diff", "--name-only", "HEAD"])
            .current_dir(workspace_root)
            .output()
            .context("failed to run git diff HEAD")?;

        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                if !line.is_empty() {
                    let full_path = workspace_root.join(line);
                    if let Ok(canonical) = full_path.canonicalize() {
                        files.insert(normalize_path(&canonical));
                    }
                }
            }
        }

        // Also get untracked files
        let output = Command::new("git")
            .args(["ls-files", "--others", "--exclude-standard"])
            .current_dir(workspace_root)
            .output()
            .context("failed to run git ls-files")?;

        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                if !line.is_empty() {
                    let full_path = workspace_root.join(line);
                    if let Ok(canonical) = full_path.canonicalize() {
                        files.insert(normalize_path(&canonical));
                    }
                }
            }
        }
    }

    Ok(files.into_iter().collect())
}

fn resolve_symbol_at(
    store: &store::IndexStore,
    file: &Path,
    line: usize,
    character: usize,
) -> Result<SymbolRecord> {
    let canonical_file = file.canonicalize().unwrap_or_else(|_| file.to_path_buf());
    let file_str = normalize_path(&canonical_file);
    let symbols = store.list_symbols(Some(&file_str), None, None, None)?;
    let contents = fs::read(&canonical_file)?;
    let offset = line_char_to_offset(&contents, line, character)
        .ok_or_else(|| anyhow!("could not map line/character to byte offset"))?
        as i64;

    let ident = find_identifier_at_offset(&contents, offset as usize);

    if let Some(def) = narrowest_symbol_covering(&symbols, offset, ident.as_deref()) {
        return Ok(def);
    }

    let ident = ident.ok_or_else(|| anyhow!("no symbol found at {}:{}", file.display(), line))?;
    let mut candidates = store.list_symbols(None, None, Some(&ident), None)?;
    if candidates.is_empty() {
        bail!("no symbol found at {}:{}", file.display(), line);
    }
    candidates.sort_by(|a, b| {
        a.file
            .cmp(&b.file)
            .then_with(|| a.qualifier.cmp(&b.qualifier))
            .then_with(|| a.start.cmp(&b.start))
    });
    Ok(candidates.remove(0))
}

fn narrowest_symbol_covering(
    symbols: &[SymbolRecord],
    offset: i64,
    ident: Option<&str>,
) -> Option<SymbolRecord> {
    let mut best: Option<&SymbolRecord> = None;
    for sym in symbols {
        if sym.start <= offset && offset < sym.end {
            if let Some(id) = ident {
                if !identifier_matches_symbol(id, sym) {
                    continue;
                }
            }
            let span = sym.end - sym.start;
            if best.map(|b| span < (b.end - b.start)).unwrap_or(true) {
                best = Some(sym);
            }
        }
    }

    best.cloned()
}

fn find_identifier_at_offset(buf: &[u8], offset: usize) -> Option<String> {
    if offset >= buf.len() {
        return None;
    }
    let is_ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut start = offset;
    while start > 0 && is_ident(buf[start.saturating_sub(1)]) {
        start -= 1;
    }
    let mut end = offset;
    while end < buf.len() && is_ident(buf[end]) {
        end += 1;
    }
    if start == end {
        return None;
    }
    std::str::from_utf8(&buf[start..end])
        .ok()
        .map(|s| s.to_string())
}

fn identifier_matches_symbol(ident: &str, sym: &SymbolRecord) -> bool {
    if ident == sym.name {
        return true;
    }
    matches!(
        ident,
        "fn" | "function" | "class" | "interface" | "enum" | "struct" | "impl"
    )
}

fn dedup_symbols(symbols: &mut Vec<SymbolRecord>) {
    let mut seen = HashSet::new();
    symbols.retain(|s| seen.insert(s.id.clone()));
}

fn search_usages_by_name(
    store: &store::IndexStore,
    target: &SymbolRecord,
    _workspace_root: &Path,
) -> Result<Vec<store::ReferenceRecord>> {
    let mut refs = Vec::new();

    // Use dependency graph: only search files that depend on the target's file
    let dependents = store.get_dependents(&target.file)?;
    let paths: Vec<PathBuf> = if dependents.is_empty() {
        // No dependency info - fall back to all indexed files
        store.list_paths()?.into_iter().map(PathBuf::from).collect()
    } else {
        // Search dependents plus the target's own file
        let mut paths: Vec<PathBuf> = dependents.into_iter().map(PathBuf::from).collect();
        paths.push(PathBuf::from(&target.file));
        paths
    };

    for path in paths {
        let canonical = path.canonicalize().unwrap_or(path.clone());
        let path_str = normalize_path(&canonical);
        let buf = match fs::read(&canonical) {
            Ok(b) => b,
            Err(_) => continue,
        };
        let mut idx = 0usize;
        while let Some(pos) = buf[idx..]
            .windows(target.name.len())
            .position(|w| w == target.name.as_bytes())
        {
            let abs = idx + pos;
            if !is_word_boundary(&buf, abs, target.name.len()) {
                idx += pos + target.name.len();
                continue;
            }
            let abs = idx + pos;
            let start = abs as i64;
            let end = (abs + target.name.len()) as i64;
            if path_str == target.file && start >= target.start && end <= target.end {
                idx += pos + target.name.len();
                continue;
            }
            refs.push(store::ReferenceRecord {
                file: path_str.clone(),
                start,
                end,
                symbol_id: target.id.clone(),
            });
            idx += pos + target.name.len();
        }
    }
    Ok(refs)
}

fn is_word_boundary(buf: &[u8], start: usize, len: usize) -> bool {
    let is_ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let before_ok = if start == 0 {
        true
    } else {
        !is_ident(buf[start - 1])
    };
    let end = start + len;
    let after_ok = if end >= buf.len() {
        true
    } else {
        !is_ident(buf[end])
    };
    before_ok && after_ok
}

fn line_char_to_offset(buf: &[u8], line: usize, character: usize) -> Option<usize> {
    if line == 0 || character == 0 {
        return None;
    }
    let mut idx = 0;
    let mut current_line = 1usize;
    while current_line < line {
        if let Some(pos) = buf[idx..].iter().position(|b| *b == b'\n') {
            idx += pos + 1;
            current_line += 1;
        } else {
            return None;
        }
    }
    let line_end = buf[idx..]
        .iter()
        .position(|b| *b == b'\n')
        .map(|p| idx + p)
        .unwrap_or(buf.len());
    let line_len = line_end - idx;
    let col = character.saturating_sub(1).min(line_len);
    Some(idx + col)
}

/// Check if a file path indicates a test file based on common conventions.
/// Returns true for:
/// - Files in `tests/`, `__tests__/`, `test/` directories
/// - Files matching `*_test.rs`, `*_spec.rs` (Rust)
/// - Files matching `*.test.ts`, `*.spec.ts`, `*.test.tsx`, `*.spec.tsx` (TypeScript)
fn is_test_file(path: &str) -> bool {
    let path_lower = path.to_lowercase();

    // Check directory patterns
    if path_lower.contains("/tests/")
        || path_lower.contains("/__tests__/")
        || path_lower.contains("/test/")
        || path_lower.contains("/spec/")
    {
        return true;
    }

    // Check file name patterns
    if let Some(file_name) = path.rsplit('/').next() {
        let name_lower = file_name.to_lowercase();
        // Rust patterns
        if name_lower.ends_with("_test.rs") || name_lower.ends_with("_spec.rs") {
            return true;
        }
        // TypeScript/JavaScript patterns
        if name_lower.ends_with(".test.ts")
            || name_lower.ends_with(".spec.ts")
            || name_lower.ends_with(".test.tsx")
            || name_lower.ends_with(".spec.tsx")
            || name_lower.ends_with(".test.js")
            || name_lower.ends_with(".spec.js")
            || name_lower.ends_with(".test.jsx")
            || name_lower.ends_with(".spec.jsx")
        {
            return true;
        }
    }

    false
}

fn offset_to_line_char_in_file(path: &str, offset: i64) -> Result<(usize, usize)> {
    let buf = fs::read(path).with_context(|| format!("failed to read {}", path))?;
    offset_to_line_char_in_buf(&buf, offset as usize)
        .ok_or_else(|| anyhow!("could not map byte offset for {path}"))
}

fn offset_to_line_char_in_buf(buf: &[u8], offset: usize) -> Option<(usize, usize)> {
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

fn workspace_root_from_db(db: &Path) -> Result<PathBuf> {
    let abs = if db.is_absolute() {
        db.to_path_buf()
    } else {
        env::current_dir()
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
    Err(anyhow!("could not derive workspace root from db path"))
}

fn parse_file_position(
    file: &Path,
    line: Option<usize>,
    character: Option<usize>,
) -> Result<(PathBuf, usize, usize)> {
    let (base, embedded) = split_file_and_embedded_position(file);
    if let (Some(l), Some(c)) = (line, character) {
        return Ok((base, l, c));
    }
    if let Some((l, c)) = embedded {
        return Ok((base, l, c));
    }

    Err(anyhow!(
        "must provide --line and --character or include :line:character in --file"
    ))
}

fn split_file_and_embedded_position(file: &Path) -> (PathBuf, Option<(usize, usize)>) {
    let raw = file.to_string_lossy();
    let parts: Vec<&str> = raw.split(':').collect();
    if parts.len() >= 3 {
        if let (Ok(line), Ok(character)) = (
            parts[parts.len() - 2].parse::<usize>(),
            parts[parts.len() - 1].parse::<usize>(),
        ) {
            let base = parts[..parts.len() - 2].join(":");
            return (PathBuf::from(base), Some((line, character)));
        }
    }
    (file.to_path_buf(), None)
}

// ==================== MCP Configuration Commands ====================

/// Get the path to Claude Desktop config file
fn claude_desktop_config_path() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        dirs::home_dir().map(|h| {
            h.join("Library/Application Support/Claude/claude_desktop_config.json")
        })
    }
    #[cfg(target_os = "windows")]
    {
        std::env::var("APPDATA")
            .ok()
            .map(|appdata| PathBuf::from(appdata).join("Claude/claude_desktop_config.json"))
    }
    #[cfg(target_os = "linux")]
    {
        dirs::home_dir().map(|h| h.join(".config/Claude/claude_desktop_config.json"))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        None
    }
}

/// Get the path to project-level Claude Code MCP config
fn claude_code_config_path() -> PathBuf {
    PathBuf::from(".claude/mcp.json")
}

/// Find the gabb binary path
fn find_gabb_binary() -> String {
    // Try to find the binary in common locations
    if let Ok(current_exe) = std::env::current_exe() {
        return current_exe.to_string_lossy().to_string();
    }
    // Fallback to just "gabb" (assume it's in PATH)
    "gabb".to_string()
}

/// Generate MCP server config JSON for a workspace
fn generate_mcp_config(root: &Path, use_absolute_path: bool) -> serde_json::Value {
    let root_str = if use_absolute_path {
        root.canonicalize()
            .unwrap_or_else(|_| root.to_path_buf())
            .to_string_lossy()
            .to_string()
    } else {
        ".".to_string()
    };

    serde_json::json!({
        "mcpServers": {
            "gabb": {
                "command": find_gabb_binary(),
                "args": ["mcp-server", "--root", root_str]
            }
        }
    })
}

/// Print MCP configuration JSON
fn mcp_config(root: &Path) -> Result<()> {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let config = generate_mcp_config(&root, true);

    println!("Add this to your Claude Desktop config:\n");
    println!(
        "{}",
        serde_json::to_string_pretty(&config).unwrap_or_default()
    );
    println!();

    if let Some(config_path) = claude_desktop_config_path() {
        println!("Config file location:");
        println!("  {}", config_path.display());
    } else {
        println!("Config file locations:");
        println!("  macOS:   ~/Library/Application Support/Claude/claude_desktop_config.json");
        println!("  Windows: %APPDATA%\\Claude\\claude_desktop_config.json");
        println!("  Linux:   ~/.config/Claude/claude_desktop_config.json");
    }

    println!();
    println!("Or run `gabb mcp install` to install automatically.");

    Ok(())
}

/// Install gabb into MCP configuration
fn mcp_install(root: &Path, claude_desktop_only: bool, claude_code_only: bool) -> Result<()> {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let install_both = !claude_desktop_only && !claude_code_only;
    let mut installed_any = false;

    // Install to Claude Desktop
    if install_both || claude_desktop_only {
        if let Some(config_path) = claude_desktop_config_path() {
            match install_to_config_file(&config_path, &root, true) {
                Ok(true) => {
                    println!("✓ Installed gabb to Claude Desktop config");
                    println!("  {}", config_path.display());
                    installed_any = true;
                }
                Ok(false) => {
                    println!("✓ gabb already configured in Claude Desktop");
                }
                Err(e) => {
                    eprintln!("✗ Failed to install to Claude Desktop: {}", e);
                }
            }
        } else {
            println!("⚠ Claude Desktop config path not found on this platform");
        }
    }

    // Install to Claude Code (project-level)
    if install_both || claude_code_only {
        let config_path = claude_code_config_path();
        match install_to_config_file(&config_path, &root, false) {
            Ok(true) => {
                println!("✓ Installed gabb to Claude Code project config");
                println!("  {}", config_path.display());
                installed_any = true;
            }
            Ok(false) => {
                println!("✓ gabb already configured in Claude Code project config");
            }
            Err(e) => {
                eprintln!("✗ Failed to install to Claude Code: {}", e);
            }
        }
    }

    if installed_any {
        println!();
        println!("Restart Claude Desktop/Code to load the new MCP server.");
    }

    Ok(())
}

/// Install gabb config to a specific config file
/// Returns Ok(true) if installed, Ok(false) if already present
fn install_to_config_file(config_path: &Path, root: &Path, use_absolute: bool) -> Result<bool> {
    // Create parent directory if needed
    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent)?;
    }

    // Read existing config or create new one
    let mut config: serde_json::Value = if config_path.exists() {
        let content = fs::read_to_string(config_path)
            .with_context(|| format!("Failed to read {}", config_path.display()))?;
        serde_json::from_str(&content).unwrap_or_else(|_| serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    // Check if gabb is already configured
    if config
        .get("mcpServers")
        .and_then(|s| s.get("gabb"))
        .is_some()
    {
        return Ok(false);
    }

    // Backup existing config
    if config_path.exists() {
        let backup_path = config_path.with_extension("json.bak");
        fs::copy(config_path, &backup_path)
            .with_context(|| format!("Failed to backup {}", config_path.display()))?;
    }

    // Add gabb to mcpServers
    let gabb_config = generate_mcp_config(root, use_absolute);
    let mcp_servers = config
        .as_object_mut()
        .ok_or_else(|| anyhow!("Config is not a JSON object"))?
        .entry("mcpServers")
        .or_insert_with(|| serde_json::json!({}));

    if let Some(servers) = mcp_servers.as_object_mut() {
        if let Some(gabb) = gabb_config
            .get("mcpServers")
            .and_then(|s| s.get("gabb"))
            .cloned()
        {
            servers.insert("gabb".to_string(), gabb);
        }
    }

    // Write updated config
    let content = serde_json::to_string_pretty(&config)?;
    fs::write(config_path, content)
        .with_context(|| format!("Failed to write {}", config_path.display()))?;

    Ok(true)
}

/// Check MCP configuration status
fn mcp_status() -> Result<()> {
    let mut found_any = false;

    // Check Claude Desktop
    if let Some(config_path) = claude_desktop_config_path() {
        print!("Claude Desktop: ");
        if config_path.exists() {
            match check_gabb_in_config(&config_path) {
                Ok(true) => {
                    println!("✓ gabb configured");
                    println!("  {}", config_path.display());
                    found_any = true;
                }
                Ok(false) => {
                    println!("✗ gabb not configured");
                    println!("  Config exists at: {}", config_path.display());
                }
                Err(e) => {
                    println!("✗ Error reading config: {}", e);
                }
            }
        } else {
            println!("✗ Config file not found");
            println!("  Expected: {}", config_path.display());
        }
    } else {
        println!("Claude Desktop: ⚠ Platform not supported");
    }

    println!();

    // Check Claude Code (project-level)
    let code_config_path = claude_code_config_path();
    print!("Claude Code (project): ");
    if code_config_path.exists() {
        match check_gabb_in_config(&code_config_path) {
            Ok(true) => {
                println!("✓ gabb configured");
                println!("  {}", code_config_path.display());
                found_any = true;
            }
            Ok(false) => {
                println!("✗ gabb not configured");
                println!("  Config exists at: {}", code_config_path.display());
            }
            Err(e) => {
                println!("✗ Error reading config: {}", e);
            }
        }
    } else {
        println!("✗ No project config");
        println!("  Run `gabb mcp install --claude-code` to create");
    }

    println!();

    // Check if gabb binary is accessible
    print!("gabb binary: ");
    if let Ok(exe) = std::env::current_exe() {
        println!("✓ {}", exe.display());
    } else {
        println!("⚠ Could not determine path");
    }

    if !found_any {
        println!();
        println!("Run `gabb mcp install` to configure MCP for Claude.");
    }

    Ok(())
}

/// Check if gabb is configured in a config file
fn check_gabb_in_config(config_path: &Path) -> Result<bool> {
    let content = fs::read_to_string(config_path)?;
    let config: serde_json::Value = serde_json::from_str(&content)?;
    Ok(config
        .get("mcpServers")
        .and_then(|s| s.get("gabb"))
        .is_some())
}

/// Uninstall gabb from MCP configuration
fn mcp_uninstall(claude_desktop_only: bool, claude_code_only: bool) -> Result<()> {
    let uninstall_both = !claude_desktop_only && !claude_code_only;
    let mut removed_any = false;

    // Uninstall from Claude Desktop
    if uninstall_both || claude_desktop_only {
        if let Some(config_path) = claude_desktop_config_path() {
            if config_path.exists() {
                match uninstall_from_config_file(&config_path) {
                    Ok(true) => {
                        println!("✓ Removed gabb from Claude Desktop config");
                        removed_any = true;
                    }
                    Ok(false) => {
                        println!("✓ gabb was not in Claude Desktop config");
                    }
                    Err(e) => {
                        eprintln!("✗ Failed to uninstall from Claude Desktop: {}", e);
                    }
                }
            } else {
                println!("✓ Claude Desktop config does not exist");
            }
        }
    }

    // Uninstall from Claude Code
    if uninstall_both || claude_code_only {
        let config_path = claude_code_config_path();
        if config_path.exists() {
            match uninstall_from_config_file(&config_path) {
                Ok(true) => {
                    println!("✓ Removed gabb from Claude Code project config");
                    removed_any = true;
                }
                Ok(false) => {
                    println!("✓ gabb was not in Claude Code project config");
                }
                Err(e) => {
                    eprintln!("✗ Failed to uninstall from Claude Code: {}", e);
                }
            }
        } else {
            println!("✓ Claude Code project config does not exist");
        }
    }

    if removed_any {
        println!();
        println!("Restart Claude Desktop/Code to apply changes.");
    }

    Ok(())
}

/// Remove gabb from a config file
/// Returns Ok(true) if removed, Ok(false) if wasn't present
fn uninstall_from_config_file(config_path: &Path) -> Result<bool> {
    let content = fs::read_to_string(config_path)?;
    let mut config: serde_json::Value = serde_json::from_str(&content)?;

    // Check if gabb exists
    let has_gabb = config
        .get("mcpServers")
        .and_then(|s| s.get("gabb"))
        .is_some();

    if !has_gabb {
        return Ok(false);
    }

    // Backup before modifying
    let backup_path = config_path.with_extension("json.bak");
    fs::copy(config_path, &backup_path)?;

    // Remove gabb from mcpServers
    if let Some(mcp_servers) = config.get_mut("mcpServers").and_then(|s| s.as_object_mut()) {
        mcp_servers.remove("gabb");
    }

    // Write updated config
    let content = serde_json::to_string_pretty(&config)?;
    fs::write(config_path, content)?;

    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gabb_cli::indexer;
    use gabb_cli::store::IndexStore;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn resolves_symbol_at_position() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let file_path = root.join("sample.ts");
        fs::write(&file_path, "function foo() {}\nfunction bar() {}\n").unwrap();
        let file_path = file_path.canonicalize().unwrap();

        let db_path = root.join(".gabb/index.db");
        let store = IndexStore::open(&db_path).unwrap();
        indexer::build_full_index(root, &store).unwrap();

        let symbol = resolve_symbol_at(&store, &file_path, 1, 10).unwrap();
        assert_eq!(symbol.name, "foo");
        assert_eq!(symbol.kind, "function");
    }

    #[test]
    fn resolves_reference_by_name_when_not_a_definition() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let impl_path = root.join("indexer.rs");
        let caller_path = root.join("daemon.rs");
        fs::write(
            &impl_path,
            r#"
                fn build_full_index() {}
            "#,
        )
        .unwrap();
        let call_src = r#"fn main() { build_full_index(); }"#;
        fs::write(&caller_path, call_src).unwrap();
        let caller_path = caller_path.canonicalize().unwrap();

        let db_path = root.join(".gabb/index.db");
        let store = IndexStore::open(&db_path).unwrap();
        indexer::build_full_index(root, &store).unwrap();

        let offset = call_src.find("build_full_index").unwrap();
        let (line, character) = offset_to_line_char(call_src.as_bytes(), offset).unwrap();

        let symbol = resolve_symbol_at(&store, &caller_path, line, character).unwrap();
        assert_eq!(symbol.name, "build_full_index");
        assert!(symbol.file.ends_with("indexer.rs"));
    }

    #[test]
    fn usages_skip_definition_span() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let file_path = root.join("foo.ts");
        let source = "function foo() {}\nfoo();\n";
        fs::write(&file_path, source).unwrap();
        let file_path = file_path.canonicalize().unwrap();

        let db_path = root.join(".gabb/index.db");
        let store = IndexStore::open(&db_path).unwrap();
        indexer::build_full_index(root, &store).unwrap();

        let symbol = resolve_symbol_at(&store, &file_path, 1, 10).unwrap();
        let root = db_path.parent().and_then(|p| p.parent()).unwrap_or(root);
        let refs = super::search_usages_by_name(&store, &symbol, root).unwrap();
        assert!(
            refs.iter().all(|r| !(r.file == symbol.file
                && r.start >= symbol.start
                && r.end <= symbol.end)),
            "should not return reference within definition"
        );
    }

    #[test]
    fn finds_usages_across_files_via_fallback() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let def_path = root.join("lib.rs");
        let caller_path = root.join("main.rs");
        fs::write(
            &def_path,
            r#"
                pub fn build_full_index() {}
            "#,
        )
        .unwrap();
        fs::write(
            &caller_path,
            r#"
                fn main() {
                    build_full_index();
                }
            "#,
        )
        .unwrap();

        let db_path = root.join(".gabb/index.db");
        let store = IndexStore::open(&db_path).unwrap();
        indexer::build_full_index(root, &store).unwrap();

        let def_path = def_path.canonicalize().unwrap();
        let src = fs::read_to_string(&def_path).unwrap();
        let offset = src.find("build_full_index").unwrap();
        let (line, character) = offset_to_line_char(src.as_bytes(), offset).unwrap();
        let symbol = resolve_symbol_at(&store, &def_path, line, character).unwrap();
        let refs = super::search_usages_by_name(&store, &symbol, root).unwrap();
        assert!(
            refs.iter().any(|r| r.file.ends_with("main.rs")),
            "expected a usage in main.rs, got {:?}",
            refs
        );
    }

    #[test]
    fn parses_line_character_from_file_arg() {
        let file = PathBuf::from("src/daemon.rs:18:5");
        let (path, line, character) = parse_file_position(file.as_path(), None, None).unwrap();
        assert_eq!(path, PathBuf::from("src/daemon.rs"));
        assert_eq!(line, 18);
        assert_eq!(character, 5);

        // Explicit args override embedded position.
        let (path2, line2, character2) =
            parse_file_position(file.as_path(), Some(1), Some(2)).unwrap();
        assert_eq!(path2, PathBuf::from("src/daemon.rs"));
        assert_eq!(line2, 1);
        assert_eq!(character2, 2);
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
    fn detects_test_files_correctly() {
        // Directory-based patterns
        assert!(is_test_file("/project/tests/foo.rs"));
        assert!(is_test_file("/project/src/__tests__/component.test.ts"));
        assert!(is_test_file("/project/test/helper.ts"));
        assert!(is_test_file("/project/spec/models.spec.ts"));

        // Rust file patterns
        assert!(is_test_file("/project/src/indexer_test.rs"));
        assert!(is_test_file("/project/src/store_spec.rs"));

        // TypeScript/JavaScript file patterns
        assert!(is_test_file("/project/src/utils.test.ts"));
        assert!(is_test_file("/project/src/utils.spec.ts"));
        assert!(is_test_file("/project/src/component.test.tsx"));
        assert!(is_test_file("/project/src/component.spec.tsx"));
        assert!(is_test_file("/project/src/helper.test.js"));
        assert!(is_test_file("/project/src/helper.spec.jsx"));

        // Production files (should return false)
        assert!(!is_test_file("/project/src/main.rs"));
        assert!(!is_test_file("/project/src/lib.rs"));
        assert!(!is_test_file("/project/src/utils.ts"));
        assert!(!is_test_file("/project/src/component.tsx"));
        assert!(!is_test_file("/project/src/index.js"));

        // Edge cases
        assert!(!is_test_file("/project/src/testing.ts")); // "testing" != "test"
        assert!(!is_test_file("/project/src/contest.ts")); // contains "test" but not a test file
    }
}
