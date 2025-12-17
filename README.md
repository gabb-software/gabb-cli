# Gabb CLI

Gabb is a Rust CLI that builds a local code index so editors and AI coding assistants can answer questions like "where is this implemented?" without shipping your sources to a remote service. It includes an indexing daemon that stays in sync with filesystem changes.

## Status
- Indexes TypeScript/TSX, Rust, and Kotlin, storing results in a local SQLite database
- Commands: `gabb daemon start/stop/restart/status`, `gabb symbols`, `gabb symbol`, `gabb implementation`, `gabb usages`, `gabb definition`, `gabb duplicates`, `gabb mcp-server`
- Outputs: symbol definitions, relationships (implements/extends), and references
- MCP server for AI assistant integration (Claude Desktop, Claude Code)

## Quickstart
```bash
# 1) Install via Homebrew
brew install dmb23/tap/gabb

# 2) Initialize gabb with AI assistant integration
gabb init --mcp --skill

# 3) Start the daemon in background
gabb daemon start --background

# 4) Query the index
gabb symbols --kind function --limit 10
gabb symbol --name MyClass
gabb usages --file src/main.rs:10:5
```

The `gabb init --mcp --skill` command sets up:
- `.claude/mcp.json` - MCP server config for Claude Code
- `.claude/skills/gabb/SKILL.md` - Agent skill that teaches Claude to prefer gabb over grep

The daemon will crawl your workspace, index all supported files, and keep the SQLite database up to date as files change. Use `-v`/`-vv` to increase logging.

Query commands (symbols, usages, etc.) will auto-start the daemon if it's not running.

## Installation

### Homebrew (Recommended)

```bash
brew tap gabb-software/homebrew-tap
brew install gabb
```

This installs pre-built binaries for macOS (Intel and Apple Silicon) and Linux.

## Usage
```bash
gabb daemon start --root <workspace> --db <path/to/index.db> [--rebuild] [--background] [-v|-vv]
gabb daemon stop [--root <workspace>] [--force]
gabb daemon status [--root <workspace>]
gabb symbols --db <path/to/index.db> [--file <path>] [--kind <kind>] [--limit <n>]
gabb symbol --db <path/to/index.db> --name <name> [--file <path>] [--kind <kind>] [--limit <n>]
gabb implementation --db <path/to/index.db> --file <path[:line:char]> [--line <line>] [--character <char>] [--limit <n>] [--kind <kind>]
gabb usages --db <path/to/index.db> --file <path[:line:char]> [--line <line>] [--character <char>] [--limit <n>]
gabb mcp-server --root <workspace> --db <path/to/index.db>
```

Flags:
- `--root`: workspace to index (defaults to current directory)
- `--db`: SQLite database path (defaults to `.gabb/index.db`)
- `--rebuild`: delete any existing DB at `--db` and perform a full reindex before watching
- `-v`, `-vv`: increase log verbosity
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
| `gabb_duplicates` | Find duplicate symbol definitions |
| `gabb_daemon_status` | Check the status of the gabb indexing daemon |

---

### Claude Code

Claude Code supports three configuration scopes:
- **local** (default): Available only to you in the current project
- **project**: Shared with your team via `.mcp.json` at project root
- **user**: Available to you across all projects

#### Option 1: CLI Command (Recommended)

```bash
# Add for current project only (local scope)
claude mcp add gabb -- gabb mcp-server --root .

# Add globally for all projects (user scope)
claude mcp add gabb --scope user -- gabb mcp-server --root .

# Add as shared team config (project scope)
claude mcp add gabb --scope project -- gabb mcp-server --root .
```

#### Option 2: Edit Configuration File

**Local scope:** `~/.claude.json` or `.claude/settings.json` in your project
**Project scope:** `.mcp.json` at your project root (committed to git)

```json
{
  "mcpServers": {
    "gabb": {
      "command": "gabb",
      "args": ["mcp-server", "--root", "."]
    }
  }
}
```

Using `--root .` means gabb will use the current working directory as the workspace root.

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
      "args": ["mcp-server", "--root", "/path/to/your/project"]
    }
  }
}
```

Replace `/path/to/your/project` with the absolute path to your workspace. The MCP server will auto-start the daemon if needed.

---

### Codex CLI

Codex stores MCP configuration in `~/.codex/config.toml`.

#### Option 1: CLI Command (Recommended)

```bash
codex mcp add gabb -- gabb mcp-server --root /path/to/your/project
```

#### Option 2: Edit config.toml

Add to `~/.codex/config.toml`:

```toml
[mcp_servers.gabb]
command = "gabb"
args = ["mcp-server", "--root", "/path/to/your/project"]
```

Replace `/path/to/your/project` with your workspace path.

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
gabb mcp config

# Auto-install into Claude Desktop/Code configuration
gabb mcp install                    # Install to both Claude Desktop and Claude Code
gabb mcp install --claude-desktop   # Install to Claude Desktop only
gabb mcp install --claude-code      # Install to Claude Code only (creates .claude/mcp.json)

# Check current MCP configuration status
gabb mcp status

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
- `src/languages/`: language parsers (TypeScript, Rust, Kotlin) built on tree-sitter
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

## Contributing
Issues and PRs are welcome. Please:
- Keep commits focused and prefer Conventional Commits (`feat: ...`, `fix: ...`)
- Add or update tests when changing indexing behavior
- Run `cargo fmt`, `cargo clippy`, and `cargo test` before submitting
