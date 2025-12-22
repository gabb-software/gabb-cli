pub mod cpp;
pub mod kotlin;
pub mod rust;
pub mod typescript;

use crate::store::SymbolRecord;
use tree_sitter::Node;

// ============================================================================
// Shared parser utilities
// ============================================================================

/// Extract the text content of a tree-sitter node from source code.
pub fn slice(source: &str, node: &Node) -> String {
    let bytes = node.byte_range();
    source.get(bytes).unwrap_or_default().trim().to_string()
}

/// Binding for a locally-defined symbol with its ID and optional qualifier.
#[derive(Clone, Debug)]
pub struct SymbolBinding {
    pub id: String,
    pub qualifier: Option<String>,
}

impl From<&SymbolRecord> for SymbolBinding {
    fn from(value: &SymbolRecord) -> Self {
        Self {
            id: value.id.clone(),
            qualifier: value.qualifier.clone(),
        }
    }
}

/// A resolved target symbol with its ID and optional qualifier.
/// Used for resolving member access and method calls.
#[derive(Clone, Debug)]
pub struct ResolvedTarget {
    pub id: String,
    pub qualifier: Option<String>,
}

impl ResolvedTarget {
    /// Build a qualified member ID from this target.
    pub fn member_id(&self, member: &str) -> String {
        if let Some(q) = &self.qualifier {
            format!("{q}::{member}")
        } else {
            format!("{}::{member}", self.id)
        }
    }
}

// ============================================================================
// Import bindings
// ============================================================================

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
    /// The full import statement text (e.g., "import { foo } from './bar'")
    pub import_text: String,
}

/// Minimum symbol size in bytes to compute content hash.
/// Smaller symbols (getters, trivial functions) are skipped to reduce noise.
const MIN_HASH_SIZE: usize = 50;

/// Compute a normalized content hash for duplicate detection.
/// Returns None if the content is too small (< MIN_HASH_SIZE bytes).
///
/// Normalization:
/// - Strips leading/trailing whitespace
/// - Collapses all internal whitespace to single spaces
/// - Uses blake3 for fast, high-quality hashing
/// - Returns first 16 hex chars (64 bits) - sufficient for grouping
pub fn compute_content_hash(source: &[u8], start: usize, end: usize) -> Option<String> {
    if end <= start || end > source.len() {
        return None;
    }

    let body = &source[start..end];
    if body.len() < MIN_HASH_SIZE {
        return None;
    }

    // Normalize: collapse whitespace, trim
    let normalized = normalize_whitespace(body);
    if normalized.len() < MIN_HASH_SIZE {
        return None;
    }

    let hash = blake3::hash(&normalized);
    Some(hash.to_hex()[..16].to_string())
}

/// Normalize whitespace in source code for consistent hashing.
/// - Converts all whitespace sequences to single space
/// - Trims leading/trailing whitespace
fn normalize_whitespace(source: &[u8]) -> Vec<u8> {
    let mut result = Vec::with_capacity(source.len());
    let mut in_whitespace = true; // Start true to trim leading

    for &b in source {
        if b.is_ascii_whitespace() {
            if !in_whitespace && !result.is_empty() {
                result.push(b' ');
            }
            in_whitespace = true;
        } else {
            result.push(b);
            in_whitespace = false;
        }
    }

    // Trim trailing space
    if result.last() == Some(&b' ') {
        result.pop();
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_content_hash_normalizes_whitespace() {
        let source1 = b"function foo() {\n    return 42;\n} // some padding to meet min size requirement here";
        let source2 =
            b"function foo() { return 42; } // some padding to meet min size requirement here";

        let hash1 = compute_content_hash(source1, 0, source1.len());
        let hash2 = compute_content_hash(source2, 0, source2.len());

        assert!(hash1.is_some());
        assert!(hash2.is_some());
        assert_eq!(
            hash1, hash2,
            "Different whitespace should produce same hash"
        );
    }

    #[test]
    fn test_compute_content_hash_skips_small_content() {
        let source = b"fn x() {}";
        let hash = compute_content_hash(source, 0, source.len());
        assert!(hash.is_none(), "Small content should not be hashed");
    }

    #[test]
    fn test_compute_content_hash_different_content() {
        let source1 =
            b"function calculateTotal(items) { return items.reduce((a, b) => a + b, 0); }";
        let source2 =
            b"function calculateSum(values) { return values.reduce((x, y) => x + y, 0); }";

        let hash1 = compute_content_hash(source1, 0, source1.len());
        let hash2 = compute_content_hash(source2, 0, source2.len());

        assert!(hash1.is_some());
        assert!(hash2.is_some());
        assert_ne!(
            hash1, hash2,
            "Different content should produce different hashes"
        );
    }
}
