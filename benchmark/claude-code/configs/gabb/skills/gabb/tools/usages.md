# gabb_usages

Find all places where a symbol is used.

```
gabb_usages file="src/payments.py" line=42 character=5
```

**Use before refactoring** to understand impact. More accurate than Grep - understands code structure.

**Parameters:**
- `file`, `line`, `character` - Position of symbol definition
- `format="refactor"` - Returns edit-ready JSON for rename operations
- `limit` - Max results (default: 50)
