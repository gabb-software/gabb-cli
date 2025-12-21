---
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

If the index doesn't exist, gabb will auto-start the daemon to build it.
