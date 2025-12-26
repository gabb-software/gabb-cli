---
name: gabb-code-navigation
description: |
  MANDATORY for Python/TypeScript/Rust/Kotlin/C++ code files. Use gabb_symbols
  to find functions/classes, gabb_structure before reading files, gabb_definition
  to trace definitions. NEVER use Grep/Read/Glob on code - gabb is faster and
  semantic. Activates on: finding code, bug hunting, understanding files,
  locating definitions, tracing execution, exploring codebases.
allowed-tools: mcp__gabb__*, Edit, Write, Bash, Read, Glob
---

# MANDATORY: Use gabb for Code Navigation

## STOP! Before Using Read/Grep/Glob

**For .py, .ts, .tsx, .rs, .kt, .cpp files - use gabb tools instead:**

| You want to... | DON'T use | USE this |
|---------------|-----------|----------|
| Find a function/class | `Grep "def foo"` | `gabb_symbols name_contains="foo" include_source=true` |
| Understand a file | `Read file.py` | `gabb_structure file="file.py"` |
| Get a definition | `Read` + search | `gabb_symbol name="MyClass" include_source=true` |
| Find all usages | `Grep "MyClass"` | `gabb_usages file="..." line=N character=M` |
| Trace execution | Multiple Reads | `gabb_callees` / `gabb_callers` |

**Rule: ALWAYS add `include_source=true`** - this gives you the code without needing Read.

## Quick Start: Bug Hunting Workflow

```
Step 1: gabb_symbols name_contains="<keyword>" include_source=true
        → Find functions/classes related to the bug

Step 2: gabb_structure file="<found_file>"
        → See ALL functions with line ranges

Step 3: gabb_callees file="..." line=N character=M include_source=true
        → Trace what the function calls

Step 4: gabb_symbol name="suspect_fn" include_source=true
        → Get exact source code
```

**4 gabb calls vs 15+ Grep/Read calls blindly exploring.**

## Supported Languages

| Language   | Extensions |
|------------|------------|
| Python     | `.py`, `.pyi` |
| TypeScript | `.ts`, `.tsx` |
| Rust       | `.rs` |
| Kotlin     | `.kt`, `.kts` |
| C++        | `.cpp`, `.cc`, `.hpp`, `.h++` |

**Not indexed** (use Grep/Glob): `.js`, `.jsx`, `.c`, `.h`, `.go`, `.java`

## The `include_source=true` Rule

**If you're about to call Read after a gabb tool, STOP.** You forgot `include_source=true`.

```
WRONG (3 calls):
gabb_symbols name_contains="config"  → lines 150, 280
Read file offset=145 limit=50
Read file offset=275 limit=50

RIGHT (1 call):
gabb_symbols name_contains="config" include_source=true
→ Full source code of ALL matching functions
```

## Key Tools Reference

### gabb_symbols - Search for symbols
```
gabb_symbols name_contains="user" kind="function" include_source=true
```
Filters: `name`, `name_contains`, `name_pattern`, `kind`, `file`

### gabb_symbol - Get specific symbol
```
gabb_symbol name="MyClass" include_source=true
```

### gabb_structure - File overview (USE BEFORE READ!)
```
gabb_structure file="src/large_file.py"
```
Returns hierarchical view of all functions/classes with line ranges.

### gabb_definition - Jump to definition
```
gabb_definition file="src/app.py" line=50 character=10
```
From usage → definition location with source.

### gabb_usages - Find all references
```
gabb_usages file="src/types.py" line=10 character=5
```
Essential before refactoring.

### gabb_callers / gabb_callees - Call graph
```
gabb_callers file="auth.py" line=50 character=10 include_source=true
gabb_callees file="auth.py" line=50 character=10 include_source=true
```
Trace execution flow backwards (callers) or forwards (callees).

## When Read/Grep ARE OK

- Non-code files: markdown, JSON, config, logs
- Unsupported languages: JavaScript, Go, Java, C
- Literal strings in comments or error messages
- After `gabb_structure`, reading <50 specific lines
