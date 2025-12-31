---
name: gabb-code-navigation
description: |
  Decision guide for code navigation. Teaches when to use gabb tools vs default
  tools (Grep/Read) for maximum efficiency. Applies to Python/TypeScript/Rust/Kotlin/C++.
allowed-tools: mcp__gabb__*, Edit, Write, Bash, Read, Glob
---

# gabb Code Navigation

## Purpose

Gabb provides fast, precise code navigation through a local symbol index. Use gabb tools instead of Grep/Read/Glob when navigating code in supported languages—gabb understands code structure (functions, classes, types) rather than just text patterns.

## When to Use gabb

Use gabb tools when you're working with **code symbols**:

- Finding where a function, class, or type is defined
- Finding all usages of a symbol before refactoring
- Tracing call graphs (who calls what, what calls whom)
- Understanding type hierarchies (inheritance, implementations)
- Getting a quick overview of a file's structure
- Safe, automated renaming across the codebase

## When NOT to Use gabb

Fall back to Grep/Read when:

- Searching **non-code files** (.json, .md, .yaml, .toml, .env)
- Searching **unsupported languages** (.js, .jsx, .go, .java, .c, .h)
- Finding **text content** (error messages, log strings, comments)
- Finding **config values** or environment variables
- Broad codebase exploration (use Task/Explore agent instead)

## Supported Languages

| Language   | Extensions                              |
|------------|----------------------------------------|
| Python     | `.py`, `.pyi`                          |
| TypeScript | `.ts`, `.tsx`                          |
| Rust       | `.rs`                                  |
| Kotlin     | `.kt`, `.kts`                          |
| C++        | `.cpp`, `.cc`, `.cxx`, `.hpp`, `.hh`   |

## Tool Selection Matrix

| Task | Tool | Notes |
|------|------|-------|
| **Preview file structure** | `gabb_structure file="path"` | Use FIRST for files >100 lines. Returns symbol names/lines, no source code. |
| **Find symbol by exact name** | `gabb_symbol name="X"` | Use when you know the exact name. Add `include_source=true` to see code. |
| **Search symbols by keyword** | `gabb_symbols name_contains="X"` | Substring/pattern match. Do NOT use `include_source` with broad searches. |
| **Jump to definition** | `gabb_definition file line character` | Point to a usage, get the definition location. |
| **Find all usages** | `gabb_usages file line character` | Find everywhere a symbol is referenced. Use before refactoring. |
| **Find callers** | `gabb_callers file line character` | Who calls this function? Trace execution backwards. |
| **Find callees** | `gabb_callees file line character` | What does this function call? Trace execution forwards. |
| **Find parent types** | `gabb_supertypes file line character` | What does this class inherit/implement? |
| **Find child types** | `gabb_subtypes file line character` | What extends/implements this type? |
| **Find implementations** | `gabb_implementations file line character` | Find concrete implementations of interface/trait. |
| **Safe rename** | `gabb_rename file line character new_name` | Get all locations to update for renaming. |

## Common Patterns

### Pattern: Understand a File Before Reading

For any file >100 lines, get structure first:

```
gabb_structure file="src/large_file.py"
```

This returns symbol names, kinds, and line numbers—no source code. Then either:
- Use `Read file="path" offset=150 limit=50` to read specific line ranges
- Use `gabb_symbol name="specific_function" include_source=true` to get one symbol's code

### Pattern: Safe Refactoring

Before modifying a function or type, find all usages:

```
gabb_usages file="src/payments.py" line=42 character=5
```

For automated rename with edit-ready output:

```
gabb_rename file="src/payments.py" line=42 character=5 new_name="process_order"
```

### Pattern: Trace Call Flow

To understand execution flow, use callers/callees:

```
# Who calls this function? (trace backwards)
gabb_callers file="src/api.py" line=100 character=5 transitive=true

# What does this function call? (trace forwards)
gabb_callees file="src/api.py" line=100 character=5 transitive=true
```

Use `transitive=true` to follow the full chain.

### Pattern: Explore Type Hierarchy

```
# What does this class inherit from?
gabb_supertypes file="src/models.py" line=20 character=5 transitive=true

# What classes extend this base?
gabb_subtypes file="src/base.py" line=10 character=5 transitive=true
```

## Choosing Between Similar Tools

### `gabb_usages` vs `gabb_callers`

| Use `gabb_usages` when... | Use `gabb_callers` when... |
|---------------------------|---------------------------|
| Finding ALL references to any symbol | Finding only FUNCTION call sites |
| Refactoring (need every mention) | Understanding execution flow |
| Symbol is a type, constant, or variable | Symbol is a function/method |

### `gabb_subtypes` vs `gabb_implementations`

| Use `gabb_subtypes` when... | Use `gabb_implementations` when... |
|-----------------------------|-----------------------------------|
| Finding class inheritance hierarchy | Finding interface/trait implementations |
| You have a base class | You have an interface or trait |
| Want full hierarchy with `transitive=true` | Want concrete implementations only |

### `gabb_symbol` vs `gabb_symbols`

| Use `gabb_symbol` when... | Use `gabb_symbols` when... |
|---------------------------|---------------------------|
| You know the exact name | Searching by keyword/pattern |
| Want one specific result | Want multiple matches |
| Always safe with `include_source=true` | Avoid `include_source` (too much data) |

## Tips and Best Practices

1. **Start with `gabb_structure`** - It's cheap (no source code returned) and gives you line numbers for targeted reading.

2. **Be careful with `include_source`** - Only use it for specific lookups (exact name via `gabb_symbol`). Broad searches with `include_source=true` return too much data.

3. **Use `transitive=true` sparingly** - Full call chains or type hierarchies can be large. Start without it, add if needed.

4. **Position parameters are 1-based** - Line and character numbers match what editors show (starting from 1, not 0).

5. **Use `format="refactor"`** - When you need edit-ready output from `gabb_usages`, this gives you exact text spans.

6. **Fall back gracefully** - If gabb returns no results for a supported language, the index may be stale. Use Grep as fallback.
