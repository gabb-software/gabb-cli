---
name: gabb-code-navigation
description: |
  Teaches when to use gabb_structure for efficient file exploration.
  Use gabb_structure before reading large files in supported languages.
allowed-tools: mcp__gabb__gabb_structure, Edit, Write, Bash, Read, Glob
---

# gabb File Structure Preview

## When to Use `gabb_structure`

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
