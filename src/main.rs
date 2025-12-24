use anyhow::{anyhow, bail, Context, Result};
use clap::{Parser, Subcommand};
use std::collections::HashSet;
use std::env;
use std::fs;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

use gabb_cli::daemon;
use gabb_cli::is_test_file;
use gabb_cli::mcp;
use gabb_cli::offset_to_line_col_in_file;
use gabb_cli::store;
use gabb_cli::store::{normalize_path, DbOpenResult, IndexStore, SymbolQuery, SymbolRecord};
use gabb_cli::workspace;
use gabb_cli::ExitCode;
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

/// Ensure the index is available using the shared daemon logic.
fn ensure_index_available(db: &Path, opts: &DaemonOptions) -> Result<()> {
    // Derive workspace root from db path
    let workspace_root = daemon::workspace_root_from_db(db)
        .unwrap_or_else(|_| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    let daemon_opts = daemon::EnsureIndexOptions {
        no_start_daemon: opts.no_start_daemon,
        timeout: std::time::Duration::from_secs(60),
        no_daemon_warnings: opts.no_daemon,
        auto_restart_on_version_mismatch: false, // CLI shows warning, user decides
    };

    daemon::ensure_index_available(&workspace_root, db, &daemon_opts)
}

// ==================== Output Formatting ====================

/// Options for displaying source code in output
#[derive(Clone, Copy, Default)]
struct SourceDisplayOptions {
    include_source: bool,
    context_lines: Option<usize>,
}

/// A symbol with resolved line/column positions for output
#[derive(serde::Serialize)]
struct SymbolOutput {
    id: String,
    name: String,
    kind: String,
    context: String, // "test" or "prod"
    file: String,
    start: Position,
    end: Position,
    #[serde(skip_serializing_if = "Option::is_none")]
    visibility: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    container: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    qualifier: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<String>,
}

#[derive(serde::Serialize, Clone)]
struct Position {
    line: usize,
    character: usize,
}

/// Output for file structure command showing hierarchical symbols
#[derive(serde::Serialize)]
struct FileStructure {
    file: String,
    context: String, // "test" or "prod"
    symbols: Vec<SymbolNode>,
}

/// A symbol node in the file structure hierarchy
#[derive(serde::Serialize, Clone)]
struct SymbolNode {
    name: String,
    kind: String,
    context: String, // "test" or "prod"
    start: Position,
    end: Position,
    #[serde(skip_serializing_if = "Option::is_none")]
    visibility: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    children: Vec<SymbolNode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<String>,
}

impl SymbolOutput {
    fn from_record(sym: &SymbolRecord) -> Option<Self> {
        Self::from_record_with_source(sym, SourceDisplayOptions::default())
    }

    fn from_record_with_source(sym: &SymbolRecord, opts: SourceDisplayOptions) -> Option<Self> {
        let (line, col) = offset_to_line_char_in_file(&sym.file, sym.start).ok()?;
        let (end_line, end_col) = offset_to_line_char_in_file(&sym.file, sym.end).ok()?;

        let source = if opts.include_source {
            mcp::extract_source(&sym.file, sym.start, sym.end, opts.context_lines)
        } else {
            None
        };

        // Determine context from file path OR inline test markers (#[cfg(test)], #[test])
        let context = if sym.is_test || is_test_file(&sym.file) {
            "test"
        } else {
            "prod"
        };

        Some(Self {
            id: sym.id.clone(),
            name: sym.name.clone(),
            kind: sym.kind.clone(),
            context: context.to_string(),
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
            source,
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
            self.context.clone(),
            self.location(),
            self.visibility.clone().unwrap_or_default(),
            self.container.clone().unwrap_or_default(),
        ]
    }
}

/// Format and output a list of symbols
fn output_symbols(
    symbols: &[SymbolRecord],
    format: OutputFormat,
    source_opts: SourceDisplayOptions,
) -> Result<()> {
    let outputs: Vec<SymbolOutput> = symbols
        .iter()
        .filter_map(|s| SymbolOutput::from_record_with_source(s, source_opts))
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
            wtr.write_record([
                "name",
                "kind",
                "context",
                "location",
                "visibility",
                "container",
            ])?;
            for sym in &outputs {
                wtr.write_record(sym.to_row())?;
            }
            wtr.flush()?;
        }
        OutputFormat::Tsv => {
            println!("name\tkind\tcontext\tlocation\tvisibility\tcontainer");
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
                    "{:<10} {:<30} [{}] {}{}",
                    sym.kind,
                    sym.name,
                    sym.context,
                    sym.location(),
                    container
                );
                if let Some(src) = &sym.source {
                    println!("{}\n", src);
                }
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
    source_opts: SourceDisplayOptions,
) -> Result<()> {
    let target_out = SymbolOutput::from_record(target)
        .ok_or_else(|| anyhow!("Failed to resolve target position"))?;
    let impl_outputs: Vec<SymbolOutput> = implementations
        .iter()
        .filter_map(|s| SymbolOutput::from_record_with_source(s, source_opts))
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
                if let Some(src) = &sym.source {
                    println!("{}\n", src);
                }
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

    /// Suppress non-essential output (for scripts). Errors still go to stderr.
    #[arg(short, long, global = true)]
    quiet: bool,

    /// Output format (text, json, jsonl, csv, tsv)
    #[arg(long, short = 'f', global = true, value_enum, default_value = "text")]
    format: OutputFormat,

    /// Workspace root (auto-detected from .gabb/, .git/, Cargo.toml, etc. if not specified)
    #[arg(long, short = 'w', global = true, env = "GABB_WORKSPACE")]
    workspace: Option<PathBuf>,

    /// Path to the SQLite index database (default: <workspace>/.gabb/index.db)
    #[arg(long, global = true, env = "GABB_DB")]
    db: Option<PathBuf>,

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
        /// Only show symbols from this file
        #[arg(long)]
        file: Option<PathBuf>,
        /// Only show symbols of this kind (function, class, interface, method, struct, enum, trait)
        #[arg(long)]
        kind: Option<String>,
        /// Only show symbols with this exact name (or fuzzy pattern if --fuzzy is used)
        #[arg(long)]
        name: Option<String>,
        /// Enable fuzzy/prefix search using FTS5 (supports patterns like "getUser*" or "usrsvc")
        #[arg(long)]
        fuzzy: bool,
        /// Limit the number of results
        #[arg(long)]
        limit: Option<usize>,
        /// Number of results to skip (for pagination)
        #[arg(long)]
        offset: Option<usize>,
        /// Include source code in output
        #[arg(long)]
        source: bool,
        /// Number of context lines before/after (like grep -C)
        #[arg(short = 'C', long)]
        context: Option<usize>,
    },
    /// Find implementations for symbol at a source position
    Implementation {
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
        /// Include source code in output
        #[arg(long)]
        source: bool,
        /// Number of context lines before/after (like grep -C)
        #[arg(short = 'C', long)]
        context: Option<usize>,
    },
    /// Find usages of the symbol at a source position
    Usages {
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
        /// Include source code in output
        #[arg(long)]
        source: bool,
        /// Number of context lines before/after (like grep -C)
        #[arg(short = 'C', long)]
        context: Option<usize>,
    },
    /// Show details for symbols with a given name
    Symbol {
        /// Symbol name to look up (or fuzzy pattern if --fuzzy is used)
        #[arg(long)]
        name: String,
        /// Only show symbols from this file
        #[arg(long)]
        file: Option<PathBuf>,
        /// Only show symbols of this kind (function, class, interface, method, struct, enum, trait)
        #[arg(long)]
        kind: Option<String>,
        /// Enable fuzzy/prefix search using FTS5 (supports patterns like "getUser*" or "usrsvc")
        #[arg(long)]
        fuzzy: bool,
        /// Limit the number of results
        #[arg(long)]
        limit: Option<usize>,
        /// Include source code in output
        #[arg(long)]
        source: bool,
        /// Number of context lines before/after (like grep -C)
        #[arg(short = 'C', long)]
        context: Option<usize>,
    },
    /// Go to definition: find where a symbol is declared
    Definition {
        /// Source file containing the reference. You can optionally append :line:character (1-based), e.g. ./src/daemon.rs:18:5
        #[arg(long)]
        file: PathBuf,
        /// 1-based line number within the file (optional if provided in --file)
        #[arg(long)]
        line: Option<usize>,
        /// 1-based character offset within the line (optional if provided in --file)
        #[arg(long, alias = "col")]
        character: Option<usize>,
        /// Include source code in output
        #[arg(long)]
        source: bool,
        /// Number of context lines before/after (like grep -C)
        #[arg(short = 'C', long)]
        context: Option<usize>,
    },
    /// Find duplicate code in the codebase
    Duplicates {
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
    McpServer,
    /// Manage MCP (Model Context Protocol) configuration for AI assistants
    Mcp {
        #[command(subcommand)]
        command: McpCommands,
    },
    /// Initialize gabb in a project
    Init {
        /// Create .claude/mcp.json for Claude Code integration
        #[arg(long)]
        mcp: bool,
        /// Add .gabb/ and .claude/ to .gitignore
        #[arg(long)]
        gitignore: bool,
        /// Create .claude/skills/gabb/ agent skill for discoverability
        #[arg(long)]
        skill: bool,
    },
    /// Interactive setup wizard for one-command onboarding
    Setup {
        /// Accept all defaults without prompting (non-interactive mode)
        #[arg(long, short = 'y')]
        yes: bool,
        /// Show what would happen without making changes
        #[arg(long)]
        dry_run: bool,
    },
    /// Find all files that #include this header (reverse dependency lookup)
    Includers {
        /// Header file to find includers for
        file: PathBuf,
        /// Follow transitive includers (files that include files that include this)
        #[arg(long)]
        transitive: bool,
        /// Maximum number of results
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Find all headers included by this file (forward dependency lookup)
    Includes {
        /// Source file to find includes for
        file: PathBuf,
        /// Follow transitive includes
        #[arg(long)]
        transitive: bool,
        /// Maximum number of results
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Show the structure of a file (symbols with hierarchy and positions)
    Structure {
        /// File to analyze
        file: PathBuf,
        /// Include source code snippets
        #[arg(long)]
        source: bool,
        /// Number of context lines before/after (like grep -C)
        #[arg(short = 'C', long)]
        context: Option<usize>,
    },
    /// Show index statistics (file counts, symbol counts, index metadata)
    Stats,
}

#[derive(Subcommand, Debug)]
enum McpCommands {
    /// Print MCP configuration JSON for manual setup
    Config {
        /// Output style: json (raw JSON only), snippet (with instructions)
        #[arg(long, short = 'o', default_value = "snippet")]
        output: McpConfigFormat,
    },
    /// Install gabb MCP server into Claude Desktop/Code configuration
    Install {
        /// Only install for Claude Desktop
        #[arg(long)]
        claude_desktop: bool,
        /// Only install for Claude Code (project-level .claude/mcp.json)
        #[arg(long)]
        claude_code: bool,
    },
    /// Check MCP configuration status and optionally test server startup
    Status {
        /// Test MCP server startup (dry run)
        #[arg(long)]
        dry_run: bool,
    },
    /// Remove gabb from MCP configuration
    Uninstall {
        /// Only uninstall from Claude Desktop
        #[arg(long)]
        claude_desktop: bool,
        /// Only uninstall from Claude Code
        #[arg(long)]
        claude_code: bool,
    },
    /// Generate a slash command for Claude Code
    Command,
}

#[derive(Clone, Debug, Default, clap::ValueEnum)]
enum McpConfigFormat {
    /// Raw JSON output only (for piping/scripting)
    Json,
    /// JSON with setup instructions (default)
    #[default]
    Snippet,
}

#[derive(Subcommand, Debug)]
enum DaemonCommands {
    /// Start the indexing daemon
    Start {
        /// Delete and recreate index on start
        #[arg(long)]
        rebuild: bool,
        /// Run in background (daemonize)
        #[arg(long, short = 'b')]
        background: bool,
        /// Log file path (default: .gabb/daemon.log when backgrounded)
        #[arg(long)]
        log_file: Option<PathBuf>,
        /// Suppress progress output
        #[arg(long, short = 'q')]
        quiet: bool,
    },
    /// Stop a running daemon
    Stop {
        /// Force immediate shutdown (SIGKILL)
        #[arg(long)]
        force: bool,
    },
    /// Restart the daemon
    Restart {
        /// Force full reindex on restart
        #[arg(long)]
        rebuild: bool,
    },
    /// Show daemon status
    Status,
}

fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    init_logging(cli.verbose, cli.quiet);
    let format = cli.format;
    let quiet = cli.quiet;
    let daemon_opts = DaemonOptions {
        no_start_daemon: cli.no_start_daemon,
        no_daemon: cli.no_daemon,
    };

    // Resolve workspace and database paths
    let workspace = match workspace::resolve_workspace(cli.workspace.as_deref()) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::Error.into();
        }
    };
    let db = workspace::resolve_db_path(cli.db.as_deref(), &workspace);

    // Set environment variables for child processes
    workspace::set_env_for_children(&workspace, &db);

    let result = match cli.command {
        Commands::Daemon { command } => match command {
            DaemonCommands::Start {
                rebuild,
                background,
                log_file,
                quiet: daemon_quiet,
            } => daemon::start(
                &workspace,
                &db,
                rebuild,
                background,
                log_file.as_deref(),
                daemon_quiet || quiet,
            )
            .map(|_| ExitCode::Success),
            DaemonCommands::Stop { force } => {
                daemon::stop(&workspace, force).map(|_| ExitCode::Success)
            }
            DaemonCommands::Restart { rebuild } => {
                daemon::restart(&workspace, &db, rebuild).map(|_| ExitCode::Success)
            }
            DaemonCommands::Status => daemon::status(&workspace, format).map(|_| ExitCode::Success),
        },
        Commands::Symbols {
            file,
            kind,
            name,
            fuzzy,
            limit,
            offset,
            source,
            context,
        } => ensure_index_available(&db, &daemon_opts).and_then(|_| {
            let source_opts = SourceDisplayOptions {
                include_source: source,
                context_lines: context,
            };
            list_symbols(
                &db,
                file.as_ref(),
                kind.as_deref(),
                name.as_deref(),
                fuzzy,
                limit,
                offset,
                format,
                source_opts,
                quiet,
            )
        }),
        Commands::Implementation {
            file,
            line,
            character,
            limit,
            kind,
            source,
            context,
        } => ensure_index_available(&db, &daemon_opts).and_then(|_| {
            let source_opts = SourceDisplayOptions {
                include_source: source,
                context_lines: context,
            };
            find_implementation(
                &db,
                &file,
                line,
                character,
                limit,
                kind.as_deref(),
                format,
                source_opts,
                quiet,
            )
        }),
        Commands::Usages {
            file,
            line,
            character,
            limit,
            source,
            context,
        } => ensure_index_available(&db, &daemon_opts).and_then(|_| {
            let source_opts = SourceDisplayOptions {
                include_source: source,
                context_lines: context,
            };
            find_usages(
                &db,
                &file,
                line,
                character,
                limit,
                format,
                source_opts,
                quiet,
            )
        }),
        Commands::Symbol {
            name,
            file,
            kind,
            fuzzy,
            limit,
            source,
            context,
        } => ensure_index_available(&db, &daemon_opts).and_then(|_| {
            let source_opts = SourceDisplayOptions {
                include_source: source,
                context_lines: context,
            };
            show_symbol(
                &db,
                &name,
                file.as_ref(),
                kind.as_deref(),
                fuzzy,
                limit,
                format,
                source_opts,
                quiet,
            )
        }),
        Commands::Definition {
            file,
            line,
            character,
            source,
            context,
        } => ensure_index_available(&db, &daemon_opts).and_then(|_| {
            let source_opts = SourceDisplayOptions {
                include_source: source,
                context_lines: context,
            };
            find_definition(&db, &file, line, character, format, source_opts, quiet)
        }),
        Commands::Duplicates {
            uncommitted,
            staged,
            kind,
            min_count,
        } => ensure_index_available(&db, &daemon_opts).and_then(|_| {
            find_duplicates(
                &db,
                uncommitted,
                staged,
                kind.as_deref(),
                min_count,
                format,
                quiet,
            )
        }),
        Commands::McpServer => mcp::run_server(&workspace, &db).map(|_| ExitCode::Success),
        Commands::Mcp { command } => match command {
            McpCommands::Config { output } => {
                mcp_config(&workspace, output).map(|_| ExitCode::Success)
            }
            McpCommands::Install {
                claude_desktop,
                claude_code,
            } => mcp_install(&workspace, claude_desktop, claude_code).map(|_| ExitCode::Success),
            McpCommands::Status { dry_run } => {
                mcp_status(&workspace, &db, dry_run).map(|_| ExitCode::Success)
            }
            McpCommands::Uninstall {
                claude_desktop,
                claude_code,
            } => mcp_uninstall(claude_desktop, claude_code).map(|_| ExitCode::Success),
            McpCommands::Command => mcp_command(&workspace).map(|_| ExitCode::Success),
        },
        Commands::Init {
            mcp,
            gitignore,
            skill,
        } => init_project(&workspace, mcp, gitignore, skill).map(|_| ExitCode::Success),
        Commands::Setup { yes, dry_run } => {
            setup_wizard(&workspace, &db, yes, dry_run).map(|_| ExitCode::Success)
        }
        Commands::Includers {
            file,
            transitive,
            limit,
        } => ensure_index_available(&db, &daemon_opts)
            .and_then(|_| find_includers(&db, &file, transitive, limit, format, quiet)),
        Commands::Includes {
            file,
            transitive,
            limit,
        } => ensure_index_available(&db, &daemon_opts)
            .and_then(|_| find_includes(&db, &file, transitive, limit, format, quiet)),
        Commands::Structure {
            file,
            source,
            context,
        } => ensure_index_available(&db, &daemon_opts).and_then(|_| {
            let source_opts = SourceDisplayOptions {
                include_source: source,
                context_lines: context,
            };
            file_structure(&db, &workspace, &file, format, source_opts, quiet)
        }),
        Commands::Stats => ensure_index_available(&db, &daemon_opts)
            .and_then(|_| show_stats(&db, format).map(|_| ExitCode::Success)),
    };

    match result {
        Ok(code) => code.into(),
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::Error.into()
        }
    }
}

