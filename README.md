# Gabb CLI

Gabb is a Rust CLI that builds a local code index so editors and AI coding assistants can answer questions like "where is this implemented?" without shipping your sources to a remote service. It includes an indexing daemon that stays in sync with filesystem changes.

## Status
- Indexes TypeScript/TSX, Rust, Kotlin, C++, and Python, storing results in a local SQLite database
- Commands: `gabb setup`, `gabb init`, `gabb daemon start/stop/restart/status`, `gabb symbols`, `gabb symbol`, `gabb implementation`, `gabb usages`, `gabb definition`, `gabb duplicates`, `gabb structure`, `gabb stats`, `gabb includers`, `gabb includes`, `gabb mcp-server`
- Outputs: symbol definitions, relationships (implements/extends), and references
- MCP server for AI assistant integration (Claude Desktop, Claude Code)

## Quickstart
```bash
# 1) Install via Homebrew
brew install dmb23/tap/gabb

# 2) Run the interactive setup wizard
gabb setup

# 3) Query the index
gabb symbols --kind function --limit 10
gabb symbol --name MyClass
gabb usages --file src/main.rs:10:5
```

The `gabb setup` command is an interactive wizard that:
1. Detects your workspace and project type
2. Creates the `.gabb/` directory
3. Offers to install MCP config (`.claude/mcp.json`) for Claude Code
4. Offers to create an agent skill (`.claude/skills/gabb/SKILL.md`)
5. Updates `.gitignore` to exclude generated files
6. Runs the initial index and displays statistics

Use `gabb setup --yes` for non-interactive mode, `gabb setup --dry-run` to preview what would happen, or `gabb setup --no-index` to only create config files without indexing.

To keep the index updated as files change, start the daemon in background mode:
```bash
gabb daemon start --background
```

The daemon will watch your workspace and keep the SQLite database up to date. Use `-v`/`-vv` to increase logging.

Query commands (symbols, usages, etc.) will auto-start the daemon if it's not running.

## When to Use Gabb vs Read/Grep

Gabb provides **semantic** code understanding, not just text search. Here's when to use it:

| Goal | Use Gabb | Why |
|------|----------|-----|
| Find a symbol definition | `gabb symbol --name MyType` | Instant O(1) index lookup vs scanning files |
| Understand a file's structure | `gabb structure src/main.rs` | Get outline without reading 1000+ lines |
| Find all usages of a symbol | `gabb usages --file path:line:col` | Semantic refs only, no false matches in comments |
| See what calls a function | `gabb callers --file path:line:col` | Follows actual call graph |
| See what a function calls | `gabb callees --file path:line:col` | Trace execution flow forward |
| Find trait implementations | `gabb implementation --file path:line:col` | Understands type relationships |
| Safe rename refactoring | `gabb rename --file path:line:col --new-name X` | Gets all locations that need updating |

**Rule of thumb**: If you're looking for code structure or symbol information, gabb is almost always faster and more accurate than text search.

### Performance Benefits

| Operation | Gabb | Text Search |
|-----------|------|-------------|
| Find symbol in 2000-line file | ~1ms (index lookup) | ~100ms+ (read & scan) |
| Find all usages across codebase | ~5ms (indexed refs) | Seconds (grep entire tree) |
| Token cost (AI assistants) | ~50 tokens (structured result) | ~40,000 tokens (full file read) |

## Example Workflows

### Understanding a New Codebase
```bash
# 1. See language breakdown and symbol counts
gabb stats

# 2. Find key abstractions (traits, interfaces)
gabb symbols --kind trait
gabb symbols --kind interface

# 3. Understand entry points
gabb structure src/main.rs
```

### Implementing a Feature Using Existing Types
```bash
# 1. Find the type you need to use
gabb symbol --name ExistingType

# 2. See the file structure around it
gabb structure src/types.rs

# 3. Find how others use it
gabb usages --file src/types.rs:42:1
```

### Safe Refactoring
```bash
# 1. Find all usages before changing anything
gabb usages --file src/api.rs:100:5

# 2. Check what implements the interface you're changing
gabb implementation --file src/traits.rs:25:1

# 3. Get rename locations (then apply with your editor)
gabb rename --file src/api.rs:100:5 --new-name betterName
```

### Tracing Execution Flow
```bash
# Who calls this function? (trace backwards)
gabb callers --file src/auth.rs:50:1

# What does this function call? (trace forwards)
gabb callees --file src/auth.rs:50:1

# Full call chain (recursive)
gabb callers --file src/auth.rs:50:1 --transitive
```

