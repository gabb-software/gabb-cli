# Gabb MCP Server

Code indexing server providing fast, indexed access to code symbols via SQLite. Enables O(1) symbol lookups instead of file scanning.

## Tool Selection Within Gabb

Choose the right gabb tool for your task:

| Task | Tool | Notes |
|------|------|-------|
| Get file overview before reading | `gabb_structure` | Cheap, returns symbol names and line numbers only |
| Find symbol by exact name | `gabb_symbol` | Fast lookup when you know the name |
| Search symbols with filters | `gabb_symbols` | Supports kind, file, namespace, pattern filters |
| Jump from usage to definition | `gabb_definition` | Point at a usage, get the definition location |
| Find all usages of a symbol | `gabb_usages` | Essential before refactoring |
| Get rename edit locations | `gabb_rename` | Returns edit-ready JSON for all rename sites |
| Trace who calls a function | `gabb_callers` | Understand impact and call flow |
| Trace what a function calls | `gabb_callees` | Understand dependencies |
| Find interface implementations | `gabb_implementations` | Find concrete types |
| Find parent types | `gabb_supertypes` | Understand inheritance |
| Find child types | `gabb_subtypes` | Find derived types |
| Find duplicate code | `gabb_duplicates` | Identify refactoring opportunities |
| Check server health | `gabb_daemon_status` | Verify index is available |
| Get index statistics | `gabb_stats` | File counts, symbol counts, languages |

## Context Management

**Pagination**: Most tools support `limit` (default: 50) and `offset` parameters. Use these to control result size:
```
gabb_symbols kind="function" limit=20 offset=0
```

**Token efficiency**: The `gabb_structure` tool returns only symbol names and line numbers—no source code. Use it first, then fetch source selectively:
- `gabb_structure` + targeted `Read` with offset/limit (most efficient)
- `gabb_symbols include_source=true` only when you need source for multiple symbols

**Source context**: When using `include_source=true`, you can also set `context_lines=N` to include surrounding lines (like grep -C).

## Common Workflows

### Exploring an unfamiliar file
1. `gabb_structure file="src/large_file.rs"` — get symbol overview with line numbers
2. Identify symbols of interest from the output
3. `Read file_path="src/large_file.rs" offset=N limit=M` — read specific sections

### Finding where a symbol is defined
1. `gabb_symbol name="MyFunction"` — if you know the exact name
2. `gabb_symbols name_contains="Func" kind="function"` — if you need to search

### Before refactoring a symbol
1. `gabb_usages file="src/foo.rs" line=42 character=10` — find all references
2. Review each usage to understand impact
3. `gabb_rename file="src/foo.rs" line=42 character=10 new_name="NewName"` — get edit locations

### Understanding call flow
1. `gabb_callers file="src/foo.rs" line=42 character=10` — who calls this?
2. `gabb_callees file="src/foo.rs" line=42 character=10` — what does this call?
3. Add `transitive=true` to trace the full call chain

### Finding implementations of an interface
1. Point to the interface/trait definition
2. `gabb_implementations file="src/traits.rs" line=10 character=5`
3. Or use `gabb_subtypes` for class hierarchies

## Error Handling

| Error | Cause | Resolution |
|-------|-------|------------|
| "No index found" or empty results | Daemon not running or index missing | Run `gabb_daemon_status` to check; start daemon if needed |
| Stale results | Index out of date | Daemon should auto-update; restart if persistent |
| "File not in index" | Unsupported language or file excluded | Check supported languages in skill documentation |
| Position-based query returns nothing | Wrong line/character coordinates | Coordinates are 1-based; verify position |

**Health check**: Use `gabb_daemon_status` to verify:
- Daemon is running
- Index location is correct
- Version matches expectations

**Statistics**: Use `gabb_stats` to see:
- Number of indexed files by language
- Number of symbols by kind
- Index size and last update time