fn init_logging(verbosity: u8, quiet: bool) {
    let level = if quiet {
        "warn" // In quiet mode, only show warnings and errors
    } else {
        match verbosity {
            0 => "info",
            1 => "debug",
            _ => "trace",
        }
    };
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(level))
        .format_timestamp_secs()
        .init();
}

#[allow(clippy::too_many_arguments)]
fn list_symbols(
    db: &Path,
    file: Option<&PathBuf>,
    kind: Option<&str>,
    name: Option<&str>,
    fuzzy: bool,
    limit: Option<usize>,
    offset: Option<usize>,
    format: OutputFormat,
    source_opts: SourceDisplayOptions,
    _quiet: bool,
) -> Result<ExitCode> {
    let store = open_store_for_query(db)?;
    let file_str = file.map(|p| p.to_string_lossy().to_string());

    let mut symbols: Vec<SymbolRecord> = match (fuzzy, name) {
        (true, Some(query)) => {
            // Use FTS5 search for fuzzy matching
            let mut results = store.search_symbols_fts(query)?;

            // Apply additional filters
            if let Some(f) = &file_str {
                results.retain(|s| s.file == *f);
            }
            if let Some(k) = kind {
                results.retain(|s| s.kind == k);
            }
            // Apply offset
            if let Some(off) = offset {
                if off < results.len() {
                    results = results.into_iter().skip(off).collect();
                } else {
                    results.clear();
                }
            }
            if let Some(l) = limit {
                results.truncate(l);
            }
            results
        }
        _ => {
            let query = SymbolQuery {
                file: file_str.as_deref(),
                kind,
                name,
                limit,
                offset,
                ..Default::default()
            };
            store.list_symbols_filtered(&query)?
        }
    };

    // Sort by name for consistent output
    if fuzzy {
        symbols.sort_by(|a, b| a.name.cmp(&b.name));
    }

    let found = !symbols.is_empty();
    output_symbols(&symbols, format, source_opts)?;
    Ok(if found {
        ExitCode::Success
    } else {
        ExitCode::NotFound
    })
}

