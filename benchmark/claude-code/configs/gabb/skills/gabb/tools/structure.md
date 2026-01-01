# gabb_structure

**Cheap overview of a file before reading it.**

```
gabb_structure file="src/payments.py"
```

## ⚠️ MANDATORY PRE-READ CHECK

**Before calling Read on ANY .py/.ts/.tsx/.rs/.kt/.cpp/.cc/.hpp file, you MUST call gabb_structure FIRST.**

- Reading a large file directly can cost 5,000-10,000 tokens
- gabb_structure costs ~50 tokens and shows what's inside
- You can then Read with offset/limit (saves 90%+ tokens)

**Exceptions:**
- Files known to be <50 lines
- Files you've already seen structure for in this conversation
- Non-code files (.json, .md, .yaml, .toml)
- Unsupported languages (.js, .jsx, .go, .java, .c, .h)

## What it shows:

- **Summary stats**: symbol counts by kind, total line count
- **Key types**: important public types with many methods
- Symbol names, kinds (function, class, method)
- Line numbers and positions
- Hierarchy (methods inside classes)
- Test vs production context

**After seeing structure:**
- Use `Read` with `offset`/`limit` to read specific line ranges
- Use `gabb_symbols name="FunctionName" include_source=true` to get a specific symbol's code

**Parameters:**
- `file` - Path to analyze (required)
