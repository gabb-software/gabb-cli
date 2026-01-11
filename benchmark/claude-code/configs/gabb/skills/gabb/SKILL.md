---
name: gabb-code-navigation
description: |
  Decision guide for code navigation. Teaches when to use gabb tools vs default
  tools (Grep/Read) for maximum efficiency. Applies to Python/TypeScript/Rust/Kotlin/C++.
allowed-tools: mcp__gabb__*, Edit, Write, Bash, Read, Glob
---

# Gabb Code Navigation

## Search Strategy Decision Flow

When you need to find code, follow this order:

1. **Task names specific file/function?** → Read directly (skip exploration)
2. **Looking for a code construct by name?** → `gabb_symbol`
3. **Looking for text content (strings, error messages)?** → Grep
4. **Need to understand file layout?** → `gabb_structure`

## `gabb_symbol` - Workspace Symbol Search

Search for symbols (functions, classes, methods) by name across the workspace.

**When to use:**
- Task mentions a function/class/method name to find or fix
- You need to find where something is defined
- Grep would return too many false positives

**Example:**
```
gabb_symbol name="update_proxy_model_permissions"
→ function update_proxy_model_permissions [prod] migrations/0011_update_proxy_permissions.py:5:1
```

**Use Grep instead when:**
- Searching for error messages or string literals
- Looking for text patterns, not code identifiers

## `gabb_structure` - File Layout Preview

Before reading large or unfamiliar code files, consider using `gabb_structure` to preview the layout.
This saves tokens when you only need part of a large file.

**Recommended for:**
- Large files (>100 lines) where you only need part
- Unfamiliar codebases where you're exploring
- Files you'll read multiple times

**Skip when:**
- You already know exactly what you're looking for
- The file is likely small (<100 lines)
- You can answer from existing context
- Files you've already seen structure for in this conversation

## Supported Languages

| Language | Extensions |
|----------|------------|
| Python | `.py`, `.pyi` |
| TypeScript | `.ts`, `.tsx` |
| Rust | `.rs` |
| Kotlin | `.kt`, `.kts` |
| C++ | `.cpp`, `.cc`, `.cxx`, `.hpp`, `.hh` |

**For .js, .jsx, .go, .java, .c, .h → Use Grep/Read**

## Quick Reference

| Goal | Tool |
|------|------|
| Find symbol by name | `gabb_symbol name="X"` |
| Preview file structure | `gabb_structure file="path"` |
| Find usages before refactoring | `gabb_usages file="X" line=N character=M` |

## When to Fall Back to Grep/Read

1. Searching text content (error messages, comments, strings)
2. Unsupported file types (.js, .go, .java, .json, .md)
3. Finding config values or non-code content