#[allow(clippy::too_many_arguments)]
fn find_implementation(
    db: &Path,
    file: &Path,
    line: Option<usize>,
    character: Option<usize>,
    limit: Option<usize>,
    kind: Option<&str>,
    format: OutputFormat,
    source_opts: SourceDisplayOptions,
    _quiet: bool,
) -> Result<ExitCode> {
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

    let found = !impl_symbols.is_empty();
    output_implementations(&target, &impl_symbols, format, source_opts)?;
    Ok(if found {
        ExitCode::Success
    } else {
        ExitCode::NotFound
    })
}

#[allow(clippy::too_many_arguments)]
fn find_usages(
    db: &Path,
    file: &Path,
    line: Option<usize>,
    character: Option<usize>,
    limit: Option<usize>,
    format: OutputFormat,
    source_opts: SourceDisplayOptions,
    _quiet: bool,
) -> Result<ExitCode> {
    let store = open_store_for_query(db)?;
    let (file, line, character) = parse_file_position(file, line, character)?;
    let target = resolve_symbol_at(&store, &file, line, character)?;
    let workspace_root = daemon::workspace_root_from_db(db).unwrap_or_else(|_| {
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

    let found = !refs.is_empty();
    output_usages(&target, &refs, &store, format, source_opts)?;
    Ok(if found {
        ExitCode::Success
    } else {
        ExitCode::NotFound
    })
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
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<String>,
    /// Import statement that brought the symbol into scope (if from different file)
    #[serde(skip_serializing_if = "Option::is_none")]
    import_via: Option<String>,
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
    store: &IndexStore,
    format: OutputFormat,
    source_opts: SourceDisplayOptions,
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

            // Look up import binding if this file is different from the target file
            let import_via = if file != &target.file {
                store
                    .get_import_binding(file, &target.file, &target.name)
                    .ok()
                    .flatten()
                    .map(|b| b.import_text)
            } else {
                None
            };

            let usages: Vec<UsageLocation> = file_refs
                .iter()
                .filter_map(|r| {
                    let (line, col) = offset_to_line_char_in_file(&r.file, r.start).ok()?;
                    let (end_line, end_col) = offset_to_line_char_in_file(&r.file, r.end).ok()?;

                    let source = if source_opts.include_source {
                        mcp::extract_source(&r.file, r.start, r.end, source_opts.context_lines)
                    } else {
                        None
                    };

                    Some(UsageLocation {
                        start: Position {
                            line,
                            character: col,
                        },
                        end: Position {
                            line: end_line,
                            character: end_col,
                        },
                        source,
                        import_via: import_via.clone(),
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
                    // Show import statement if present (only once per file)
                    if let Some(first_usage) = fu.usages.first() {
                        if let Some(import_via) = &first_usage.import_via {
                            println!("  via: {}", import_via.trim());
                        }
                    }
                    for u in &fu.usages {
                        println!(
                            "  {}:{}-{}:{}",
                            u.start.line, u.start.character, u.end.line, u.end.character
                        );
                        if let Some(src) = &u.source {
                            println!("{}", src);
                        }
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

#[allow(clippy::too_many_arguments)]
fn show_symbol(
    db: &Path,
    name: &str,
    file: Option<&PathBuf>,
    kind: Option<&str>,
    fuzzy: bool,
    limit: Option<usize>,
    format: OutputFormat,
    source_opts: SourceDisplayOptions,
    quiet: bool,
) -> Result<ExitCode> {
    let store = open_store_for_query(db)?;
    let file_str = file.map(|p| p.to_string_lossy().to_string());

    let symbols = if fuzzy {
        // Use FTS5 search for fuzzy matching
        let mut results = store.search_symbols_fts(name)?;

        // Apply additional filters
        if let Some(f) = &file_str {
            results.retain(|s| s.file == *f);
        }
        if let Some(k) = kind {
            results.retain(|s| s.kind == k);
        }
        if let Some(l) = limit {
            results.truncate(l);
        }
        results
    } else {
        store.list_symbols(file_str.as_deref(), kind, Some(name), limit)?
    };

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
                if !quiet {
                    println!("No symbols found for name '{}'.", name);
                }
            }
        }
        return Ok(ExitCode::NotFound);
    }

    let workspace_root = daemon::workspace_root_from_db(db)
        .unwrap_or_else(|_| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    // Build detailed output for each symbol
    let detailed_symbols: Vec<SymbolDetailOutput> = symbols
        .iter()
        .filter_map(|sym| {
            let base = SymbolOutput::from_record_with_source(sym, source_opts)?;
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
                if let Some(src) = &sym.base.source {
                    println!("{}\n", src);
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

    Ok(ExitCode::Success)
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
    source_opts: SourceDisplayOptions,
    _quiet: bool,
) -> Result<ExitCode> {
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

    let def_out = SymbolOutput::from_record_with_source(&definition, source_opts)
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
            if let Some(src) = &def_out.source {
                println!("{}", src);
            }
        }
    }

    // Definition always found if we reach here (errors are returned early)
    Ok(ExitCode::Success)
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
    quiet: bool,
) -> Result<ExitCode> {
    let store = open_store_for_query(db)?;
    let workspace_root = daemon::workspace_root_from_db(db)?;

    // Get file filter based on git flags
    let file_filter: Option<Vec<String>> = if uncommitted || staged {
        let files = get_git_changed_files(&workspace_root, uncommitted, staged)?;
        if files.is_empty() {
            output_empty_duplicates(format, quiet)?;
            return Ok(ExitCode::NotFound);
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

    let found = !group_outputs.is_empty();

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
                if !quiet {
                    println!("No duplicates found.");
                }
                return Ok(ExitCode::NotFound);
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

    Ok(if found {
        ExitCode::Success
    } else {
        ExitCode::NotFound
    })
}

fn output_empty_duplicates(format: OutputFormat, quiet: bool) -> Result<()> {
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
            if !quiet {
                println!("No changed files found.");
            }
        }
    }
    Ok(())
}

/// Find all files that include/import the given file (reverse dependency lookup).
fn find_includers(
    db: &Path,
    file: &Path,
    transitive: bool,
    limit: Option<usize>,
    format: OutputFormat,
    quiet: bool,
) -> Result<ExitCode> {
    let store = open_store_for_query(db)?;
    let file_str = normalize_path(file);

    let mut files: Vec<String> = if transitive {
        // Use get_invalidation_set which returns transitive reverse dependencies
        store.get_invalidation_set(&file_str)?
    } else {
        // Just direct dependents
        store.get_dependents(&file_str)?
    };

    // Remove the original file from transitive results
    files.retain(|f| f != &file_str);

    if let Some(lim) = limit {
        files.truncate(lim);
    }

    let found = !files.is_empty();
    output_file_list(&files, &file_str, "includers", transitive, format, quiet)?;
    Ok(if found {
        ExitCode::Success
    } else {
        ExitCode::NotFound
    })
}

/// Find all files that the given file includes/imports (forward dependency lookup).
fn find_includes(
    db: &Path,
    file: &Path,
    transitive: bool,
    limit: Option<usize>,
    format: OutputFormat,
    quiet: bool,
) -> Result<ExitCode> {
    let store = open_store_for_query(db)?;
    let file_str = normalize_path(file);

    let mut files: Vec<String> = if transitive {
        store.get_transitive_dependencies(&file_str)?
    } else {
        store
            .get_file_dependencies(&file_str)?
            .into_iter()
            .map(|d| d.to_file)
            .collect()
    };

    if let Some(lim) = limit {
        files.truncate(lim);
    }

    let found = !files.is_empty();
    output_file_list(&files, &file_str, "includes", transitive, format, quiet)?;
    Ok(if found {
        ExitCode::Success
    } else {
        ExitCode::NotFound
    })
}

/// Output a list of files in various formats.
fn output_file_list(
    files: &[String],
    source_file: &str,
    relation: &str,
    transitive: bool,
    format: OutputFormat,
    quiet: bool,
) -> Result<()> {
    #[derive(serde::Serialize)]
    struct FileListOutput {
        source_file: String,
        relation: String,
        transitive: bool,
        count: usize,
        files: Vec<String>,
    }

    let output = FileListOutput {
        source_file: source_file.to_string(),
        relation: relation.to_string(),
        transitive,
        count: files.len(),
        files: files.to_vec(),
    };

    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&output)?);
        }
        OutputFormat::Jsonl => {
            for file in files {
                println!("{}", serde_json::to_string(&file)?);
            }
        }
        OutputFormat::Csv => {
            let mut wtr = csv::Writer::from_writer(std::io::stdout());
            wtr.write_record(["file"])?;
            for file in files {
                wtr.write_record([file])?;
            }
            wtr.flush()?;
        }
        OutputFormat::Tsv => {
            println!("file");
            for file in files {
                println!("{file}");
            }
        }
        OutputFormat::Text => {
            let transitive_str = if transitive { " (transitive)" } else { "" };
            if files.is_empty() {
                if !quiet {
                    println!(
                        "No {} found for {}{}",
                        relation, source_file, transitive_str
                    );
                }
            } else {
                println!(
                    "Found {} {}{} for {}:\n",
                    files.len(),
                    relation,
                    transitive_str,
                    source_file
                );
                for file in files {
                    println!("  {file}");
                }
            }
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

fn offset_to_line_char_in_file(path: &str, offset: i64) -> Result<(usize, usize)> {
    offset_to_line_col_in_file(Path::new(path), offset as usize)
        .with_context(|| format!("failed to convert offset for {path}"))
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

// ==================== File Structure Command ====================

/// Build a hierarchical tree of symbols from flat records
fn build_symbol_tree(
    symbols: Vec<SymbolRecord>,
    source_opts: SourceDisplayOptions,
    file_path: &str,
) -> Result<Vec<SymbolNode>> {
    use std::collections::HashMap;

    // Convert each symbol to a node with resolved positions
    let mut nodes: Vec<(Option<String>, SymbolNode)> = Vec::new();

    for sym in &symbols {
        let (start_line, start_col) = offset_to_line_char_in_file(&sym.file, sym.start)?;
        let (end_line, end_col) = offset_to_line_char_in_file(&sym.file, sym.end)?;

        let source = if source_opts.include_source {
            mcp::extract_source(&sym.file, sym.start, sym.end, source_opts.context_lines)
        } else {
            None
        };

        // Determine context from file path OR inline test markers (#[cfg(test)], #[test])
        let context = if sym.is_test || is_test_file(file_path) {
            "test"
        } else {
            "prod"
        };

        let node = SymbolNode {
            name: sym.name.clone(),
            kind: sym.kind.clone(),
            context: context.to_string(),
            start: Position {
                line: start_line,
                character: start_col,
            },
            end: Position {
                line: end_line,
                character: end_col,
            },
            visibility: sym.visibility.clone(),
            children: Vec::new(),
            source,
        };

        nodes.push((sym.container.clone(), node));
    }

    // Group children by their container name
    let mut children_by_container: HashMap<String, Vec<SymbolNode>> = HashMap::new();
    let mut roots: Vec<SymbolNode> = Vec::new();

    for (container, node) in nodes {
        if let Some(container_name) = container {
            children_by_container
                .entry(container_name)
                .or_default()
                .push(node);
        } else {
            roots.push(node);
        }
    }

    // Recursively attach children to their parents
    fn attach_children(node: &mut SymbolNode, children_map: &mut HashMap<String, Vec<SymbolNode>>) {
        if let Some(children) = children_map.remove(&node.name) {
            node.children = children;
            for child in &mut node.children {
                attach_children(child, children_map);
            }
        }
    }

    for root in &mut roots {
        attach_children(root, &mut children_by_container);
    }

    // Any remaining orphans (container exists but parent wasn't found) become roots
    for (_, orphans) in children_by_container {
        roots.extend(orphans);
    }

    // Sort by start position
    roots.sort_by(|a, b| {
        a.start
            .line
            .cmp(&b.start.line)
            .then(a.start.character.cmp(&b.start.character))
    });

    Ok(roots)
}

/// Show the structure of a file (symbols with hierarchy and positions)
fn file_structure(
    db: &Path,
    workspace: &Path,
    file: &Path,
    format: OutputFormat,
    source_opts: SourceDisplayOptions,
    _quiet: bool,
) -> Result<ExitCode> {
    let store = open_store_for_query(db)?;

    // Resolve the file path
    let file_path = if file.is_absolute() {
        file.to_path_buf()
    } else {
        workspace.join(file)
    };
    let file_path = file_path
        .canonicalize()
        .unwrap_or_else(|_| file_path.clone());
    let file_str = normalize_path(&file_path);

    // Check if file exists
    if !file_path.exists() {
        bail!("File not found: {}", file_path.display());
    }

    // Query symbols for this file
    let query = SymbolQuery {
        file: Some(&file_str),
        ..Default::default()
    };
    let symbols: Vec<SymbolRecord> = store.list_symbols_filtered(&query)?;

    if symbols.is_empty() {
        bail!(
            "No symbols found in {}. Is it indexed? Run `gabb daemon start` to index.",
            file_str
        );
    }

    // Determine if this is a test file
    let context = if is_test_file(&file_str) {
        "test"
    } else {
        "prod"
    };

    // Build the hierarchical tree
    let tree = build_symbol_tree(symbols, source_opts, &file_str)?;

    let structure = FileStructure {
        file: file_str.clone(),
        context: context.to_string(),
        symbols: tree,
    };

    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&structure)?);
        }
        OutputFormat::Jsonl => {
            println!("{}", serde_json::to_string(&structure)?);
        }
        OutputFormat::Csv | OutputFormat::Tsv => {
            // Flatten the tree for CSV/TSV output
            fn flatten_nodes(nodes: &[SymbolNode], depth: usize, rows: &mut Vec<Vec<String>>) {
                for node in nodes {
                    let indent = "  ".repeat(depth);
                    rows.push(vec![
                        format!("{}{}", indent, node.name),
                        node.kind.clone(),
                        node.context.clone(),
                        format!("{}:{}", node.start.line, node.start.character),
                        format!("{}:{}", node.end.line, node.end.character),
                        node.visibility.clone().unwrap_or_default(),
                    ]);
                    flatten_nodes(&node.children, depth + 1, rows);
                }
            }

            let mut rows = Vec::new();
            flatten_nodes(&structure.symbols, 0, &mut rows);

            if matches!(format, OutputFormat::Csv) {
                let mut wtr = csv::Writer::from_writer(std::io::stdout());
                wtr.write_record(["name", "kind", "context", "start", "end", "visibility"])?;
                for row in rows {
                    wtr.write_record(&row)?;
                }
                wtr.flush()?;
            } else {
                println!("name\tkind\tcontext\tstart\tend\tvisibility");
                for row in rows {
                    println!("{}", row.join("\t"));
                }
            }
        }
        OutputFormat::Text => {
            println!("{} ({})", structure.file, structure.context);
            print_tree(&structure.symbols, "", true, source_opts.include_source);
        }
    }

    Ok(ExitCode::Success)
}

/// Print symbol tree with ASCII art indentation
fn print_tree(nodes: &[SymbolNode], prefix: &str, _is_last_group: bool, show_source: bool) {
    for (i, node) in nodes.iter().enumerate() {
        let is_last = i == nodes.len() - 1;
        let connector = if is_last { "└─" } else { "├─" };
        let position = format!(
            "[{}:{} - {}:{}]",
            node.start.line, node.start.character, node.end.line, node.end.character
        );
        let visibility = node
            .visibility
            .as_ref()
            .map(|v| format!(" ({})", v))
            .unwrap_or_default();
        let context_indicator = format!(" [{}]", node.context);

        println!(
            "{}{} {} {}{}{}  {}",
            prefix, connector, node.kind, node.name, visibility, context_indicator, position
        );

        if show_source {
            if let Some(src) = &node.source {
                let child_prefix = if is_last {
                    format!("{}   ", prefix)
                } else {
                    format!("{}│  ", prefix)
                };
                for line in src.lines() {
                    println!("{}    {}", child_prefix, line);
                }
                println!("{}    ", child_prefix);
            }
        }

        if !node.children.is_empty() {
            let child_prefix = if is_last {
                format!("{}   ", prefix)
            } else {
                format!("{}│  ", prefix)
            };
            print_tree(&node.children, &child_prefix, is_last, show_source);
        }
    }
}

// ==================== Stats Command ====================

fn show_stats(db: &Path, format: OutputFormat) -> Result<()> {
    let store = open_store_for_query(db)?;
    let stats = store.get_index_stats()?;

    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&stats)?);
        }
        OutputFormat::Jsonl => {
            println!("{}", serde_json::to_string(&stats)?);
        }
        OutputFormat::Csv | OutputFormat::Tsv => {
            // CSV/TSV format doesn't make sense for nested stats, output as JSON
            println!("{}", serde_json::to_string_pretty(&stats)?);
        }
        OutputFormat::Text => {
            println!("Index Statistics");
            println!("================");
            println!();

            println!("Files:");
            println!("  Total: {}", stats.files.total);
            if !stats.files.by_language.is_empty() {
                println!("  By language:");
                let mut langs: Vec<_> = stats.files.by_language.iter().collect();
                langs.sort_by(|a, b| b.1.cmp(a.1)); // Sort by count descending
                for (lang, count) in langs {
                    println!("    {}: {}", lang, count);
                }
            }
            println!();

            println!("Symbols:");
            println!("  Total: {}", stats.symbols.total);
            if !stats.symbols.by_kind.is_empty() {
                println!("  By kind:");
                let mut kinds: Vec<_> = stats.symbols.by_kind.iter().collect();
                kinds.sort_by(|a, b| b.1.cmp(a.1)); // Sort by count descending
                for (kind, count) in kinds {
                    println!("    {}: {}", kind, count);
                }
            }
            println!();

            println!("Index:");
            println!("  Size: {} bytes", stats.index.size_bytes);
            if let Some(updated) = &stats.index.last_updated {
                println!("  Last updated: {}", updated);
            }
            println!("  Schema version: {}", stats.index.schema_version);
            println!();

            if stats.errors.parse_failures > 0 {
                println!("Errors:");
                println!("  Parse failures: {}", stats.errors.parse_failures);
                if !stats.errors.failed_files.is_empty() {
                    println!("  Failed files:");
                    for file in &stats.errors.failed_files {
                        println!("    {}", file);
                    }
                }
            }
        }
    }

    Ok(())
}

