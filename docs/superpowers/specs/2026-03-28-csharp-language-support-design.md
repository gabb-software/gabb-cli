# C# Language Support for gabb-cli

**Date:** 2026-03-28
**Status:** Approved

## Overview

Add C# language support to gabb-cli's code indexing system, following the established tree-sitter parser pattern used by all other supported languages.

## Approach

Use the `tree-sitter-c-sharp` crate (the standard tree-sitter grammar for C#) and implement a parser following the exact same architecture as the existing Ruby/Python/Kotlin parsers.

## Parser Scope

### Symbols Extracted

| Kind | C# Construct | Qualifier | Visibility |
|------|-------------|-----------|------------|
| `class` | `class`, `record` | Enclosing namespace/class | public/private/internal/protected |
| `struct` | `struct`, `record struct` | Enclosing namespace/class | Same |
| `interface` | `interface` | Enclosing namespace/class | Same |
| `enum` | `enum` | Enclosing namespace/class | Same |
| `method` | Methods, constructors | Enclosing class/struct | Same |
| `property` | Properties | Enclosing class/struct | Same |
| `field` | Fields, constants | Enclosing class/struct | Same |
| `namespace` | `namespace` declarations | Parent namespace if nested | n/a |

### Edges

| Kind | Meaning |
|------|---------|
| `extends` | Class inheritance, struct implementing interface |
| `implements` | Interface implementation |
| `calls` | Method/constructor invocations |

### Dependencies

- `using` directives mapped as namespace-based dependencies
- Cross-file resolution handled by the existing two-phase indexing (import bindings resolved during resolution pass)

### Test Detection

- **Path patterns:** `/Tests/`, `/Test/`, `*Tests.cs`, `*Test.cs`
- **Attribute markers:** `[Test]`, `[Fact]`, `[Theory]`, `[TestMethod]`

### File Extensions

- `.cs`

## Architecture & Integration

### New File: `src/languages/csharp.rs`

Follows the established parser pattern:

```
static CSHARP_LANGUAGE: Lazy<Language>  // tree-sitter language singleton

pub fn index_file(path, source) -> Result<(symbols, edges, refs, deps, imports)>
  +-- walk_symbols()        // Recursive AST walk for symbol extraction
  |   +-- handle_class()    // class, record
  |   +-- handle_struct()   // struct, record struct
  |   +-- handle_interface()
  |   +-- handle_enum()
  |   +-- handle_method()   // methods + constructors
  |   +-- handle_property()
  |   +-- handle_field()
  +-- collect_references()  // Find usages of declared symbols
  +-- collect_imports()     // using directives -> import bindings
  +-- collect_call_edges()  // Method/constructor invocations

#[derive(Clone)]
pub struct CSharpParser;    // Implements LanguageParser trait
```

### Modified Files

- `Cargo.toml` -- add `tree-sitter-c-sharp` dependency
- `src/languages/mod.rs` -- add `pub mod csharp;`
- `src/languages/registry.rs` -- register `CSharpParser` for `.cs` extension

### Documentation Updates

- `assets/SKILL.md` -- add C# to supported languages table
- `.claude/skills/gabb/SKILL.md` -- add C# to supported languages table
- `README.md` -- update language lists
- `CLAUDE.md` -- add `csharp.rs` to Language Parsers list

### Tests

Inline `#[cfg(test)]` module in `csharp.rs` covering:

- Class/struct/interface/enum extraction
- Method and property extraction with visibility
- Inheritance and interface implementation edges
- Nested types (class within class, namespace nesting)
- `using` directive handling
- Test file/attribute detection
- Record types

## C#-Specific Design Decisions

### Namespace Handling
Both file-scoped (`namespace Foo;`) and block-scoped (`namespace Foo { }`) forms are supported. Symbols receive their full namespace as a qualifier (e.g., `MyApp.Models`).

### Partial Classes
Each `partial class` declaration produces its own symbol per file. No merging is performed -- the store's name-based lookup will find all parts. This matches how tree-sitter sees them as separate declarations.

### Generic Types
Stored by base name (e.g., `List` for `List<T>`). No special handling for type parameters -- consistent with TypeScript/Kotlin parsers.

### Properties vs Fields
Both extracted as separate symbol kinds. Auto-properties (`public string Name { get; set; }`) are treated as properties.

### Visibility Defaults
C# defaults are respected: class members default to `private`, top-level types default to `internal`. Explicit modifiers take precedence.

### `using` as Dependencies
Since C# `using` references namespaces (not files), dependencies are recorded as namespace references. Cross-file resolution relies on the two-phase indexing -- import bindings map `using` names to symbols found in other files during the resolution pass.

## Branch Strategy

Work will be done in a feature branch suitable for opening a PR to the upstream gabb-cli repository.
