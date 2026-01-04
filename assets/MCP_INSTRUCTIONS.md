# Gabb MCP Server

Code indexing server providing lightweight file structure previews via SQLite index.

## Pre-Read Check for Code Files (Recommended)

For large or unfamiliar code files, consider calling `gabb_structure` first to see the layout.
This saves tokens when you only need part of a large file.

**Recommended for:**
- Large files (>100 lines) where you only need part
- Unfamiliar codebases where you're exploring
- Files you'll read multiple times in a session

**Skip this check when:**
- You already know exactly what you're looking for
- The file is likely small (<100 lines)
- You can answer from existing context
- Files you've already seen structure for

**The pattern:**
```
gabb_structure file="path/to/file.rs"      # ~50 tokens, shows layout
Read file="path/to/file.rs" offset=X limit=Y   # Read only what you need
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
