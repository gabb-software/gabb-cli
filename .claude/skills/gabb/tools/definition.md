# gabb_definition

Jump from a usage to where the symbol is defined.

```
gabb_definition file="src/app.py" line=15 character=10 include_source=true
```

Point to where a symbol is **used** and get where it's **defined**.

**Parameters:**
- `file`, `line`, `character` - Position of the usage
- `include_source=true` - Include definition's source code
