---
name: gabb-code-navigation
description: |
  Use gabb MCP tools for code navigation. Prefer gabb_symbols, gabb_usages,
  gabb_definition over grep/ripgrep when finding symbol definitions, usages,
  and implementations. Gabb understands code structure, not just text patterns.
---

# Code Navigation with gabb

This project uses gabb for fast, accurate code navigation. The gabb MCP tools
provide precise file:line:column locations and understand code structure.

## When to Use gabb Instead of grep/rg

| Task | gabb Tool | Why Better |
|------|-----------|------------|
| Find where something is defined | `gabb_symbol`, `gabb_definition` | Precise location, not text match |
| Find all usages of a symbol | `gabb_usages` | Understands imports, avoids false matches |
| Find interface implementations | `gabb_implementations` | Follows type relationships |
| Find parent types (inheritance) | `gabb_supertypes` | Navigates extends/implements edges |
| Find child types (subclasses) | `gabb_subtypes` | Finds all types that inherit from X |
| Find who calls a function | `gabb_callers` | Call graph: trace backwards through callers |
| Find what a function calls | `gabb_callees` | Call graph: trace forwards through callees |
| Rename a symbol | `gabb_rename` | Returns edit-ready JSON for all locations |
| Explore codebase structure | `gabb_symbols` | Filter by kind (function, class, etc.) |
| Find duplicate code | `gabb_duplicates` | Content-aware, not text search |
| View file structure | `gabb_structure` | Hierarchical symbol tree with positions |
| Find C++ header dependencies | `gabb_includers`, `gabb_includes` | Tracks #include relationships |

## Available MCP Tools

- **gabb_symbols**: Search for symbols by name, kind, or file. Supports:
  - `name`: Exact match when you know the name
  - `name_pattern`: Glob patterns like `get*`, `*Handler`, `*User*`
  - `name_contains`: Substring search, e.g., `User` finds `getUser`, `UserService`
  - `case_insensitive`: Set to true for case-insensitive matching
  - `file`: Filter by exact path, directory (`src/`), or glob (`src/**/*.ts`)
  - `namespace`: Filter by namespace/qualifier prefix (e.g., `std::collections`, `std::*`)
  - `scope`: Filter by containing scope (e.g., `MyClass` to find methods within it)
  - `include_source`: Include the symbol's source code in output
  - `context_lines`: Lines before/after (like grep -C), use with `include_source`
- **gabb_symbol**: Get details for a specific symbol by exact name.
- **gabb_definition**: Jump to definition from a usage location (file:line:col).
- **gabb_usages**: Find all references to a symbol before refactoring. Use `format: "refactor"` for edit-ready JSON output.
- **gabb_implementations**: Find classes/structs implementing an interface/trait.
- **gabb_rename**: Get all locations to update when renaming a symbol. Returns JSON with `old_text`, `new_text`, and exact positions for Edit tool.
- **gabb_supertypes**: Find parent types (superclasses, interfaces) of a type. Use `transitive: true` for full hierarchy.
- **gabb_subtypes**: Find child types (subclasses, implementors) of a type. Use `transitive: true` for full hierarchy.
- **gabb_callers**: Find all functions/methods that call a given function. Use `transitive: true` for full call chain.
- **gabb_callees**: Find all functions/methods called by a given function. Use `transitive: true` for full call chain.
- **gabb_duplicates**: Find copy-paste code for refactoring opportunities.
- **gabb_structure**: Get hierarchical file structure showing all symbols with positions.
- **gabb_includers**: Find all files that #include a header (C++ reverse dependency).
- **gabb_includes**: Find all headers included by a file (C++ forward dependency).
- **gabb_daemon_status**: Check if the indexing daemon is running.
- **gabb_stats**: Get comprehensive index statistics (files by language, symbols by kind, index size).

## Tips

- The daemon auto-starts when needed - no manual setup required
- Results include precise locations in `file:line:column` format
- Use `--kind` filters to narrow symbol searches (function, class, interface, etc.)
- The index updates automatically when files change