// ==================== MCP Configuration Commands ====================

/// Get the path to Claude Desktop config file
fn claude_desktop_config_path() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        dirs::home_dir()
            .map(|h| h.join("Library/Application Support/Claude/claude_desktop_config.json"))
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
                "args": ["mcp-server", "--workspace", root_str]
            }
        }
    })
}

/// Print MCP configuration JSON
fn mcp_config(root: &Path, format: McpConfigFormat) -> Result<()> {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let config = generate_mcp_config(&root, true);

    match format {
        McpConfigFormat::Json => {
            // Raw JSON only - suitable for piping/scripting
            println!(
                "{}",
                serde_json::to_string_pretty(&config).unwrap_or_default()
            );
        }
        McpConfigFormat::Snippet => {
            // Friendly output with instructions
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
                println!(
                    "  macOS:   ~/Library/Application Support/Claude/claude_desktop_config.json"
                );
                println!("  Windows: %APPDATA%\\Claude\\claude_desktop_config.json");
                println!("  Linux:   ~/.config/Claude/claude_desktop_config.json");
            }

            println!();
            println!("Or run `gabb mcp install` to install automatically.");
        }
    }

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
fn mcp_status(workspace: &Path, db: &Path, dry_run: bool) -> Result<()> {
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

    // Dry-run: test MCP server startup
    if dry_run {
        println!();
        print!("MCP server test: ");

        match test_mcp_server_startup(workspace, db) {
            Ok(()) => {
                println!("✓ Server starts successfully");
            }
            Err(e) => {
                println!("✗ Server startup failed");
                println!("  Error: {}", e);
                return Err(e);
            }
        }
    }

    if !found_any && !dry_run {
        println!();
        println!("Run `gabb mcp install` to configure MCP for Claude.");
    }

    Ok(())
}

