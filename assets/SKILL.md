---
name: gabb-code-navigation
description: |
  Teaches when to use gabb_structure for efficient file exploration.
  Use gabb_structure before reading large files in supported languages.
allowed-tools: mcp__gabb__gabb_structure, Edit, Write, Bash, Read, Glob
---

# gabb File Structure Preview

## Pre-Flight Checklist (Before ANY Read on Code Files)

Before calling `Read` on a code file, run this check:

```
□ Is file extension in [.py, .pyi, .ts, .tsx, .rs, .kt, .kts, .cpp, .cc, .cxx, .hpp, .hh]?
  ├─ NO  → Use Read directly (unsupported language)
  └─ YES → Have I called gabb_structure on this file in this session?
           ├─ NO  → Call gabb_structure FIRST, then decide what to read
           └─ YES → Use Read with offset/limit based on structure output
```

**Why checklists work**: They force a pause before automatic behavior.

## Purpose

Gabb provides a single tool—`gabb_structure`—that gives you a cheap, lightweight overview of a file's symbols before reading it. Use it to see what's in a file without the token cost of reading the entire thing.

## When to Use `gabb_structure`

**MANDATORY**: Before reading any supported code file, call `gabb_structure` first.

The ONLY exceptions are:
- Files known to be <50 lines
- Files you've already seen structure for in this conversation
- Non-code files (.json, .md, .yaml, .toml)
- Unsupported languages (.js, .jsx, .go, .java, .c, .h)

**Why this is mandatory:**
- Large files consume 5,000-10,000 tokens per Read
- `gabb_structure` costs ~50 tokens, shows file layout
- You can then Read specific sections (saves 90%+ tokens)

## Supported Languages

| Language   | Extensions                              |
|------------|----------------------------------------|
| Python     | `.py`, `.pyi`                          |
| TypeScript | `.ts`, `.tsx`                          |
| Rust       | `.rs`                                  |
| Kotlin     | `.kt`, `.kts`                          |
| C++        | `.cpp`, `.cc`, `.cxx`, `.hpp`, `.hh`   |

## Usage Pattern

```
1. gabb_structure file="src/large_file.rs"
   → Returns symbol names, kinds, line numbers (NO source code)

2. Read file_path="src/large_file.rs" offset=150 limit=50
   → Read only the section you need
```

## What the Output Looks Like

```
/path/to/file.rs:450
Summary: 15 functions, 3 structs | 450 lines
Key types: MyStruct (10 methods)

MyStruct st 10
 new fn 12
 process fn 17
helper fn 30
main fn 50
```

The output shows:
- File path and line count
- Summary stats (function count, struct count, line count)
- Key types with method counts
- Compact symbol tree: `name kind_abbrev line` with single-space indent for children
