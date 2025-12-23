//! Core abstractions for language parsers.
//!
//! This module defines the `LanguageParser` trait and associated types that provide
//! a unified interface for all language-specific parsers. This enables:
//! - Consistent parser interface across languages
//! - Easy addition of new language support
//! - Centralized parser dispatch via `ParserRegistry`

use crate::store::{EdgeRecord, FileDependency, ReferenceRecord, SymbolRecord};
use anyhow::Result;
use std::path::Path;
use tree_sitter::Language;

use super::ImportBindingInfo;

/// Result of parsing a single file.
///
/// Contains all indexable information extracted from a source file.
#[derive(Debug, Clone, Default)]
pub struct ParseResult {
    /// Symbol definitions found in the file
    pub symbols: Vec<SymbolRecord>,
    /// Relationships between symbols (implements, extends, calls, etc.)
    pub edges: Vec<EdgeRecord>,
    /// References to symbols (usages)
    pub references: Vec<ReferenceRecord>,
    /// File dependencies (imports, includes, mod declarations)
    pub dependencies: Vec<FileDependency>,
    /// Import bindings for cross-file reference resolution
    pub import_bindings: Vec<ImportBindingInfo>,
}

impl ParseResult {
    /// Create a new empty ParseResult.
    pub fn new() -> Self {
        Self::default()
    }

    /// Convert to the tuple format used by legacy parser functions.
    #[allow(clippy::type_complexity)]
    pub fn into_tuple(
        self,
    ) -> (
        Vec<SymbolRecord>,
        Vec<EdgeRecord>,
        Vec<ReferenceRecord>,
        Vec<FileDependency>,
        Vec<ImportBindingInfo>,
    ) {
        (
            self.symbols,
            self.edges,
            self.references,
            self.dependencies,
            self.import_bindings,
        )
    }

    /// Create from the tuple format used by legacy parser functions.
    #[allow(clippy::type_complexity)]
    pub fn from_tuple(
        tuple: (
            Vec<SymbolRecord>,
            Vec<EdgeRecord>,
            Vec<ReferenceRecord>,
            Vec<FileDependency>,
            Vec<ImportBindingInfo>,
        ),
    ) -> Self {
        Self {
            symbols: tuple.0,
            edges: tuple.1,
            references: tuple.2,
            dependencies: tuple.3,
            import_bindings: tuple.4,
        }
    }
}

/// Configuration for a language parser.
#[derive(Debug, Clone)]
pub struct LanguageConfig {
    /// Human-readable language name (e.g., "TypeScript", "Rust")
    pub name: &'static str,
    /// File extensions this parser handles (e.g., &["ts", "tsx"])
    pub extensions: &'static [&'static str],
}

/// The core trait for language parsers.
///
/// Implementors provide language-specific parsing logic while the framework
/// handles common concerns like file I/O, caching, and dispatch.
///
/// # Example
///
/// ```ignore
/// struct MyLanguageParser {
///     language: Language,
/// }
///
/// impl LanguageParser for MyLanguageParser {
///     fn config(&self) -> LanguageConfig {
///         LanguageConfig {
///             name: "MyLanguage",
///             extensions: &["ml", "mli"],
///         }
///     }
///
///     fn language(&self) -> &Language {
///         &self.language
///     }
///
///     fn parse(&self, path: &Path, source: &str) -> Result<ParseResult> {
///         // Parse the file and extract symbols, edges, references, etc.
///         Ok(ParseResult::new())
///     }
/// }
/// ```
pub trait LanguageParser: Send + Sync {
    /// Get parser configuration (name, extensions).
    fn config(&self) -> LanguageConfig;

    /// Get the tree-sitter language for this parser.
    fn language(&self) -> &Language;

    /// Parse a file and extract all indexable information.
    ///
    /// # Arguments
    /// * `path` - Path to the source file (used for symbol IDs and qualifiers)
    /// * `source` - Source code content
    ///
    /// # Returns
    /// A `ParseResult` containing symbols, edges, references, dependencies, and import bindings.
    fn parse(&self, path: &Path, source: &str) -> Result<ParseResult>;

    /// Check if this parser handles files with the given extension.
    fn handles_extension(&self, ext: &str) -> bool {
        self.config().extensions.contains(&ext)
    }
}

/// Standardized symbol kinds across all languages.
///
/// These provide a common vocabulary for symbol types, though languages
/// may use language-specific strings in the database for finer granularity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SymbolKind {
    Function,
    Method,
    Class,
    Struct,
    Interface,
    Trait,
    Enum,
    EnumMember,
    Type,
    Const,
    Variable,
    Property,
    Module,
    Namespace,
}

