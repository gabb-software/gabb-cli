---
name: gabb-code-navigation
description: |
  Teaches when to use gabb_structure for efficient file exploration.
  Use gabb_structure before reading large files in supported languages.
allowed-tools: mcp__gabb__gabb_structure, mcp__gabb__gabb_symbol, Edit, Write, Bash, Read, Glob
---

# Gabb Code Navigation

## Search Strategy (Parallel-First)

When the target location isn't obvious, run MULTIPLE searches in parallel:

**Combine in ONE turn:**
- `gabb_symbol` for likely function/class names
- `Grep` for error messages, string literals, or text patterns
- `Glob` for filename patterns (if file location is hinted)

**Example:** Task mentions "fix the IsNull lookup validation"
→ Call `gabb_symbol(name="IsNull")` AND `Grep(pattern="IsNull")` in same turn
→ Don't wait for one to finish before trying the other

**After finding a location:** Immediately Read the file in the SAME response.
Don't make it a separate turn.

**Only go sequential when:**
- First search definitively found the target (no need for more)
- You need the result of tool A to know what to search for with tool B

**Efficiency target:** Aim for ≤3 turns per task. Each turn adds context tokens.

**Skip exploration entirely when:**
- Task names a specific file/function → Read directly
- Change is localized with obvious target

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

**First:** Assess if exploration is needed (see MCP instructions).
For trivial tasks with obvious targets, go directly to the file.

**If exploring:** Before reading large or unfamiliar code files, consider using `gabb_structure` to preview the layout.
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
- **You're searching for string literals, regex patterns, or error messages**
  (gabb_structure shows symbols, not strings—use Grep directly)

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
