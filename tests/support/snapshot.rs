use std::collections::BTreeMap;
use std::path::Path;

use gabb_cli::store::IndexStore;
use serde::{Deserialize, Serialize};

/// A serializable snapshot of the index state
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct WorkspaceSnapshot {
    /// Symbols organized by file (sorted for determinism)
    pub symbols: BTreeMap<String, Vec<SymbolSnapshot>>,
    /// Edges organized by source symbol
    pub edges: BTreeMap<String, Vec<EdgeSnapshot>>,
    /// Reference counts per symbol
    pub reference_counts: BTreeMap<String, usize>,
    /// File dependency graph
    pub dependencies: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Clone)]
pub struct SymbolSnapshot {
    pub name: String,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visibility: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub container: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Clone)]
pub struct EdgeSnapshot {
    pub dst: String,
    pub kind: String,
}

impl WorkspaceSnapshot {
    /// Capture current index state
    pub fn capture(store: &IndexStore) -> Self {
        let all_symbols = store
            .list_symbols(None, None, None, None)
            .unwrap_or_default();

        let mut symbols: BTreeMap<String, Vec<SymbolSnapshot>> = BTreeMap::new();
        let mut reference_counts: BTreeMap<String, usize> = BTreeMap::new();
        let mut edges: BTreeMap<String, Vec<EdgeSnapshot>> = BTreeMap::new();

        for sym in all_symbols {
            // Normalize file path for portability
            let file_key = normalize_snapshot_path(&sym.file);

            symbols.entry(file_key).or_default().push(SymbolSnapshot {
                name: sym.name.clone(),
                kind: sym.kind.clone(),
                visibility: sym.visibility.clone(),
                container: sym.container.clone(),
            });

            // Collect reference count
            let refs = store.references_for_symbol(&sym.id).unwrap_or_default();
            if !refs.is_empty() {
                reference_counts.insert(sym.name.clone(), refs.len());
            }

            // Collect edges
            let sym_edges = store.edges_from(&sym.id).unwrap_or_default();
            if !sym_edges.is_empty() {
                edges.insert(
                    sym.name.clone(),
                    sym_edges
                        .into_iter()
                        .map(|e| EdgeSnapshot {
                            dst: extract_symbol_name(&e.dst),
                            kind: e.kind,
                        })
                        .collect(),
                );
            }
        }

        // Sort symbols within each file for determinism
        for syms in symbols.values_mut() {
            syms.sort();
        }
        for edge_list in edges.values_mut() {
            edge_list.sort();
        }

        // Collect dependencies
        let all_deps = store.get_all_dependencies().unwrap_or_default();
        let mut dependencies: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for dep in all_deps {
            let from_key = normalize_snapshot_path(&dep.from_file);
            let to_key = normalize_snapshot_path(&dep.to_file);
            dependencies.entry(from_key).or_default().push(to_key);
        }
        for deps in dependencies.values_mut() {
            deps.sort();
        }

        WorkspaceSnapshot {
            symbols,
            edges,
            reference_counts,
            dependencies,
        }
    }

    /// Compare with expected snapshot, returning differences
    pub fn diff(&self, expected: &WorkspaceSnapshot) -> Vec<SnapshotDiff> {
        let mut diffs = Vec::new();

        // Compare symbols
        for (file, expected_syms) in &expected.symbols {
            match self.symbols.get(file) {
                None => diffs.push(SnapshotDiff::MissingFile(file.clone())),
                Some(actual_syms) => {
                    for exp in expected_syms {
                        if !actual_syms.contains(exp) {
                            diffs.push(SnapshotDiff::MissingSymbol {
                                file: file.clone(),
                                symbol: exp.name.clone(),
                            });
                        }
                    }
                    for act in actual_syms {
                        if !expected_syms.contains(act) {
                            diffs.push(SnapshotDiff::UnexpectedSymbol {
                                file: file.clone(),
                                symbol: act.name.clone(),
                            });
                        }
                    }
                }
            }
        }

        // Check for files in actual but not in expected
        for file in self.symbols.keys() {
            if !expected.symbols.contains_key(file) {
                diffs.push(SnapshotDiff::UnexpectedFile(file.clone()));
            }
        }

        // Compare reference counts
        for (sym, expected_count) in &expected.reference_counts {
            let actual_count = self.reference_counts.get(sym).copied().unwrap_or(0);
            if actual_count != *expected_count {
                diffs.push(SnapshotDiff::ReferenceCountMismatch {
                    symbol: sym.clone(),
                    expected: *expected_count,
                    actual: actual_count,
                });
            }
        }

        diffs
    }

