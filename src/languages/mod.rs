pub mod rust;
pub mod typescript;

/// Represents an import binding that maps a local name to a symbol in another file.
/// Used for two-phase indexing to resolve cross-file references.
#[derive(Clone, Debug)]
pub struct ImportBindingInfo {
    /// The name used locally in the importing file (may be aliased)
    pub local_name: String,
    /// The resolved path of the source file (canonical path)
    pub source_file: String,
    /// The original name exported from the source file (before aliasing)
    pub original_name: String,
}
