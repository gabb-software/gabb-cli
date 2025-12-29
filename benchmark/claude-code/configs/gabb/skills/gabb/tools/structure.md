# gabb_structure

**Cheap overview of a file before reading it.**

```
gabb_structure file="src/payments.py"
```

**USE THIS FIRST** for any file >100 lines. It's a lightweight table of contents - shows what's in the file without returning source code:

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
