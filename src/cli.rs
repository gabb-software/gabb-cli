//! CLI argument definitions using clap.
//!
//! This module defines the command-line interface schema for gabb.

use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

use gabb_cli::OutputFormat;

#[derive(Parser, Debug)]
#[command(name = "gabb", version, about = "Gabb CLI indexing daemon")]
pub struct Cli {
    /// Increase output verbosity (-v, -vv)
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    pub verbose: u8,

    /// Suppress non-essential output (for scripts). Errors still go to stderr.
    #[arg(short, long, global = true)]
    pub quiet: bool,

    /// Output format (text, json, jsonl, csv, tsv)
    #[arg(long, short = 'f', global = true, value_enum, default_value = "text")]
    pub format: OutputFormat,

    /// Workspace root (auto-detected from .gabb/, .git/, Cargo.toml, etc. if not specified)
    #[arg(long, short = 'w', global = true, env = "GABB_WORKSPACE")]
    pub workspace: Option<PathBuf>,

    /// Path to the SQLite index database (default: <workspace>/.gabb/index.db)
    #[arg(long, global = true, env = "GABB_DB")]
    pub db: Option<PathBuf>,

    /// Don't auto-start daemon if index doesn't exist
    #[arg(long, global = true)]
    pub no_start_daemon: bool,

    /// Suppress daemon-related warnings and status checks
    #[arg(long, global = true)]
    pub no_daemon: bool,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
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
        /// Skip the initial indexing step (only create config files)
        #[arg(long)]
        no_index: bool,
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
    },
    /// Show index statistics (file counts, symbol counts, index metadata)
    Stats,
}

#[derive(Subcommand, Debug)]
pub enum McpCommands {
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

#[derive(Clone, Debug, Default, ValueEnum)]
pub enum McpConfigFormat {
    /// Raw JSON output only (for piping/scripting)
    Json,
    /// JSON with setup instructions (default)
    #[default]
    Snippet,
}

#[derive(Subcommand, Debug)]
pub enum DaemonCommands {
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
