# gabb_callers

Find all functions that call a given function.

```
gabb_callers file="src/payments.py" line=42 character=5 include_source=true
```

Trace execution flow **backwards** - who calls this function?

**Parameters:**
- `file`, `line`, `character` - Position of function
- `include_source=true` - Include caller source code
- `transitive=true` - Follow full call chain (callers of callers)
- `limit` - Max results (default: 50)
