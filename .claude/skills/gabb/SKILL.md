---
name: gabb-code-navigation
description: |
  Teaches when to use gabb_structure for efficient file exploration.
  Use gabb_structure before reading large files in supported languages.
allowed-tools: mcp__gabb__gabb_structure, mcp__gabb__gabb_symbol, Edit, Write, Bash, Read, Glob
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

**After `gabb_symbol` returns a location:** Go directly to `Read` with offset.
Don't call `gabb_structure` on a file where you already know the target line.

**SKIP `gabb_structure` when:**
- `gabb_symbol` already found the exact file:line location
- You're reading a single known file (not choosing between files)
- The file is <200 lines (just read it directly)
- You only need a specific function/class (use offset from symbol search)
- You're searching for string literals or error messages (use Grep)

**USE `gabb_structure` only when:**
- Multiple files matched and you need to pick the right one
- You need to understand overall file organization before making changes
- The file is very large (>500 lines) AND you don't know which section to read

## Supported Languages

| Language   | Extensions                              |
|------------|----------------------------------------|
| Python     | `.py`, `.pyi`                          |
| TypeScript | `.ts`, `.tsx`                          |
| Rust       | `.rs`                                  |
| Go         | `.go`                                  |
| Kotlin     | `.kt`, `.kts`                          |
| C++        | `.cpp`, `.cc`, `.cxx`, `.hpp`, `.hh`   |

## Usage Patterns

### Symbol Search
```
gabb_symbol name="MyClass"
→ class MyClass [prod] src/models.py:45:1
```

### File Structure Preview
```
1. gabb_structure file="src/large_file.rs"
   → Returns symbol names, kinds, line numbers (NO source code)

2. Read file_path="src/large_file.rs" offset=150 limit=50
   → Read only the section you need
```

## What `gabb_structure` Output Looks Like

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