/// Test MCP server startup (dry run)
fn test_mcp_server_startup(workspace: &Path, db: &Path) -> Result<()> {
    use std::io::{BufRead, BufReader, Write};
    use std::process::{Command, Stdio};
    use std::sync::mpsc;
    use std::time::Duration;

    let gabb_binary = find_gabb_binary();

    // Spawn the MCP server process
    let mut child = Command::new(&gabb_binary)
        .args([
            "mcp-server",
            "--workspace",
            workspace.to_string_lossy().as_ref(),
            "--db",
            db.to_string_lossy().as_ref(),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("Failed to spawn MCP server: {}", gabb_binary))?;

    // Send an initialize request to test the server responds correctly
    let stdin = child.stdin.as_mut().context("Failed to get stdin")?;
    let initialize_request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {
                "name": "gabb-dry-run",
                "version": "1.0.0"
            }
        }
    });

    // Write the request as a single JSON line (gabb MCP server uses JSON lines protocol)
    let request_str = serde_json::to_string(&initialize_request)?;
    writeln!(stdin, "{}", request_str)?;
    stdin.flush()?;

    // Read response with timeout using a channel
    let stdout = child.stdout.take().context("Failed to get stdout")?;
    let (tx, rx) = mpsc::channel::<Result<serde_json::Value>>();

    let reader_thread = std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);

        // Read a single JSON line response
        let mut response_line = String::new();
        if reader.read_line(&mut response_line).is_err() {
            let _ = tx.send(Err(anyhow!("Failed to read response")));
            return;
        }

        if response_line.is_empty() {
            let _ = tx.send(Err(anyhow!("Empty response from server")));
            return;
        }

        match serde_json::from_str(&response_line) {
            Ok(response) => {
                let _ = tx.send(Ok(response));
            }
            Err(e) => {
                let _ = tx.send(Err(anyhow!(
                    "Failed to parse response JSON: {} (got: {})",
                    e,
                    response_line.trim()
                )));
            }
        }
    });

    // Wait for response with 5 second timeout
    let result = rx.recv_timeout(Duration::from_secs(5));

    // Kill the server process
    let _ = child.kill();
    let _ = child.wait();

    // Wait for reader thread to finish
    let _ = reader_thread.join();

    // Check result
    match result {
        Ok(Ok(response)) => {
            // Verify it's a valid initialize response
            if response.get("result").is_some() {
                Ok(())
            } else if let Some(error) = response.get("error") {
                Err(anyhow!("Server returned error: {}", error))
            } else {
                Err(anyhow!("Unexpected response format"))
            }
        }
        Ok(Err(e)) => Err(e),
        Err(_) => Err(anyhow!("Server did not respond within 5 seconds")),
    }
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

