---
name: gabb-code-navigation
description: |
  Decision guide for code navigation. Teaches when to use gabb tools vs default
  tools (Grep/Read) for maximum efficiency. Applies to Python/TypeScript/Rust/Kotlin/C++.
allowed-tools: mcp__gabb__*, Edit, Write, Bash, Read, Glob
---

# When to Use gabb vs Grep/Read

## The Core Decision

**Ask yourself: "Am I looking for CODE (functions, classes, symbols)?"**

| If YES → Use gabb | If NO → Use Grep/Read |
|-------------------|----------------------|
| Find a function | Search log messages |
| Find a class | Find config values |
| Find where X is defined | Search comments |
| Find usages of X | Find in .json/.md/.yaml |

## Supported Languages

| Language | Extensions |
|----------|------------|
| Python | `.py`, `.pyi` |
| TypeScript | `.ts`, `.tsx` |
| Rust | `.rs` |
| Kotlin | `.kt`, `.kts` |
| C++ | `.cpp`, `.cc`, `.cxx`, `.hpp`, `.hh` |

**For .js, .jsx, .go, .java, .c, .h → Use Grep/Read**

## Gabb is for PRECISION, Not Discovery

**For broad codebase exploration** → Use Task/Explore agent or Grep
**For precise symbol lookup** → Use gabb tools

## Start Here: The Two-Step Pattern

**Step 1: Get cheap overview** (no source code, just structure)
```
gabb_structure file="path/to/file.py"
```
Returns symbol names, kinds, line numbers. Use for any file >100 lines.

**Step 2: Get specific code** (one of these based on what you need)
- `gabb_symbol name="FunctionName" include_source=true` - get ONE specific symbol's code
- `Read file="path" offset=150 limit=50` - read specific line range from structure output

## Quick Reference

| Goal | Tool |
|------|------|
| Explore codebase broadly | Task/Explore agent or Grep |
| Preview file structure (cheap) | `gabb_structure file="path"` → [details](./tools/structure.md) |
| Get ONE symbol by exact name | `gabb_symbol name="X" include_source=true` → [details](./tools/symbols.md) |
| Find usages before refactoring | `gabb_usages file="X" line=N character=M` → [details](./tools/usages.md) |

**IMPORTANT:** Only use `include_source=true` for SPECIFIC symbol lookups (exact name). Broad searches with `name_contains` should NOT use `include_source` - returns too much data.

## Specialized Tools

For call tracing, type hierarchies, and other tasks:
- [callers.md](./tools/callers.md) / [callees.md](./tools/callees.md) - trace call graph
- [hierarchy.md](./tools/hierarchy.md) - supertypes/subtypes
- [definition.md](./tools/definition.md) - jump to definition
- [rename.md](./tools/rename.md) - safe renaming
- [implementations.md](./tools/implementations.md) - find interface implementations

## When to Fall Back to Grep/Read

1. Searching text content (error messages, comments, strings)
2. Unsupported file types (.js, .go, .java, .json, .md)
3. Finding config values or non-code content
