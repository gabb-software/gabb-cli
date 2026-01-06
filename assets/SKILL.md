---
name: gabb-code-navigation
description: |
  Teaches when to use gabb_peek for efficient file exploration.
  gabb_peek is a smart preview tool that returns the right format automatically.
allowed-tools: mcp__gabb__gabb_peek, Edit, Write, Bash, Read, Glob
---

# gabb Smart File Preview

## When to Use `gabb_peek`

**First:** Assess if exploration is needed (see MCP instructions).
For trivial tasks with obvious targets, go directly to the file.

**If exploring:** Use `gabb_peek` as your first step when exploring any file.
It automatically returns the right format:
- **Small files (<75 lines):** Full contents with line numbers
- **Non-code files (.json, .md, .yaml):** Full contents with line numbers
- **Large code files (>75 lines):** Symbol structure overview

This eliminates guessing about file size or type.

## Supported Languages for Structure

When returning structure (for large code files):

| Language   | Extensions                              |
|------------|----------------------------------------|
| Python     | `.py`, `.pyi`                          |
| TypeScript | `.ts`, `.tsx`                          |
| Rust       | `.rs`                                  |
| Kotlin     | `.kt`, `.kts`                          |
| C++        | `.cpp`, `.cc`, `.cxx`, `.hpp`, `.hh`   |

## Usage Pattern

```
1. gabb_peek file="src/some_file.py"
   → For small/non-code: full contents with line numbers
   → For large code: symbol structure

2. If structure returned and you need more:
   Read file_path="src/some_file.py" offset=150 limit=50
```

## Output Examples

### Small File or Non-Code File
```
path/to/config.json (42 lines, non-code file)
    1→{
    2→  "name": "example",
    3→  ...
   42→}
```

### Large Code File
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

## When NOT to Use gabb_peek

Fall back to Grep directly for:
- Searching for string literals, regex patterns, or error messages
  (gabb_peek shows file contents or symbols, not search results)