/// Generate a slash command file for Claude Code
fn mcp_command(root: &Path) -> Result<()> {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());

    // Create .claude/commands directory
    let commands_dir = root.join(".claude").join("commands");
    if !commands_dir.exists() {
        fs::create_dir_all(&commands_dir)?;
        println!("Created .claude/commands/");
    }

    let command_file = commands_dir.join("gabb.md");

    // Generate the slash command content
    let content = r#"---
description: Search code symbols with gabb
---
Use the gabb MCP tools to help with this code navigation request.

Available tools:
- gabb_symbols: List/search symbols (functions, classes, types)
- gabb_symbol: Get detailed information about a symbol by name
- gabb_definition: Go to where a symbol is defined
- gabb_usages: Find all references to a symbol
- gabb_implementations: Find implementations of interfaces/traits
- gabb_duplicates: Find duplicate code in the codebase
- gabb_structure: Get hierarchical file structure showing symbols with positions
- gabb_includers: Find all files that #include a header (C++ reverse dependency)
- gabb_includes: Find all headers included by a file (C++ forward dependency)
- gabb_daemon_status: Check if the indexing daemon is running
- gabb_stats: Get index statistics (files by language, symbols by kind, index size)

If the index doesn't exist, gabb will auto-start the daemon to build it.
"#;

    if command_file.exists() {
        println!("Slash command already exists at {}", command_file.display());
        println!("To overwrite, delete it first and run this command again.");
    } else {
        fs::write(&command_file, content)?;
        println!("Created slash command: {}", command_file.display());
        println!();
        println!("You can now use /gabb in Claude Code to invoke gabb tools.");
    }

    Ok(())
}

