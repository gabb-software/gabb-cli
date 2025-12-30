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

## Quick Paths (Use These First)

**Issue mentions a specific function/method name?**
→ `gabb_symbol name="exact_name" include_source=true` — **1 tool call, done**

**Issue mentions a concept but not exact name?**
→ `gabb_symbols name_contains="concept" include_source=true` — **1 tool call**

**Need to understand a file's structure first?**
→ `gabb_structure file="path"` then targeted lookup — **2 calls**

## Quick Reference

| Goal | Tool |
|------|------|
| Preview file structure (cheap) | `gabb_structure file="path"` → [details](./tools/structure.md) |
| Find and read code by keyword | `gabb_symbols name_contains="X" include_source=true` → [details](./tools/symbols.md) |
| Find usages before refactoring | `gabb_usages file="X" line=N character=M` → [details](./tools/usages.md) |

**Use `include_source=true`** on `gabb_symbols`, `gabb_symbol`, `gabb_definition` - NOT on `gabb_structure`.

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
