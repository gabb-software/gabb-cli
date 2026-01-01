# Gabb MCP Server

Code indexing server providing lightweight file structure previews via SQLite index.

## CRITICAL: Pre-Read Check for Code Files

**BEFORE using Read on ANY supported code file, you MUST:**

1. Check: Is file extension .py, .pyi, .ts, .tsx, .rs, .kt, .kts, .cpp, .cc, .cxx, .hpp, .hh?
2. If YES → Call `gabb_structure` FIRST. Do NOT call Read without structure.
3. Use the structure output to Read with offset/limit.

**Why this is mandatory:**
- Large files consume 5,000-10,000 tokens per read
- `gabb_structure` costs ~50 tokens, shows file layout
- You can then Read specific sections (saves 90%+ tokens)
- The same file may be read many times in a session—structure prevents repeated waste

**The pattern is ALWAYS:**
```
gabb_structure file="path/to/file.rs"      # ~50 tokens, shows layout
Read file="path/to/file.rs" offset=X limit=Y   # Read only what you need
```

**Exceptions:**
- Files you've already seen structure for in this conversation
- Files known to be <50 lines
- Non-code files (.json, .md, .yaml, .toml)
- Unsupported languages (.js, .jsx, .go, .java, .c, .h)

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