// ==================== Init Command ====================

/// Initialize gabb in a project
fn init_project(
    root: &Path,
    setup_mcp: bool,
    setup_gitignore: bool,
    setup_skill: bool,
) -> Result<()> {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());

    println!("Initializing gabb in {}", root.display());

    // Create .gabb directory
    let gabb_dir = root.join(".gabb");
    if !gabb_dir.exists() {
        fs::create_dir_all(&gabb_dir)?;
        println!("  Created .gabb/");
    } else {
        println!("  .gabb/ already exists");
    }

    // Setup MCP configuration if requested
    if setup_mcp {
        init_mcp_config(&root)?;
    }

    // Setup .gitignore if requested
    if setup_gitignore {
        init_gitignore(&root)?;
    }

    // Setup agent skill if requested
    if setup_skill {
        init_skill(&root)?;
    }

    println!();
    println!("Next steps:");
    println!("  1. Start the daemon:    gabb daemon start");
    if setup_mcp {
        println!("  2. Restart Claude Code to load the MCP server");
    } else if setup_skill {
        println!("  2. The skill will auto-activate when Claude Code sees relevant requests");
    } else {
        println!("  2. For AI integration: gabb init --mcp --skill");
    }

    Ok(())
}

/// Create .claude/mcp.json with gabb configuration
fn init_mcp_config(root: &Path) -> Result<()> {
    let claude_dir = root.join(".claude");
    let mcp_config_path = claude_dir.join("mcp.json");

    // Create .claude directory
    if !claude_dir.exists() {
        fs::create_dir_all(&claude_dir)?;
        println!("  Created .claude/");
    }

    // Generate MCP config with relative path (version control friendly)
    let config = serde_json::json!({
        "mcpServers": {
            "gabb": {
                "command": find_gabb_binary(),
                "args": ["mcp-server", "--workspace", "."]
            }
        }
    });

    if mcp_config_path.exists() {
        // Check if gabb already configured
        let existing = fs::read_to_string(&mcp_config_path)?;
        let existing_config: serde_json::Value = serde_json::from_str(&existing)?;
        if existing_config
            .get("mcpServers")
            .and_then(|s| s.get("gabb"))
            .is_some()
        {
            println!("  .claude/mcp.json already has gabb configured");
            return Ok(());
        }

        // Merge with existing config
        let mut merged: serde_json::Value = existing_config;
        let mcp_servers = merged
            .as_object_mut()
            .ok_or_else(|| anyhow!("Invalid mcp.json format"))?
            .entry("mcpServers")
            .or_insert_with(|| serde_json::json!({}));

        if let Some(servers) = mcp_servers.as_object_mut() {
            if let Some(gabb) = config
                .get("mcpServers")
                .and_then(|s| s.get("gabb"))
                .cloned()
            {
                servers.insert("gabb".to_string(), gabb);
            }
        }

        fs::write(&mcp_config_path, serde_json::to_string_pretty(&merged)?)?;
        println!("  Added gabb to .claude/mcp.json");
    } else {
        fs::write(&mcp_config_path, serde_json::to_string_pretty(&config)?)?;
        println!("  Created .claude/mcp.json");
    }

    Ok(())
}

/// Add .gabb/ and .claude/ to .gitignore
fn init_gitignore(root: &Path) -> Result<()> {
    let gitignore_path = root.join(".gitignore");
    let entries_to_add = vec![".gabb/", ".claude/"];

    let existing_content = if gitignore_path.exists() {
        fs::read_to_string(&gitignore_path)?
    } else {
        String::new()
    };

    let existing_lines: Vec<&str> = existing_content.lines().collect();
    let mut additions = Vec::new();

    for entry in &entries_to_add {
        // Check if entry already exists (exact match or with comment)
        let already_present = existing_lines.iter().any(|line| {
            let trimmed = line.trim();
            trimmed == *entry || trimmed == entry.trim_end_matches('/')
        });

        if !already_present {
            additions.push(*entry);
        }
    }

    if additions.is_empty() {
        println!("  .gitignore already configured");
        return Ok(());
    }

    // Append to .gitignore
    let mut content = existing_content;
    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }
    if !content.is_empty() {
        content.push_str("\n# gabb code indexing\n");
    } else {
        content.push_str("# gabb code indexing\n");
    }
    for entry in &additions {
        content.push_str(entry);
        content.push('\n');
    }

    fs::write(&gitignore_path, content)?;
    println!(
        "  Added {} to .gitignore",
        additions
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    );

    Ok(())
}

