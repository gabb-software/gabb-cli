# gabb_supertypes / gabb_subtypes

Navigate type hierarchies.

## gabb_supertypes - Find parent types

```
gabb_supertypes file="src/models.py" line=20 character=5 transitive=true
```

What does this class inherit from? What interfaces does it implement?

## gabb_subtypes - Find child types

```
gabb_subtypes file="src/base.py" line=10 character=5 transitive=true
```

What classes extend this? What implements this interface?

**Parameters:**
- `file`, `line`, `character` - Position of type
- `transitive=true` - Full hierarchy chain, not just direct parents/children
- `include_source=true` - Include source code
- `limit` - Max results (default: 50)