## Installation

### Homebrew (macOS/Linux)

```bash
brew tap gabb-software/homebrew-tap
brew install gabb
```

This installs pre-built binaries for macOS (Intel and Apple Silicon) and Linux.

### Windows

#### Scoop (Recommended)

```powershell
# Add the gabb bucket
scoop bucket add gabb https://github.com/gabb-software/scoop-bucket

# Install gabb
scoop install gabb
```

#### Manual Download

Download the latest Windows binary from [GitHub Releases](https://github.com/gabb-software/gabb-cli/releases):

1. Download `gabb-x86_64-pc-windows-msvc.zip`
2. Extract to a directory (e.g., `C:\Program Files\gabb`)
3. Add the directory to your PATH

### Cargo (Rust)

```bash
# Install from source
cargo install gabb-cli

# Or with pre-built binaries (faster)
cargo binstall gabb-cli
```

Requires Rust 1.70+. The `cargo binstall` option downloads pre-built binaries instead of compiling from source. Works on Windows, macOS, and Linux.

## Usage
```bash
# Workspace is auto-detected from .gabb/, .git/, Cargo.toml, package.json, etc.
gabb setup [--yes] [--dry-run] [--no-index]
gabb init [--mcp] [--skill] [--gitignore]
gabb daemon start [--rebuild] [--background] [-v|-vv]
gabb daemon stop [--force]
gabb daemon status
gabb symbols [--file <path>] [--kind <kind>] [--limit <n>]
gabb symbol --name <name> [--file <path>] [--kind <kind>] [--limit <n>]
gabb implementation --file <path[:line:char]> [--line <line>] [--character <char>] [--limit <n>] [--kind <kind>]
gabb usages --file <path[:line:char]> [--line <line>] [--character <char>] [--limit <n>]
gabb structure <file> [--source] [-C <n>]
gabb mcp-server
```

### Workspace Auto-Discovery

Gabb automatically detects your workspace root by walking up from the current directory and looking for these markers (in priority order):
1. `.gabb/` - Explicit gabb workspace
2. `.git/` - Git repository root
3. Build files: `Cargo.toml`, `package.json`, `pyproject.toml`, `go.mod`, `build.gradle`, `pom.xml`, etc.

### Global Flags
- `--workspace`, `-w`: Explicit workspace root (overrides auto-detection)
- `--db`: SQLite database path (default: `<workspace>/.gabb/index.db`)
- `-v`, `-vv`: Increase log verbosity

### Environment Variables
- `GABB_WORKSPACE`: Set workspace root (lower priority than `--workspace` flag)
- `GABB_DB`: Set database path (lower priority than `--db` flag)

### Daemon Flags
- `--rebuild`: Delete existing DB and perform a full reindex
- `--background`, `-b`: Run daemon in background
Symbols command filters:
- `--file`: only show symbols from a given file path
- `--kind`: filter by kind (`function`, `class`, `interface`, `method`, `struct`, `enum`, `trait`)
- `--limit`: cap the number of rows returned
Implementation command:
- Identify the symbol via `--file` and `--line`/`--character` or embed the position as `--file path:line:char`
- Finds implementations via recorded edges (implements/extends/trait impl/overrides); falls back to same-name matches

Usages command:
- Identify the symbol under the cursor (same options as above)
- Lists recorded references from the index; if none are present (e.g., cross-file Rust calls not yet linked), falls back to a best-effort name scan across all indexed files in the workspace root
- Skips matches that overlap the symbol’s own definition span

Symbol command:
- Look up symbols by exact name (optional file/kind filters)
- Shows definition location (line/col), qualifier, visibility, container, incoming/outgoing edges, and recorded references for each match

Structure command:
- Show hierarchical structure of all symbols in a file
- **Summary stats**: symbol counts by kind, total line count
- **Key types**: highlights important public types with many methods
- Displays symbols nested by containment (e.g., methods inside classes)
- Shows start/end positions for each symbol
- Indicates whether file is test or production code

What gets indexed:
- Files: `*.ts`, `*.tsx`, `*.rs`, `*.kt`, `*.kts`
- Data stored: symbols (functions, classes, interfaces, methods, etc.), relationships (implements/extends), references
- Storage: SQLite with WAL enabled for safe concurrent reads

## MCP Server (AI Assistant Integration)

Gabb includes an MCP (Model Context Protocol) server that exposes code indexing tools to AI assistants. This allows AI coding tools to search symbols, find definitions, usages, and implementations in your codebase.

### Available Tools

| Tool | Description |
|------|-------------|
| `gabb_symbols` | List or search symbols in the codebase |
| `gabb_symbol` | Get detailed information about a symbol by name |
| `gabb_definition` | Go to definition for a symbol at a source position |
| `gabb_usages` | Find all usages/references of a symbol |
| `gabb_implementations` | Find implementations of an interface, trait, or abstract class |
| `gabb_supertypes` | Find parent types (superclasses, implemented interfaces/traits) of a type |
| `gabb_subtypes` | Find child types (subclasses, implementors) of a type/interface/trait |
| `gabb_callers` | Find all functions/methods that call a given function (call graph: who calls me?) |
| `gabb_callees` | Find all functions/methods called by a given function (call graph: what do I call?) |
| `gabb_rename` | Get all locations to update when renaming a symbol (edit-ready output) |
| `gabb_duplicates` | Find duplicate symbol definitions |
| `gabb_structure` | Cheap file overview with summary stats, key types, and symbol hierarchy (no source code) |
| `gabb_includers` | Find all files that #include a header (reverse dependency lookup) |
| `gabb_includes` | Find all headers included by a file (forward dependency lookup) |
| `gabb_daemon_status` | Check the status of the gabb indexing daemon |
| `gabb_stats` | Get comprehensive index statistics (files by language, symbols by kind, index size) |

#### `gabb_symbols` Parameters

| Parameter | Description |
|-----------|-------------|
| `name` | Exact symbol name match |
| `name_pattern` | Glob-style pattern (e.g., `get*`, `*Handler`, `*User*`) |
| `name_contains` | Substring match (e.g., `User` matches `getUser`, `UserService`) |
| `case_insensitive` | Make name matching case-insensitive (default: false) |
| `kind` | Filter by symbol kind: `function`, `class`, `interface`, `type`, `struct`, `enum`, `trait`, `method`, `const`, `variable` |
| `file` | Filter by path: exact file (`src/main.ts`), directory (`src/` or `src/components`), or glob (`src/**/*.ts`) |
| `namespace` | Filter by namespace/qualifier prefix (e.g., `std::collections`, `myapp::services`). Supports glob patterns (e.g., `std::*`) |
| `scope` | Filter by containing scope/container (e.g., `MyClass` to find methods within MyClass) |
| `limit` | Maximum results (default: 50) |
| `include_source` | Include the symbol's source code in output |
| `context_lines` | Lines before/after the symbol (like `grep -C`), requires `include_source` |

---

### Claude Code

Claude Code supports three configuration scopes:
- **local** (default): Available only to you in the current project
- **project**: Shared with your team via `.mcp.json` at project root
- **user**: Available to you across all projects

#### Option 1: CLI Command (Recommended)

```bash
# Add for current project only (local scope)
claude mcp add gabb -- gabb mcp-server

# Add globally for all projects (user scope)
claude mcp add gabb --scope user -- gabb mcp-server

# Add as shared team config (project scope)
claude mcp add gabb --scope project -- gabb mcp-server
```

#### Option 2: Edit Configuration File

**Local scope:** `~/.claude.json` or `.claude/settings.json` in your project
**Project scope:** `.mcp.json` at your project root (committed to git)

```json
{
  "mcpServers": {
    "gabb": {
      "command": "gabb",
      "args": ["mcp-server"]
    }
  }
}
```

The MCP server auto-detects the workspace root from the current directory.

#### Verify Installation

```bash
claude mcp list        # List configured servers
claude mcp get gabb    # Test the gabb server
```

---

### Claude Desktop

Add the following to your Claude Desktop configuration file:

**macOS:** `~/Library/Application Support/Claude/claude_desktop_config.json`
**Windows:** `%APPDATA%\Claude\claude_desktop_config.json`
**Linux:** `~/.config/Claude/claude_desktop_config.json`

```json
{
  "mcpServers": {
    "gabb": {
      "command": "gabb",
      "args": ["mcp-server", "--workspace", "/path/to/your/project"]
    }
  }
}
```

Replace `/path/to/your/project` with the absolute path to your workspace. You can also use the `GABB_WORKSPACE` environment variable. The MCP server will auto-start the daemon if needed.

---

### Codex CLI

Codex stores MCP configuration in `~/.codex/config.toml`.

#### Option 1: CLI Command (Recommended)

```bash
codex mcp add gabb -- gabb mcp-server --workspace /path/to/your/project
```

#### Option 2: Edit config.toml

Add to `~/.codex/config.toml`:

```toml
[mcp_servers.gabb]
command = "gabb"
args = ["mcp-server", "--workspace", "/path/to/your/project"]
```

Replace `/path/to/your/project` with your workspace path. You can also use the `GABB_WORKSPACE` environment variable.

#### Verify Installation

```bash
codex mcp list         # List configured servers
```

---

### Notes

- The MCP server automatically starts the gabb daemon if it's not already running
- All tools work with the index database at `.gabb/index.db` relative to the workspace root
- Use absolute paths for Claude Desktop and Codex; use `.` for Claude Code (it runs from your project directory)

---

### MCP Configuration Commands

Gabb provides helper commands to simplify MCP setup:

```bash
# Print MCP config JSON for manual setup
gabb mcp config                     # With setup instructions (default)
gabb mcp config --output json       # Raw JSON only (for piping/scripting)

# Auto-install into Claude Desktop/Code configuration
gabb mcp install                    # Install to both Claude Desktop and Claude Code
gabb mcp install --claude-desktop   # Install to Claude Desktop only
gabb mcp install --claude-code      # Install to Claude Code only (creates .claude/mcp.json)

# Check current MCP configuration status
gabb mcp status                     # Show configuration status
gabb mcp status --dry-run           # Also test MCP server startup

# Remove gabb from MCP configuration
gabb mcp uninstall

# Generate a slash command for Claude Code
gabb mcp command                    # Creates .claude/commands/gabb.md
```

The `gabb mcp command` creates a `/gabb` slash command in your project that helps Claude Code discover and use gabb's MCP tools for code navigation.

---

### Agent Skill for Discoverability

In addition to the MCP server, gabb can generate an **Agent Skill** that teaches Claude when to prefer gabb tools over grep/ripgrep for code navigation:

```bash
# Create the skill during project initialization
gabb init --skill

# Or create skill alongside MCP config
gabb init --mcp --skill
```

This creates `.claude/skills/gabb/SKILL.md` which Claude auto-discovers. The skill:
- Guides Claude to use `gabb_symbols` instead of grep for finding definitions
- Recommends `gabb_usages` for refactoring impact analysis
- Explains when each gabb tool is the best choice

**Skills vs MCP**: The MCP server provides the actual tools. The skill provides guidance on *when* to use them. Both complement each other for optimal AI assistant integration.

## Project Layout
- `src/main.rs`: CLI entrypoint and logging setup
- `src/daemon.rs`: filesystem watcher and incremental indexing loop
- `src/indexer.rs`: full/index-one routines and workspace traversal
- `src/languages/`: language parsers (TypeScript, Rust, Kotlin, C++) built on tree-sitter
- `src/store.rs`: SQLite-backed index store
- `src/mcp.rs`: MCP server implementation for AI assistant integration
- `ARCHITECTURE.md`: deeper design notes

## Development

### Building from Source

Prerequisite: Rust toolchain (Edition 2024). Install via [rustup](https://rustup.rs/).

```bash
# Clone the repository
git clone https://github.com/dmb23/gabb-cli.git
cd gabb-cli

# Install locally
cargo install --path .

# Or build without installing
cargo build
```

### Commands

- Format and lint: `cargo fmt && cargo clippy --all-targets --all-features`
- Tests: `cargo test`
- Docs: `cargo doc --open`

## Roadmap
- Additional commands (find implementations/usages) backed by the stored relationships
- More languages by swapping in new tree-sitter grammars
- Richer queries (duplicates, unused code) atop the same index

## Benchmarking

The `benchmark/` directory contains a suite for evaluating gabb's performance against traditional tools (grep/find/read) for code navigation tasks.

### Hypothesis

Agents using gabb's semantic indexing will find relevant source code files **faster** and with **less token overhead** than agents using traditional text-search tools.

### Running the Benchmark

```bash
cd benchmark

# 1. Setup (builds gabb for Linux/Docker, installs deps)
python setup.py

# 2. Configure API key
cp .env.example .env
# Edit .env and set ANTHROPIC_API_KEY=your-key-here

# 3. Run a single task
python run.py --task scikit-learn__scikit-learn-10297

# 4. Run multiple tasks in parallel
python run.py --tasks 20 --concurrent 5
```

See [`benchmark/README.md`](benchmark/README.md) for detailed documentation.

## Contributing
Issues and PRs are welcome. Please:
- Keep commits focused and prefer Conventional Commits (`feat: ...`, `fix: ...`)
- Add or update tests when changing indexing behavior
- Run `cargo fmt`, `cargo clippy`, and `cargo test` before submitting