/// Create .claude/skills/gabb/ agent skill for Claude Code discoverability
fn init_skill(root: &Path) -> Result<()> {
    // Create .claude/skills/gabb directory
    let skills_dir = root.join(".claude").join("skills").join("gabb");
    if !skills_dir.exists() {
        fs::create_dir_all(&skills_dir)?;
        println!("  Created .claude/skills/gabb/");
    }

    let skill_file = skills_dir.join("SKILL.md");

    // The SKILL.md template - embedded at compile time from assets/SKILL.md
    // This teaches Claude when and how to use gabb's MCP tools
    let content = include_str!("../assets/SKILL.md");

    if skill_file.exists() {
        // Check if content differs from template
        let existing = fs::read_to_string(&skill_file)?;
        if existing == content {
            println!("  .claude/skills/gabb/SKILL.md is up to date");
            return Ok(());
        }
        // Update the file
        fs::write(&skill_file, content)?;
        println!("  Updated .claude/skills/gabb/SKILL.md");
        return Ok(());
    }

    fs::write(&skill_file, content)?;
    println!("  Created .claude/skills/gabb/SKILL.md");
    println!("  Claude will auto-discover this skill for code navigation tasks");

    Ok(())
}

// ==================== Setup Wizard ====================

/// Detect what kind of project this is based on marker files
fn detect_project_type(root: &Path) -> Option<&'static str> {
    let markers = [
        ("Cargo.toml", "Rust"),
        ("package.json", "Node.js"),
        ("pyproject.toml", "Python"),
        ("go.mod", "Go"),
        ("build.gradle", "Gradle"),
        ("build.gradle.kts", "Gradle (Kotlin)"),
        ("pom.xml", "Maven"),
        ("CMakeLists.txt", "CMake"),
        ("Makefile", "Make"),
    ];

    for (file, project_type) in markers {
        if root.join(file).exists() {
            return Some(project_type);
        }
    }
    None
}

/// Prompt user for yes/no confirmation
fn prompt_yes_no(prompt: &str, default_yes: bool) -> Result<bool> {
    let suffix = if default_yes { "[Y/n]" } else { "[y/N]" };
    print!("{} {} ", prompt, suffix);
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().lock().read_line(&mut input)?;
    let input = input.trim().to_lowercase();

    if input.is_empty() {
        return Ok(default_yes);
    }

    Ok(input == "y" || input == "yes")
}

/// Interactive setup wizard for one-command onboarding
fn setup_wizard(root: &Path, db: &Path, yes: bool, dry_run: bool) -> Result<()> {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let gabb_dir = root.join(".gabb");

    // Step 1: Detect and display workspace
    println!();
    let project_type = detect_project_type(&root);
    if let Some(ptype) = project_type {
        println!(
            "🔍 Detected workspace: {} ({} found)",
            root.display(),
            ptype
        );
    } else {
        println!("🔍 Detected workspace: {}", root.display());
    }

    // Step 2: Create .gabb directory
    let gabb_exists = gabb_dir.exists();
    if gabb_exists {
        println!("📁 .gabb/ already exists");
    } else if dry_run {
        println!("📁 Would create .gabb/");
    } else {
        fs::create_dir_all(&gabb_dir)?;
        println!("📁 Created .gabb/");
    }

    // Step 3: Offer to install MCP config for Claude Code
    let claude_dir = root.join(".claude");
    let mcp_config_path = claude_dir.join("mcp.json");
    let mcp_already_configured = if mcp_config_path.exists() {
        let content = fs::read_to_string(&mcp_config_path).unwrap_or_default();
        content.contains("\"gabb\"")
    } else {
        false
    };

    let install_mcp = if mcp_already_configured {
        println!("🔧 Claude Code MCP already configured");
        false
    } else {
        let should_install = yes || prompt_yes_no("🔧 Install MCP config for Claude Code?", true)?;
        if should_install {
            if dry_run {
                println!("   Would add gabb to .claude/mcp.json");
            } else {
                init_mcp_config(&root)?;
                println!("   ✅ Added gabb to Claude Code config");
            }
        }
        should_install
    };

    // Step 4: Offer to add skill file
    let skill_dir = root.join(".claude").join("skills").join("gabb");
    let skill_file = skill_dir.join("SKILL.md");
    let skill_exists = skill_file.exists();

    let install_skill = if skill_exists {
        println!("📚 Agent skill already exists");
        false
    } else {
        let should_install = yes || prompt_yes_no("📚 Create agent skill for Claude?", true)?;
        if should_install {
            if dry_run {
                println!("   Would create .claude/skills/gabb/SKILL.md");
            } else {
                init_skill(&root)?;
                println!("   ✅ Created agent skill");
            }
        }
        should_install
    };

    // Step 5: Offer to update .gitignore
    let gitignore_path = root.join(".gitignore");
    let gitignore_content = if gitignore_path.exists() {
        fs::read_to_string(&gitignore_path).unwrap_or_default()
    } else {
        String::new()
    };
    let gitignore_has_gabb = gitignore_content.lines().any(|line| {
        let trimmed = line.trim();
        trimmed == ".gabb/" || trimmed == ".gabb"
    });
    let gitignore_has_claude = gitignore_content.lines().any(|line| {
        let trimmed = line.trim();
        trimmed == ".claude/" || trimmed == ".claude"
    });

    if gitignore_has_gabb && gitignore_has_claude {
        println!("📝 .gitignore already configured");
    } else {
        let should_update =
            yes || prompt_yes_no("📝 Add .gabb/ and .claude/ to .gitignore?", true)?;
        if should_update {
            if dry_run {
                if !gitignore_has_gabb {
                    println!("   Would add .gabb/ to .gitignore");
                }
                if !gitignore_has_claude {
                    println!("   Would add .claude/ to .gitignore");
                }
            } else {
                init_gitignore(&root)?;
                println!("   ✅ Updated .gitignore");
            }
        }
    }

    // Step 6: Start daemon and run initial index
    if dry_run {
        println!("🚀 Would start daemon and run initial index");
    } else {
        println!("🚀 Starting daemon...");

        // Check if daemon is already running
        if let Ok(Some(pid_info)) = daemon::read_pid_file(&root) {
            if daemon::is_process_running(pid_info.pid) {
                println!("   Daemon already running (PID {})", pid_info.pid);
            } else {
                // Start daemon in foreground to show progress, but don't block
                daemon::start(&root, db, false, false, None, false)?;
            }
        } else {
            // Start daemon in foreground to show progress
            daemon::start(&root, db, false, false, None, false)?;
        }
    }

    // Step 7: Print success message
    println!();
    if dry_run {
        println!("Dry run complete. No changes were made.");
    } else {
        println!("Setup complete! Claude can now use gabb tools in this project.");
        if install_mcp || install_skill {
            println!("Restart Claude Code to load the new MCP server.");
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gabb_cli::indexer;
    use gabb_cli::offset_to_line_col;
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
        indexer::build_full_index(root, &store, None::<fn(&indexer::IndexProgress)>).unwrap();

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
        indexer::build_full_index(root, &store, None::<fn(&indexer::IndexProgress)>).unwrap();

        let offset = call_src.find("build_full_index").unwrap();
        let (line, character) = offset_to_line_col(call_src.as_bytes(), offset).unwrap();

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
        indexer::build_full_index(root, &store, None::<fn(&indexer::IndexProgress)>).unwrap();

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
        indexer::build_full_index(root, &store, None::<fn(&indexer::IndexProgress)>).unwrap();

        let def_path = def_path.canonicalize().unwrap();
        let src = fs::read_to_string(&def_path).unwrap();
        let offset = src.find("build_full_index").unwrap();
        let (line, character) = offset_to_line_col(src.as_bytes(), offset).unwrap();
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
