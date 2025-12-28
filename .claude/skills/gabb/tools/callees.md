# gabb_callees

Find all functions called by a given function.

```
gabb_callees file="src/payments.py" line=42 character=5 include_source=true
```

Trace execution flow **forwards** - what does this function call?

**Parameters:**
- `file`, `line`, `character` - Position of function
- `include_source=true` - Include callee source code
- `transitive=true` - Follow full call chain (callees of callees)
- `limit` - Max results (default: 50)