    /// Check if snapshot contains a symbol with given name and kind
    pub fn has_symbol(&self, name: &str, kind: &str) -> bool {
        self.symbols
            .values()
            .flatten()
            .any(|s| s.name == name && s.kind == kind)
    }

    /// Check if snapshot has a dependency from one file to another
    pub fn has_dependency(&self, from: &str, to: &str) -> bool {
        self.dependencies
            .get(from)
            .map(|deps| deps.iter().any(|d| d.contains(to)))
            .unwrap_or(false)
    }

    /// Get all symbol names
    pub fn symbol_names(&self) -> Vec<&str> {
        self.symbols
            .values()
            .flatten()
            .map(|s| s.name.as_str())
            .collect()
    }

    /// Serialize to YAML for snapshot file storage
    pub fn to_yaml(&self) -> String {
        serde_yaml::to_string(self).unwrap()
    }

    /// Load from YAML snapshot file
    pub fn from_yaml(yaml: &str) -> anyhow::Result<Self> {
        Ok(serde_yaml::from_str(yaml)?)
    }
}

#[derive(Debug)]
pub enum SnapshotDiff {
    MissingFile(String),
    UnexpectedFile(String),
    MissingSymbol {
        file: String,
        symbol: String,
    },
    UnexpectedSymbol {
        file: String,
        symbol: String,
    },
    ReferenceCountMismatch {
        symbol: String,
        expected: usize,
        actual: usize,
    },
}

impl std::fmt::Display for SnapshotDiff {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SnapshotDiff::MissingFile(file) => write!(f, "Missing file: {}", file),
            SnapshotDiff::UnexpectedFile(file) => write!(f, "Unexpected file: {}", file),
            SnapshotDiff::MissingSymbol { file, symbol } => {
                write!(f, "Missing symbol '{}' in {}", symbol, file)
            }
            SnapshotDiff::UnexpectedSymbol { file, symbol } => {
                write!(f, "Unexpected symbol '{}' in {}", symbol, file)
            }
            SnapshotDiff::ReferenceCountMismatch {
                symbol,
                expected,
                actual,
            } => write!(
                f,
                "Reference count mismatch for '{}': expected {}, got {}",
                symbol, expected, actual
            ),
        }
    }
}

/// Strip absolute path prefix, keep only filename
fn normalize_snapshot_path(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

/// Extract symbol name from full ID (best effort)
fn extract_symbol_name(id: &str) -> String {
    // Symbol IDs are like "/path/file.ts#start-end"
    // Try to extract just a meaningful identifier
    id.rsplit('#')
        .next()
        .and_then(|s| s.split('-').next())
        .unwrap_or(id)
        .to_string()
}

/// Macro for inline snapshot testing
#[macro_export]
macro_rules! assert_snapshot {
    ($workspace:expr, $expected:expr) => {
        let actual = $workspace.snapshot();
        let expected: WorkspaceSnapshot =
            serde_yaml::from_str($expected).expect("Failed to parse expected snapshot");
        let diffs = actual.diff(&expected);
        assert!(
            diffs.is_empty(),
            "Snapshot mismatch:\n{}\n\nActual snapshot:\n{}",
            diffs
                .iter()
                .map(|d| d.to_string())
                .collect::<Vec<_>>()
                .join("\n"),
            actual.to_yaml()
        );
    };
}
