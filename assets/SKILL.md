---
name: gabb-code-navigation
description: |
  Teaches when to use gabb_structure for efficient file exploration.
  Use gabb_structure before reading large files in supported languages.
allowed-tools: mcp__gabb__gabb_structure, Edit, Write, Bash, Read, Glob
---

# gabb File Structure Preview

## Purpose

Gabb provides a single tool—`gabb_structure`—that gives you a cheap, lightweight overview of a file's symbols before reading it. Use it to see what's in a file without the token cost of reading the entire thing.

## When to Use `gabb_structure`

Use it when:
- You're about to read a file that might be large (>100 lines)
- You want to know what functions, classes, or types exist in a file
- You need line numbers to read specific sections with `Read offset/limit`

## When NOT to Use `gabb_structure`

Fall back to direct `Read` or `Grep` when:
- The file is short (<100 lines)—just read it directly
- Searching **non-code files** (.json, .md, .yaml, .toml, .env)
- Searching **unsupported languages** (.js, .jsx, .go, .java, .c, .h)
- Looking for **text content** (error messages, log strings, comments)

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
/path/to/file.rs (prod)
Summary: 15 functions, 3 structs | 450 lines
Key types: MyStruct (10 methods)
├─ struct MyStruct (pub) [prod]  [10:1 - 25:2]
│  ├─ function new (pub) [prod]  [12:5 - 15:6]
│  └─ function process [prod]  [17:5 - 24:6]
├─ function helper [prod]  [30:1 - 45:2]
└─ function main [prod]  [50:1 - 60:2]
```

The output shows:
- Summary stats (function count, struct count, line count)
- Key types with method counts
- Hierarchical tree with line ranges `[start:col - end:col]`
