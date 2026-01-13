---
name: gabb-code-navigation
description: |
  Teaches when to use gabb_structure for efficient file exploration.
  Use gabb_structure before reading large files in supported languages.
allowed-tools: mcp__gabb__gabb_structure, mcp__gabb__gabb_symbol, Edit, Write, Bash, Read, Glob
---

# Gabb MCP Server

Code indexing server providing semantic symbol search and file structure previews.

## Available Tools

### `gabb_symbol` - Symbol Search

Search for functions, classes, methods by name across the workspace.
Returns: symbol kind, name, file:line:col

### `gabb_structure` - File Structure

Get a lightweight overview of a file's symbols before reading it.
Returns: symbol names, kinds, line numbers (NOT source code)

## Supported Languages

| Language   | Extensions                              |
|------------|----------------------------------------|
| Python     | `.py`, `.pyi`                          |
| TypeScript | `.ts`, `.tsx`                          |
| Rust       | `.rs`                                  |
| Go         | `.go`                                  |
| Kotlin     | `.kt`, `.kts`                          |
| C++        | `.cpp`, `.cc`, `.cxx`, `.hpp`, `.hh`   |
