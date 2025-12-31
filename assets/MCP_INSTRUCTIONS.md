# Gabb MCP Server

Code indexing server providing lightweight file structure previews via SQLite index.

## The `gabb_structure` Tool

Use `gabb_structure` to get a **cheap, lightweight overview** of a file before reading it.

**When to use:**
- Before reading any file >100 lines
- To understand what functions/classes exist in a file
- To get line numbers for targeted `Read` calls

**What it returns:**
- File path and summary stats (function count, class count, line count)
- Key types with their line ranges
- Symbol hierarchy tree (names, kinds, line numbers)
- NO source code (saves tokens)

**Example workflow:**
```
1. gabb_structure file="src/large_file.rs"    → See symbols and line numbers
2. Read file_path="src/large_file.rs" offset=150 limit=50  → Read specific section
```

## Supported Languages

| Language   | Extensions                              |
|------------|----------------------------------------|
| Python     | `.py`, `.pyi`                          |
| TypeScript | `.ts`, `.tsx`                          |
| Rust       | `.rs`                                  |
| Kotlin     | `.kt`, `.kts`                          |
| C++        | `.cpp`, `.cc`, `.cxx`, `.hpp`, `.hh`   |

## When NOT to Use gabb

Fall back to Grep/Read for:
- Non-code files (.json, .md, .yaml, .toml)
- Unsupported languages (.js, .jsx, .go, .java, .c, .h)
- Searching for text content (strings, comments)
