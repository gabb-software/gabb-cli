# Gabb MCP Server

Code indexing server providing smart file previews via SQLite index.

## Task Complexity Assessment

Before using gabb exploration tools, assess whether exploration is needed:

**Go direct (skip exploration) when:**
- Task names a specific file or function (e.g., "fix bug in utils.py")
- Change is localized (e.g., "add parameter to X", "rename Y to Z")
- You can identify the exact target from the task description
- Task is a simple fix with obvious location

**Explore when:**
- Task requires understanding system architecture
- You need to find where something is implemented
- Change affects multiple components or has unclear scope
- You're unfamiliar with the codebase structure

## Smart File Preview

Use `gabb_peek` as your first step when exploring any file. It automatically
returns the right format based on file size and type:

- **Small files (<75 lines):** Full contents with line numbers
- **Non-code files (.json, .md, .yaml):** Full contents with line numbers
- **Large code files (>75 lines):** Symbol structure overview

This eliminates the need to guess file size before deciding between tools.

**The pattern:**
```
gabb_peek file="path/to/file"
→ Small/non-code: full contents with line numbers
→ Large code: symbol structure (~50 tokens)

If structure returned and you need more:
Read file="path/to/file" offset=X limit=Y
```

## Supported Languages

Symbol structure is available for these languages:

| Language   | Extensions                              |
|------------|----------------------------------------|
| Python     | `.py`, `.pyi`                          |
| TypeScript | `.ts`, `.tsx`                          |
| Rust       | `.rs`                                  |
| Kotlin     | `.kt`, `.kts`                          |
| C++        | `.cpp`, `.cc`, `.cxx`, `.hpp`, `.hh`   |

## When NOT to Use gabb

Fall back to Grep/Read for:
- Searching for text content (strings, comments, error messages)
  (gabb_peek shows file contents or symbols, not search results)
