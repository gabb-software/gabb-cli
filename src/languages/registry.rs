//! Parser registry for language dispatch.
//!
//! The `ParserRegistry` provides centralized parser lookup by file extension,
//! replacing the if/else chains in the indexer.

use super::traits::{LanguageParser, ParseResult};
use super::{cpp, kotlin, python, rust, typescript};
use anyhow::{bail, Result};
use std::collections::HashMap;
use std::path::Path;

/// Registry of language parsers indexed by file extension.
///
/// Use `ParserRegistry::new()` to create a registry with all built-in parsers,
/// or `ParserRegistry::empty()` to create an empty registry for testing.
pub struct ParserRegistry {
    /// Map from file extension (without dot) to parser
    parsers: HashMap<&'static str, Box<dyn LanguageParser>>,
}

impl ParserRegistry {
    /// Create a new registry with all built-in language parsers.
    pub fn new() -> Self {
        let mut registry = Self::empty();

        // Register TypeScript parser
        let ts_parser = typescript::TypeScriptParser::new();
        for ext in ts_parser.config().extensions {
            registry.parsers.insert(ext, Box::new(ts_parser.clone()));
        }

        // Register Rust parser
        let rust_parser = rust::RustParser::new();
        for ext in rust_parser.config().extensions {
            registry.parsers.insert(ext, Box::new(rust_parser.clone()));
        }

        // Register Kotlin parser
        let kotlin_parser = kotlin::KotlinParser::new();
        for ext in kotlin_parser.config().extensions {
            registry
                .parsers
                .insert(ext, Box::new(kotlin_parser.clone()));
        }

        // Register C++ parser
        let cpp_parser = cpp::CppParser::new();
        for ext in cpp_parser.config().extensions {
            registry.parsers.insert(ext, Box::new(cpp_parser.clone()));
        }

        // Register Python parser
        let python_parser = python::PythonParser::new();
        for ext in python_parser.config().extensions {
            registry
                .parsers
                .insert(ext, Box::new(python_parser.clone()));
        }

        registry
    }

    /// Create an empty registry (useful for testing).
    pub fn empty() -> Self {
        Self {
            parsers: HashMap::new(),
        }
    }

    /// Register a parser for its supported extensions.
    pub fn register<P: LanguageParser + Clone + 'static>(&mut self, parser: P) {
        for ext in parser.config().extensions {
            self.parsers.insert(ext, Box::new(parser.clone()));
        }
    }

    /// Get a parser for the given file path based on its extension.
    pub fn get_parser(&self, path: &Path) -> Option<&dyn LanguageParser> {
        let ext = path.extension()?.to_str()?;
        self.parsers.get(ext).map(|p| p.as_ref())
    }

    /// Check if a file is supported by any registered parser.
    pub fn is_supported(&self, path: &Path) -> bool {
        self.get_parser(path).is_some()
    }

    /// Parse a file using the appropriate parser.
    ///
    /// # Errors
    /// Returns an error if no parser is registered for the file type,
    /// or if parsing fails.
    pub fn parse(&self, path: &Path, source: &str) -> Result<ParseResult> {
        match self.get_parser(path) {
            Some(parser) => parser.parse(path, source),
            None => bail!(
                "no parser registered for file type: {}",
                path.extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("(no extension)")
            ),
        }
    }

    /// Get all supported file extensions.
    pub fn supported_extensions(&self) -> Vec<&'static str> {
        self.parsers.keys().copied().collect()
    }

    /// Get the names of all registered languages.
    pub fn registered_languages(&self) -> Vec<&'static str> {
        let mut names: Vec<_> = self
            .parsers
            .values()
            .map(|p| p.config().name)
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        names.sort();
        names
    }
}

impl Default for ParserRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn registry_finds_typescript_parser() {
        let registry = ParserRegistry::new();
        let path = PathBuf::from("test.ts");
        assert!(registry.is_supported(&path));
        assert!(registry.get_parser(&path).is_some());
    }

    #[test]
    fn registry_finds_rust_parser() {
        let registry = ParserRegistry::new();
        let path = PathBuf::from("test.rs");
        assert!(registry.is_supported(&path));
    }

    #[test]
    fn registry_finds_kotlin_parser() {
        let registry = ParserRegistry::new();
        assert!(registry.is_supported(&PathBuf::from("test.kt")));
        assert!(registry.is_supported(&PathBuf::from("test.kts")));
    }

    #[test]
    fn registry_finds_cpp_parser() {
        let registry = ParserRegistry::new();
        assert!(registry.is_supported(&PathBuf::from("test.cpp")));
        assert!(registry.is_supported(&PathBuf::from("test.hpp")));
        assert!(registry.is_supported(&PathBuf::from("test.cc")));
    }

    #[test]
    fn registry_finds_python_parser() {
        let registry = ParserRegistry::new();
        assert!(registry.is_supported(&PathBuf::from("test.py")));
        assert!(registry.is_supported(&PathBuf::from("test.pyi")));
    }

    #[test]
    fn registry_rejects_unsupported() {
        let registry = ParserRegistry::new();
        assert!(!registry.is_supported(&PathBuf::from("test.go")));
        assert!(!registry.is_supported(&PathBuf::from("test.java")));
    }

    #[test]
    fn registered_languages_returns_all() {
        let registry = ParserRegistry::new();
        let languages = registry.registered_languages();
        assert!(languages.contains(&"TypeScript"));
        assert!(languages.contains(&"Rust"));
        assert!(languages.contains(&"Kotlin"));
        assert!(languages.contains(&"C++"));
        assert!(languages.contains(&"Python"));
    }
}
