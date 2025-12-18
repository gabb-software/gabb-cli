#![allow(dead_code)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use gabb_cli::{indexer, store::IndexStore};
use tempfile::TempDir;

use super::cli::CliRunner;
use super::file_builders::{FileCollector, RsFileBuilder, TsFileBuilder};
use super::fixtures::FixtureDefinition;
use super::snapshot::WorkspaceSnapshot;

/// Content for a test file
#[derive(Clone)]
pub enum FileContent {
    /// Inline content string
    Inline(String),
}

impl FileContent {
    pub fn render(&self) -> String {
        match self {
            FileContent::Inline(s) => s.clone(),
        }
    }
}

/// Builder for creating test workspaces with fluent API
pub struct TestWorkspaceBuilder {
    files: HashMap<PathBuf, FileContent>,
    db_path: Option<PathBuf>,
    auto_index: bool,
}

impl Default for TestWorkspaceBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl TestWorkspaceBuilder {
    pub fn new() -> Self {
        Self {
            files: HashMap::new(),
            db_path: None,
            auto_index: true,
        }
    }

    /// Add a file with inline content
    pub fn with_file(mut self, path: impl AsRef<Path>, content: impl Into<String>) -> Self {
        self.files.insert(
            path.as_ref().to_path_buf(),
            FileContent::Inline(content.into()),
        );
        self
    }

    /// Add a TypeScript file with structured content
    pub fn with_ts_file(self, path: impl AsRef<Path>) -> TsFileBuilder<Self> {
        TsFileBuilder::new(self, path.as_ref().to_path_buf())
    }

    /// Add a Rust file with structured content
    pub fn with_rs_file(self, path: impl AsRef<Path>) -> RsFileBuilder<Self> {
        RsFileBuilder::new(self, path.as_ref().to_path_buf())
    }

    /// Load files from a YAML fixture definition
    #[allow(clippy::wrong_self_convention)]
    pub fn from_fixture(mut self, fixture_name: &str) -> Self {
        match FixtureDefinition::load(fixture_name) {
            Ok(fixture) => {
                for (file_path, content) in fixture.files {
                    self.files
                        .insert(PathBuf::from(file_path), FileContent::Inline(content));
                }
            }
            Err(e) => {
                panic!("Failed to load fixture '{}': {}", fixture_name, e);
            }
        }
        self
    }

    /// Set custom database path (relative to workspace root)
    pub fn with_db_path(mut self, path: impl AsRef<Path>) -> Self {
        self.db_path = Some(path.as_ref().to_path_buf());
        self
    }

    /// Disable automatic indexing (for testing manual index operations)
    pub fn without_auto_index(mut self) -> Self {
        self.auto_index = false;
        self
    }

    /// Build the workspace
    pub fn build(self) -> Result<TestWorkspace> {
        let temp_dir = tempfile::tempdir()?;
        let root = temp_dir.path().to_path_buf();

        // Create all files
        for (path, content) in &self.files {
            let full_path = root.join(path);
            if let Some(parent) = full_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let content_str = content.render();
            std::fs::write(&full_path, content_str)?;
        }

        let db_path = self
            .db_path
            .map(|p| root.join(p))
            .unwrap_or_else(|| root.join(".gabb/index.db"));

        let store = IndexStore::open(&db_path)?;

        if self.auto_index {
            indexer::build_full_index(&root, &store, None::<fn(&indexer::IndexProgress)>)?;
        }

        Ok(TestWorkspace {
            _temp_dir: temp_dir,
            root,
            db_path,
            store,
        })
    }
}

impl FileCollector for TestWorkspaceBuilder {
    fn collect_file(mut self, path: PathBuf, content: FileContent) -> Self {
        self.files.insert(path, content);
        self
    }
}

/// A test workspace with automatic cleanup
pub struct TestWorkspace {
    _temp_dir: TempDir, // Dropped last, cleaning up the directory
    root: PathBuf,
    db_path: PathBuf,
    store: IndexStore,
}

impl TestWorkspace {
    /// Create a new builder
    pub fn builder() -> TestWorkspaceBuilder {
        TestWorkspaceBuilder::new()
    }

    /// Get the workspace root path
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Get the database path
    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    /// Get direct access to the store for assertions
    pub fn store(&self) -> &IndexStore {
        &self.store
    }

    /// Get the CLI binary path
    pub fn cli_bin() -> &'static str {
        env!("CARGO_BIN_EXE_gabb")
    }

    /// Run a CLI command against this workspace
    pub fn cli(&self) -> CliRunner<'_> {
        CliRunner::new(self)
    }

    /// Write/update a file and optionally re-index
    pub fn write_file(&self, path: impl AsRef<Path>, content: impl AsRef<str>) -> Result<()> {
        let full_path = self.root.join(path);
        if let Some(parent) = full_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&full_path, content.as_ref())?;
        Ok(())
    }

    /// Re-index the entire workspace
    pub fn reindex(&self) -> Result<()> {
        indexer::build_full_index(&self.root, &self.store, None::<fn(&indexer::IndexProgress)>)?;
        Ok(())
    }

    /// Index a single file
    pub fn index_file(&self, path: impl AsRef<Path>) -> Result<String> {
        let full_path = self.root.join(path);
        indexer::index_one(&full_path, &self.store)
    }

    /// Get canonical path for a file in the workspace
    pub fn canonical_path(&self, path: impl AsRef<Path>) -> PathBuf {
        let path_ref = path.as_ref();
        self.root
            .join(path_ref)
            .canonicalize()
            .unwrap_or_else(|_| self.root.join(path_ref))
    }

    /// Get path string for a file (canonicalized)
    pub fn path_str(&self, path: impl AsRef<Path>) -> String {
        self.canonical_path(path).to_string_lossy().into_owned()
    }

    /// Create an index snapshot for comparison
    pub fn snapshot(&self) -> WorkspaceSnapshot {
        WorkspaceSnapshot::capture(&self.store)
    }
}