impl SymbolKind {
    /// Convert to storage string.
    pub fn as_str(&self) -> &'static str {
        match self {
            SymbolKind::Function => "function",
            SymbolKind::Method => "method",
            SymbolKind::Class => "class",
            SymbolKind::Struct => "struct",
            SymbolKind::Interface => "interface",
            SymbolKind::Trait => "trait",
            SymbolKind::Enum => "enum",
            SymbolKind::EnumMember => "enum_member",
            SymbolKind::Type => "type",
            SymbolKind::Const => "const",
            SymbolKind::Variable => "variable",
            SymbolKind::Property => "property",
            SymbolKind::Module => "module",
            SymbolKind::Namespace => "namespace",
        }
    }

    /// Parse from storage string.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "function" => Some(SymbolKind::Function),
            "method" => Some(SymbolKind::Method),
            "class" => Some(SymbolKind::Class),
            "struct" => Some(SymbolKind::Struct),
            "interface" => Some(SymbolKind::Interface),
            "trait" => Some(SymbolKind::Trait),
            "enum" => Some(SymbolKind::Enum),
            "enum_member" => Some(SymbolKind::EnumMember),
            "type" => Some(SymbolKind::Type),
            "const" => Some(SymbolKind::Const),
            "variable" => Some(SymbolKind::Variable),
            "property" => Some(SymbolKind::Property),
            "module" => Some(SymbolKind::Module),
            "namespace" => Some(SymbolKind::Namespace),
            _ => None,
        }
    }
}

/// Standardized relationship types between symbols.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EdgeKind {
    /// Class/struct implements interface/trait
    Implements,
    /// Class extends another class
    Extends,
    /// Type implements a trait (Rust-specific)
    TraitImpl,
    /// Method overrides parent method
    Overrides,
    /// Function/method calls another
    Calls,
    /// Symbol is imported from another file
    Import,
    /// Symbol is exported
    Export,
    /// Method belongs to a type (inherent impl in Rust)
    InherentImpl,
}

impl EdgeKind {
    /// Convert to storage string.
    pub fn as_str(&self) -> &'static str {
        match self {
            EdgeKind::Implements => "implements",
            EdgeKind::Extends => "extends",
            EdgeKind::TraitImpl => "trait_impl",
            EdgeKind::Overrides => "overrides",
            EdgeKind::Calls => "calls",
            EdgeKind::Import => "import",
            EdgeKind::Export => "export",
            EdgeKind::InherentImpl => "inherent_impl",
        }
    }

    /// Parse from storage string.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "implements" => Some(EdgeKind::Implements),
            "extends" => Some(EdgeKind::Extends),
            "trait_impl" => Some(EdgeKind::TraitImpl),
            "overrides" => Some(EdgeKind::Overrides),
            "calls" => Some(EdgeKind::Calls),
            "import" => Some(EdgeKind::Import),
            "export" => Some(EdgeKind::Export),
            "inherent_impl" => Some(EdgeKind::InherentImpl),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbol_kind_roundtrip() {
        for kind in [
            SymbolKind::Function,
            SymbolKind::Method,
            SymbolKind::Class,
            SymbolKind::Struct,
            SymbolKind::Interface,
            SymbolKind::Trait,
            SymbolKind::Enum,
            SymbolKind::EnumMember,
            SymbolKind::Type,
            SymbolKind::Const,
            SymbolKind::Variable,
            SymbolKind::Property,
            SymbolKind::Module,
            SymbolKind::Namespace,
        ] {
            assert_eq!(SymbolKind::parse(kind.as_str()), Some(kind));
        }
    }

    #[test]
    fn edge_kind_roundtrip() {
        for kind in [
            EdgeKind::Implements,
            EdgeKind::Extends,
            EdgeKind::TraitImpl,
            EdgeKind::Overrides,
            EdgeKind::Calls,
            EdgeKind::Import,
            EdgeKind::Export,
            EdgeKind::InherentImpl,
        ] {
            assert_eq!(EdgeKind::parse(kind.as_str()), Some(kind));
        }
    }

    #[test]
    fn parse_result_tuple_roundtrip() {
        let result = ParseResult {
            symbols: vec![],
            edges: vec![],
            references: vec![],
            dependencies: vec![],
            import_bindings: vec![],
        };
        let tuple = result.clone().into_tuple();
        let back = ParseResult::from_tuple(tuple);
        assert_eq!(back.symbols.len(), result.symbols.len());
    }
}
