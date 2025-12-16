use anyhow::Result;
use rusqlite::types::Value;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension};
use serde::Serialize;
use std::cell::RefCell;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
pub struct FileRecord {
    pub path: String,
    pub hash: String,
    pub mtime: i64,
    pub indexed_at: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct SymbolRecord {
    pub id: String,
    pub file: String,
    pub kind: String,
    pub name: String,
    pub start: i64,
    pub end: i64,
    pub qualifier: Option<String>,
    pub visibility: Option<String>,
    pub container: Option<String>,
    /// Blake3 hash of normalized symbol body for duplicate detection
    pub content_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EdgeRecord {
    pub src: String,
    pub dst: String,
    pub kind: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReferenceRecord {
    pub file: String,
    pub start: i64,
    pub end: i64,
    pub symbol_id: String,
}

/// Pre-computed file statistics for O(1) aggregate queries.
/// Used by CLI stats commands and daemon status reporting.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct FileStats {
    pub file: String,
    pub symbol_count: i64,
    pub function_count: i64,
    pub class_count: i64,
    pub interface_count: i64,
}

/// File dependency record for tracking imports/includes.
/// Used by incremental indexing and dependency graph queries.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct FileDependency {
    /// The file that contains the import/use statement
    pub from_file: String,
    /// The file being imported
    pub to_file: String,
    /// Type of dependency (e.g., "import", "use", "include")
    pub kind: String,
}

/// Represents a group of duplicate symbols sharing the same content hash.
#[derive(Debug, Clone)]
pub struct DuplicateGroup {
    pub content_hash: String,
    pub symbols: Vec<SymbolRecord>,
}

use std::collections::HashMap;

/// In-memory dependency cache for O(1) lookups.
/// Caches both forward (file -> dependencies) and reverse (file -> dependents) mappings.
/// Used by daemon for fast invalidation during file watching.
#[allow(dead_code)]
#[derive(Debug, Default)]
pub struct DependencyCache {
    /// Forward dependencies: file -> files it depends on
    forward: HashMap<String, Vec<String>>,
    /// Reverse dependencies: file -> files that depend on it
    reverse: HashMap<String, Vec<String>>,
    /// Whether the cache is populated
    populated: bool,
}

#[allow(dead_code)]
impl DependencyCache {
    /// Create a new empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Check if cache is populated.
    pub fn is_populated(&self) -> bool {
        self.populated
    }

    /// Get files that a file depends on (O(1) lookup).
    pub fn get_dependencies(&self, file: &str) -> Option<&Vec<String>> {
        self.forward.get(file)
    }

    /// Get files that depend on a file (O(1) lookup).
    pub fn get_dependents(&self, file: &str) -> Option<&Vec<String>> {
        self.reverse.get(file)
    }

    /// Clear the cache.
    pub fn clear(&mut self) {
        self.forward.clear();
        self.reverse.clear();
        self.populated = false;
    }

    /// Populate cache from a list of dependencies.
    pub fn populate(&mut self, dependencies: &[FileDependency]) {
        self.clear();

        for dep in dependencies {
            // Forward mapping
            self.forward
                .entry(dep.from_file.clone())
                .or_default()
                .push(dep.to_file.clone());

            // Reverse mapping
            self.reverse
                .entry(dep.to_file.clone())
                .or_default()
                .push(dep.from_file.clone());
        }

        self.populated = true;
    }

    /// Invalidate cache entries for a specific file (when it changes).
    pub fn invalidate_file(&mut self, file: &str) {
        // Remove forward dependencies
        if let Some(deps) = self.forward.remove(file) {
            // Also remove from reverse mappings
            for dep in deps {
                if let Some(rev) = self.reverse.get_mut(&dep) {
                    rev.retain(|f| f != file);
                }
            }
        }

        // Remove reverse dependencies
        if let Some(dependents) = self.reverse.remove(file) {
            // Also remove from forward mappings
            for dependent in dependents {
                if let Some(fwd) = self.forward.get_mut(&dependent) {
                    fwd.retain(|f| f != file);
                }
            }
        }
    }
}

#[derive(Debug)]
pub struct IndexStore {
    conn: RefCell<Connection>,
    db_path: PathBuf,
}

impl IndexStore {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        let store = Self {
            conn: RefCell::new(conn),
            db_path: path.to_path_buf(),
        };
        store.init_schema()?;
        Ok(store)
    }

    fn init_schema(&self) -> Result<()> {
        self.conn.borrow().execute_batch(
            r#"
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = NORMAL;
            PRAGMA cache_size = -64000;
            PRAGMA mmap_size = 268435456;
            PRAGMA page_size = 4096;
            PRAGMA temp_store = MEMORY;
            CREATE TABLE IF NOT EXISTS files (
                path TEXT PRIMARY KEY,
                hash TEXT NOT NULL,
                mtime INTEGER NOT NULL,
                indexed_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS symbols (
                id TEXT PRIMARY KEY,
                file TEXT NOT NULL,
                kind TEXT NOT NULL,
                name TEXT NOT NULL,
                start INTEGER NOT NULL,
                end INTEGER NOT NULL,
                qualifier TEXT,
                visibility TEXT,
                container TEXT,
                content_hash TEXT
            );
            -- B-tree indices for O(log n) lookups
            CREATE INDEX IF NOT EXISTS symbols_file_idx ON symbols(file);
            CREATE INDEX IF NOT EXISTS idx_symbols_name ON symbols(name);
            CREATE INDEX IF NOT EXISTS idx_symbols_position ON symbols(file, start, end);
            CREATE INDEX IF NOT EXISTS idx_symbols_kind_name ON symbols(kind, name);
            CREATE INDEX IF NOT EXISTS idx_symbols_content_hash ON symbols(content_hash);
            -- Compound index for multi-filter queries (file + kind + name)
            CREATE INDEX IF NOT EXISTS idx_symbols_file_kind_name ON symbols(file, kind, name);
            -- Tertiary index for kind + visibility filtered searches
            CREATE INDEX IF NOT EXISTS idx_symbols_kind_visibility ON symbols(kind, visibility);

            CREATE TABLE IF NOT EXISTS edges (
                src TEXT NOT NULL,
                dst TEXT NOT NULL,
                kind TEXT NOT NULL
            );
            -- Covering indices for edges table (include all columns for index-only scans)
            CREATE INDEX IF NOT EXISTS idx_edges_src_covering ON edges(src, dst, kind);
            CREATE INDEX IF NOT EXISTS idx_edges_dst_covering ON edges(dst, src, kind);

            CREATE TABLE IF NOT EXISTS references_tbl (
                file TEXT NOT NULL,
                start INTEGER NOT NULL,
                end INTEGER NOT NULL,
                symbol_id TEXT NOT NULL
            );
            -- Covering index for reference lookups by symbol_id (includes all columns)
            CREATE INDEX IF NOT EXISTS idx_refs_symbol_covering ON references_tbl(symbol_id, file, start, end);
            CREATE INDEX IF NOT EXISTS idx_refs_file_position ON references_tbl(file, start, end, symbol_id);

            -- FTS5 virtual table for full-text symbol search with trigram tokenization
            CREATE VIRTUAL TABLE IF NOT EXISTS symbols_fts USING fts5(
                name,
                qualifier,
                content='symbols',
                content_rowid='rowid',
                tokenize='trigram'
            );

            -- Pre-computed aggregates for instant file statistics
            CREATE TABLE IF NOT EXISTS file_stats (
                file TEXT PRIMARY KEY,
                symbol_count INTEGER NOT NULL DEFAULT 0,
                function_count INTEGER NOT NULL DEFAULT 0,
                class_count INTEGER NOT NULL DEFAULT 0,
                interface_count INTEGER NOT NULL DEFAULT 0
            );

            -- File dependency graph for incremental rebuild ordering
            CREATE TABLE IF NOT EXISTS file_dependencies (
                from_file TEXT NOT NULL,
                to_file TEXT NOT NULL,
                kind TEXT NOT NULL,
                PRIMARY KEY (from_file, to_file)
            );
            -- Index for reverse dependency lookups (find all files that depend on X)
            CREATE INDEX IF NOT EXISTS idx_deps_to_file ON file_dependencies(to_file, from_file);

            -- Triggers to keep FTS5 index in sync with symbols table
            CREATE TRIGGER IF NOT EXISTS symbols_ai AFTER INSERT ON symbols BEGIN
                INSERT INTO symbols_fts(rowid, name, qualifier)
                VALUES (NEW.rowid, NEW.name, NEW.qualifier);
            END;
            CREATE TRIGGER IF NOT EXISTS symbols_ad AFTER DELETE ON symbols BEGIN
                INSERT INTO symbols_fts(symbols_fts, rowid, name, qualifier)
                VALUES ('delete', OLD.rowid, OLD.name, OLD.qualifier);
            END;
            CREATE TRIGGER IF NOT EXISTS symbols_au AFTER UPDATE ON symbols BEGIN
                INSERT INTO symbols_fts(symbols_fts, rowid, name, qualifier)
                VALUES ('delete', OLD.rowid, OLD.name, OLD.qualifier);
                INSERT INTO symbols_fts(rowid, name, qualifier)
                VALUES (NEW.rowid, NEW.name, NEW.qualifier);
            END;
            "#,
        )?;
        self.ensure_column("symbols", "qualifier", "TEXT")?;
        self.ensure_column("symbols", "visibility", "TEXT")?;
        self.ensure_column("symbols", "content_hash", "TEXT")?;
        self.ensure_index(
            "idx_symbols_content_hash",
            "CREATE INDEX IF NOT EXISTS idx_symbols_content_hash ON symbols(content_hash)",
        )?;
        Ok(())
    }

    fn ensure_column(&self, table: &str, column: &str, ty: &str) -> Result<()> {
        let conn = self.conn.borrow();
        let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let name: String = row.get(1)?;
            if name == column {
                return Ok(());
            }
        }
        drop(rows);
        conn.execute(&format!("ALTER TABLE {table} ADD COLUMN {column} {ty}"), [])?;
        Ok(())
    }

    fn ensure_index(&self, index_name: &str, create_sql: &str) -> Result<()> {
        let conn = self.conn.borrow();
        let exists: bool = conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type='index' AND name=?1",
                params![index_name],
                |_| Ok(true),
            )
            .unwrap_or(false);
        if !exists {
            conn.execute(create_sql, [])?;
        }
        Ok(())
    }

    pub fn remove_file<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let path_str = normalize_path(path.as_ref());
        self.conn
            .borrow()
            .execute("DELETE FROM files WHERE path = ?1", params![path_str])?;
        self.conn.borrow().execute(
            "DELETE FROM references_tbl WHERE file = ?1",
            params![path_str.clone()],
        )?;
        self.conn.borrow().execute(
            "DELETE FROM edges WHERE src IN (SELECT id FROM symbols WHERE file = ?1)",
            params![path_str.clone()],
        )?;
        self.conn.borrow().execute(
            "DELETE FROM symbols WHERE file = ?1",
            params![path_str.clone()],
        )?;
        self.conn.borrow().execute(
            "DELETE FROM file_stats WHERE file = ?1",
            params![path_str.clone()],
        )?;
        self.conn.borrow().execute(
            "DELETE FROM file_dependencies WHERE from_file = ?1 OR to_file = ?1",
            params![path_str],
        )?;
        Ok(())
    }

    pub fn list_paths(&self) -> Result<HashSet<String>> {
        let conn = self.conn.borrow();
        let mut stmt = conn.prepare("SELECT path FROM files")?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<HashSet<String>>>()?;
        Ok(rows)
    }

    pub fn save_file_index(
        &self,
        file_record: &FileRecord,
        symbols: &[SymbolRecord],
        edges: &[EdgeRecord],
        references: &[ReferenceRecord],
    ) -> Result<()> {
        let conn = &mut *self.conn.borrow_mut();
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM references_tbl WHERE file = ?1",
            params![file_record.path.clone()],
        )?;
        tx.execute(
            "DELETE FROM edges WHERE src IN (SELECT id FROM symbols WHERE file = ?1)",
            params![file_record.path.clone()],
        )?;
        tx.execute(
            "DELETE FROM symbols WHERE file = ?1",
            params![file_record.path.clone()],
        )?;

        for sym in symbols {
            tx.execute(
                "INSERT INTO symbols(id, file, kind, name, start, end, qualifier, visibility, container, content_hash) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    sym.id,
                    sym.file,
                    sym.kind,
                    sym.name,
                    sym.start,
                    sym.end,
                    sym.qualifier,
                    sym.visibility,
                    sym.container,
                    sym.content_hash
                ],
            )?;
        }

        for edge in edges {
            tx.execute(
                "INSERT INTO edges(src, dst, kind) VALUES (?1, ?2, ?3)",
                params![edge.src, edge.dst, edge.kind],
            )?;
        }

        for r in references {
            tx.execute(
                "INSERT INTO references_tbl(file, start, end, symbol_id) VALUES (?1, ?2, ?3, ?4)",
                params![r.file, r.start, r.end, r.symbol_id],
            )?;
        }

        tx.execute(
            r#"
            INSERT INTO files(path, hash, mtime, indexed_at)
            VALUES (?1, ?2, ?3, ?4)
            ON CONFLICT(path) DO UPDATE SET
                hash=excluded.hash,
                mtime=excluded.mtime,
                indexed_at=excluded.indexed_at
            "#,
            params![
                file_record.path,
                file_record.hash,
                file_record.mtime,
                file_record.indexed_at
            ],
        )?;

        // Update pre-computed aggregates for file statistics
        let symbol_count = symbols.len() as i64;
        let function_count = symbols.iter().filter(|s| s.kind == "function").count() as i64;
        let class_count = symbols.iter().filter(|s| s.kind == "class").count() as i64;
        let interface_count = symbols.iter().filter(|s| s.kind == "interface").count() as i64;

        tx.execute(
            r#"
            INSERT INTO file_stats(file, symbol_count, function_count, class_count, interface_count)
            VALUES (?1, ?2, ?3, ?4, ?5)
            ON CONFLICT(file) DO UPDATE SET
                symbol_count = excluded.symbol_count,
                function_count = excluded.function_count,
                class_count = excluded.class_count,
                interface_count = excluded.interface_count
            "#,
            params![
                file_record.path,
                symbol_count,
                function_count,
                class_count,
                interface_count
            ],
        )?;

        tx.commit()?;
        Ok(())
    }

    /// Save file index without references (used in two-phase indexing first pass)
    pub fn save_file_index_without_refs(
        &self,
        file_record: &FileRecord,
        symbols: &[SymbolRecord],
        edges: &[EdgeRecord],
    ) -> Result<()> {
        let conn = &mut *self.conn.borrow_mut();
        let tx = conn.transaction()?;

        // Clear existing data for this file
        tx.execute(
            "DELETE FROM references_tbl WHERE file = ?1",
            params![file_record.path.clone()],
        )?;
        tx.execute(
            "DELETE FROM edges WHERE src IN (SELECT id FROM symbols WHERE file = ?1)",
            params![file_record.path.clone()],
        )?;
        tx.execute(
            "DELETE FROM symbols WHERE file = ?1",
            params![file_record.path.clone()],
        )?;

        for sym in symbols {
            tx.execute(
                "INSERT INTO symbols(id, file, kind, name, start, end, qualifier, visibility, container, content_hash) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    sym.id,
                    sym.file,
                    sym.kind,
                    sym.name,
                    sym.start,
                    sym.end,
                    sym.qualifier,
                    sym.visibility,
                    sym.container,
                    sym.content_hash
                ],
            )?;
        }

        for edge in edges {
            tx.execute(
                "INSERT INTO edges(src, dst, kind) VALUES (?1, ?2, ?3)",
                params![edge.src, edge.dst, edge.kind],
            )?;
        }

        tx.execute(
            r#"
            INSERT INTO files(path, hash, mtime, indexed_at)
            VALUES (?1, ?2, ?3, ?4)
            ON CONFLICT(path) DO UPDATE SET
                hash=excluded.hash,
                mtime=excluded.mtime,
                indexed_at=excluded.indexed_at
            "#,
            params![
                file_record.path,
                file_record.hash,
                file_record.mtime,
                file_record.indexed_at
            ],
        )?;

        // Update pre-computed aggregates for file statistics
        let symbol_count = symbols.len() as i64;
        let function_count = symbols.iter().filter(|s| s.kind == "function").count() as i64;
        let class_count = symbols.iter().filter(|s| s.kind == "class").count() as i64;
        let interface_count = symbols.iter().filter(|s| s.kind == "interface").count() as i64;

        tx.execute(
            r#"
            INSERT INTO file_stats(file, symbol_count, function_count, class_count, interface_count)
            VALUES (?1, ?2, ?3, ?4, ?5)
            ON CONFLICT(file) DO UPDATE SET
                symbol_count = excluded.symbol_count,
                function_count = excluded.function_count,
                class_count = excluded.class_count,
                interface_count = excluded.interface_count
            "#,
            params![
                file_record.path,
                symbol_count,
                function_count,
                class_count,
                interface_count
            ],
        )?;

        tx.commit()?;
        Ok(())
    }

    /// Save resolved references for a file (used in two-phase indexing second pass)
    pub fn save_references(&self, file_path: &str, references: &[ReferenceRecord]) -> Result<()> {
        let conn = &mut *self.conn.borrow_mut();
        let tx = conn.transaction()?;

        // Clear existing references for this file (in case of re-indexing)
        tx.execute(
            "DELETE FROM references_tbl WHERE file = ?1",
            params![file_path],
        )?;

        for r in references {
            tx.execute(
                "INSERT INTO references_tbl(file, start, end, symbol_id) VALUES (?1, ?2, ?3, ?4)",
                params![r.file, r.start, r.end, r.symbol_id],
            )?;
        }

        tx.commit()?;
        Ok(())
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    /// Update query optimizer statistics for better index usage.
    /// Should be called after bulk indexing operations.
    pub fn analyze(&self) -> Result<()> {
        self.conn.borrow().execute_batch("ANALYZE")?;
        Ok(())
    }

    /// Get pre-computed statistics for a file (O(1) lookup).
    #[allow(dead_code)]
    pub fn get_file_stats(&self, file: &str) -> Result<Option<FileStats>> {
        let file_norm = normalize_path(Path::new(file));
        let conn = self.conn.borrow();
        let mut stmt = conn.prepare(
            "SELECT file, symbol_count, function_count, class_count, interface_count FROM file_stats WHERE file = ?1",
        )?;
        let mut rows = stmt.query(params![file_norm])?;
        if let Some(row) = rows.next()? {
            Ok(Some(FileStats {
                file: row.get(0)?,
                symbol_count: row.get(1)?,
                function_count: row.get(2)?,
                class_count: row.get(3)?,
                interface_count: row.get(4)?,
            }))
        } else {
            Ok(None)
        }
    }

    /// Get total symbol counts across all indexed files (O(1) aggregate).
    #[allow(dead_code)]
    pub fn get_total_stats(&self) -> Result<FileStats> {
        let conn = self.conn.borrow();
        let mut stmt = conn.prepare(
            "SELECT COALESCE(SUM(symbol_count), 0), COALESCE(SUM(function_count), 0), COALESCE(SUM(class_count), 0), COALESCE(SUM(interface_count), 0) FROM file_stats",
        )?;
        let mut rows = stmt.query([])?;
        let row = rows.next()?.expect("aggregate query always returns a row");
        Ok(FileStats {
            file: "".into(),
            symbol_count: row.get(0)?,
            function_count: row.get(1)?,
            class_count: row.get(2)?,
            interface_count: row.get(3)?,
        })
    }

    /// Save file dependencies for a source file, replacing any existing dependencies.
    #[allow(dead_code)]
    pub fn save_file_dependencies(
        &self,
        from_file: &str,
        dependencies: &[FileDependency],
    ) -> Result<()> {
        let from_norm = normalize_path(Path::new(from_file));
        let conn = &mut *self.conn.borrow_mut();
        let tx = conn.transaction()?;

        // Remove existing dependencies for this file
        tx.execute(
            "DELETE FROM file_dependencies WHERE from_file = ?1",
            params![from_norm],
        )?;

        // Insert new dependencies
        for dep in dependencies {
            tx.execute(
                "INSERT OR REPLACE INTO file_dependencies(from_file, to_file, kind) VALUES (?1, ?2, ?3)",
                params![from_norm, normalize_path(Path::new(&dep.to_file)), dep.kind],
            )?;
        }

        tx.commit()?;
        Ok(())
    }

    /// Get files that a given file depends on (imports/uses).
    #[allow(dead_code)]
    pub fn get_file_dependencies(&self, file: &str) -> Result<Vec<FileDependency>> {
        let file_norm = normalize_path(Path::new(file));
        let conn = self.conn.borrow();
        let mut stmt = conn.prepare_cached(
            "SELECT from_file, to_file, kind FROM file_dependencies WHERE from_file = ?1",
        )?;
        let rows = stmt
            .query_map(params![file_norm], |row| {
                Ok(FileDependency {
                    from_file: row.get(0)?,
                    to_file: row.get(1)?,
                    kind: row.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Get files that depend on a given file (reverse dependencies for invalidation).
    #[allow(dead_code)]
    pub fn get_dependents(&self, file: &str) -> Result<Vec<String>> {
        let file_norm = normalize_path(Path::new(file));
        let conn = self.conn.borrow();
        let mut stmt =
            conn.prepare_cached("SELECT from_file FROM file_dependencies WHERE to_file = ?1")?;
        let rows = stmt
            .query_map(params![file_norm], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Get all file dependencies in the workspace.
    #[allow(dead_code)]
    pub fn get_all_dependencies(&self) -> Result<Vec<FileDependency>> {
        let conn = self.conn.borrow();
        let mut stmt = conn.prepare("SELECT from_file, to_file, kind FROM file_dependencies")?;
        let rows = stmt
            .query_map([], |row| {
                Ok(FileDependency {
                    from_file: row.get(0)?,
                    to_file: row.get(1)?,
                    kind: row.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Topologically sort files for rebuild ordering.
    /// Returns files in an order where dependencies come before dependents.
    /// Uses Kahn's algorithm with O(V + E) complexity.
    /// Files with cycles are appended at the end in arbitrary order.
    #[allow(dead_code)]
    pub fn topological_sort(&self, files: &[String]) -> Result<Vec<String>> {
        use std::collections::{HashMap, VecDeque};

        if files.is_empty() {
            return Ok(Vec::new());
        }

        // Build adjacency list and in-degree count for the subgraph
        let file_set: HashSet<String> = files.iter().cloned().collect();
        let mut in_degree: HashMap<String, usize> = HashMap::new();
        let mut adjacency: HashMap<String, Vec<String>> = HashMap::new();

        // Initialize all files with 0 in-degree
        for file in files {
            in_degree.entry(file.clone()).or_insert(0);
            adjacency.entry(file.clone()).or_default();
        }

        // Build graph from dependencies (only within the file set)
        for file in files {
            let deps = self.get_file_dependencies(file)?;
            for dep in deps {
                // Only count edges where both files are in our set
                if file_set.contains(&dep.to_file) {
                    // from_file depends on to_file, so to_file -> from_file edge
                    adjacency
                        .entry(dep.to_file.clone())
                        .or_default()
                        .push(file.clone());
                    *in_degree.entry(file.clone()).or_insert(0) += 1;
                }
            }
        }

        // Kahn's algorithm
        let mut queue: VecDeque<String> = VecDeque::new();
        let mut result = Vec::new();

        // Start with nodes that have no dependencies (in-degree 0)
        for (file, &degree) in &in_degree {
            if degree == 0 {
                queue.push_back(file.clone());
            }
        }

        while let Some(file) = queue.pop_front() {
            result.push(file.clone());

            if let Some(dependents) = adjacency.get(&file) {
                for dependent in dependents {
                    if let Some(degree) = in_degree.get_mut(dependent) {
                        *degree -= 1;
                        if *degree == 0 {
                            queue.push_back(dependent.clone());
                        }
                    }
                }
            }
        }

        // Handle any remaining files (cycles) by appending them
        for file in files {
            if !result.contains(file) {
                result.push(file.clone());
            }
        }

        Ok(result)
    }

    /// Get all files that need to be invalidated when a file changes.
    /// Returns the transitive closure of reverse dependencies.
    /// Useful for incremental rebuilds when a source file is modified.
    #[allow(dead_code)]
    pub fn get_invalidation_set(&self, changed_file: &str) -> Result<Vec<String>> {
        let file_norm = normalize_path(Path::new(changed_file));
        let mut visited = HashSet::new();
        let mut to_visit = vec![file_norm.clone()];
        let mut result = Vec::new();

        while let Some(file) = to_visit.pop() {
            if visited.contains(&file) {
                continue;
            }
            visited.insert(file.clone());
            result.push(file.clone());

            // Get all files that depend on this file
            let dependents = self.get_dependents(&file)?;
            for dependent in dependents {
                if !visited.contains(&dependent) {
                    to_visit.push(dependent);
                }
            }
        }

        // Sort topologically for proper rebuild order
        self.topological_sort(&result)
    }

    /// Get files that need invalidation for multiple changed files.
    /// Returns the union of invalidation sets, topologically sorted.
    #[allow(dead_code)]
    pub fn get_batch_invalidation_set(&self, changed_files: &[String]) -> Result<Vec<String>> {
        let mut all_files = HashSet::new();

        for file in changed_files {
            let invalidated = self.get_invalidation_set(file)?;
            all_files.extend(invalidated);
        }

        let files: Vec<String> = all_files.into_iter().collect();
        self.topological_sort(&files)
    }

    /// Load all dependencies into a DependencyCache for O(1) lookups.
    /// Call this once at startup for long-running processes.
    #[allow(dead_code)]
    pub fn load_dependency_cache(&self) -> Result<DependencyCache> {
        let deps = self.get_all_dependencies()?;
        let mut cache = DependencyCache::new();
        cache.populate(&deps);
        Ok(cache)
    }

    pub fn list_symbols(
        &self,
        file: Option<&str>,
        kind: Option<&str>,
        name: Option<&str>,
        limit: Option<usize>,
    ) -> Result<Vec<SymbolRecord>> {
        let file_norm = file.map(|f| normalize_path(Path::new(f)));
        let mut sql = String::from(
            "SELECT id, file, kind, name, start, end, qualifier, visibility, container, content_hash FROM symbols",
        );
        let mut values: Vec<Value> = Vec::new();
        let mut clauses: Vec<&str> = Vec::new();

        if let Some(f) = file_norm {
            clauses.push("file = ?");
            values.push(Value::from(f));
        }

        if let Some(k) = kind {
            clauses.push("kind = ?");
            values.push(Value::from(k.to_string()));
        }

        if let Some(n) = name {
            clauses.push("name = ?");
            values.push(Value::from(n.to_string()));
        }

        if !clauses.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&clauses.join(" AND "));
        }

        if let Some(lim) = limit {
            sql.push_str(" LIMIT ?");
            values.push(Value::from(lim as i64));
        }

        let conn = self.conn.borrow();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt
            .query_map(params_from_iter(values.iter()), |row| {
                Ok(SymbolRecord {
                    id: row.get(0)?,
                    file: row.get(1)?,
                    kind: row.get(2)?,
                    name: row.get(3)?,
                    start: row.get(4)?,
                    end: row.get(5)?,
                    qualifier: row.get(6)?,
                    visibility: row.get(7)?,
                    container: row.get(8)?,
                    content_hash: row.get(9)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Query edges by destination with cached prepared statement.
    pub fn edges_to(&self, dst: &str) -> Result<Vec<EdgeRecord>> {
        let conn = self.conn.borrow();
        let mut stmt = conn.prepare_cached("SELECT src, dst, kind FROM edges WHERE dst = ?1")?;
        let edges = stmt
            .query_map(params![dst], |row| {
                Ok(EdgeRecord {
                    src: row.get(0)?,
                    dst: row.get(1)?,
                    kind: row.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(edges)
    }

    /// Query edges by source with cached prepared statement.
    pub fn edges_from(&self, src: &str) -> Result<Vec<EdgeRecord>> {
        let conn = self.conn.borrow();
        let mut stmt = conn.prepare_cached("SELECT src, dst, kind FROM edges WHERE src = ?1")?;
        let edges = stmt
            .query_map(params![src], |row| {
                Ok(EdgeRecord {
                    src: row.get(0)?,
                    dst: row.get(1)?,
                    kind: row.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(edges)
    }

    pub fn symbols_by_ids(&self, ids: &[String]) -> Result<Vec<SymbolRecord>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = std::iter::repeat_n("?", ids.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT id, file, kind, name, start, end, qualifier, visibility, container, content_hash FROM symbols WHERE id IN ({})",
            placeholders
        );
        let conn = self.conn.borrow();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt
            .query_map(params_from_iter(ids.iter()), |row| {
                Ok(SymbolRecord {
                    id: row.get(0)?,
                    file: row.get(1)?,
                    kind: row.get(2)?,
                    name: row.get(3)?,
                    start: row.get(4)?,
                    end: row.get(5)?,
                    qualifier: row.get(6)?,
                    visibility: row.get(7)?,
                    container: row.get(8)?,
                    content_hash: row.get(9)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Query references by symbol ID with cached prepared statement.
    pub fn references_for_symbol(&self, symbol_id: &str) -> Result<Vec<ReferenceRecord>> {
        let conn = self.conn.borrow();
        let mut stmt = conn.prepare_cached(
            "SELECT file, start, end, symbol_id FROM references_tbl WHERE symbol_id = ?1",
        )?;
        let rows = stmt
            .query_map(params![symbol_id], |row| {
                Ok(ReferenceRecord {
                    file: row.get(0)?,
                    start: row.get(1)?,
                    end: row.get(2)?,
                    symbol_id: row.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Find a reference at a specific file and byte offset.
    /// Returns the reference record if the offset falls within a recorded reference span.
    pub fn reference_at_position(
        &self,
        file: &str,
        offset: i64,
    ) -> Result<Option<ReferenceRecord>> {
        let conn = self.conn.borrow();
        let mut stmt = conn.prepare_cached(
            "SELECT file, start, end, symbol_id FROM references_tbl
             WHERE file = ?1 AND start <= ?2 AND end > ?2
             ORDER BY (end - start) ASC
             LIMIT 1",
        )?;
        let result = stmt
            .query_row(params![file, offset], |row| {
                Ok(ReferenceRecord {
                    file: row.get(0)?,
                    start: row.get(1)?,
                    end: row.get(2)?,
                    symbol_id: row.get(3)?,
                })
            })
            .optional()?;
        Ok(result)
    }

    /// Find all groups of duplicate symbols (symbols with the same content_hash).
    /// Returns groups sorted by count (most duplicates first).
    /// Only includes groups with 2+ symbols and content_hash is not null.
    pub fn find_duplicate_groups(
        &self,
        min_count: usize,
        kind_filter: Option<&str>,
        file_filter: Option<&[String]>,
    ) -> Result<Vec<DuplicateGroup>> {
        let conn = self.conn.borrow();

        // First, find all content_hashes with duplicates
        let mut sql = String::from(
            "SELECT content_hash, COUNT(*) as cnt FROM symbols
             WHERE content_hash IS NOT NULL",
        );
        let mut values: Vec<Value> = Vec::new();

        if let Some(kind) = kind_filter {
            sql.push_str(" AND kind = ?");
            values.push(Value::from(kind.to_string()));
        }

        if let Some(files) = file_filter {
            if !files.is_empty() {
                let placeholders = std::iter::repeat_n("?", files.len())
                    .collect::<Vec<_>>()
                    .join(", ");
                sql.push_str(&format!(" AND file IN ({})", placeholders));
                for f in files {
                    values.push(Value::from(f.clone()));
                }
            }
        }

        sql.push_str(" GROUP BY content_hash HAVING COUNT(*) >= ?");
        values.push(Value::from(min_count as i64));
        sql.push_str(" ORDER BY cnt DESC");

        let mut stmt = conn.prepare(&sql)?;
        let hashes: Vec<String> = stmt
            .query_map(params_from_iter(values.iter()), |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        // Now fetch symbols for each hash
        let mut groups = Vec::new();
        for hash in hashes {
            let symbols = self.symbols_by_content_hash(&hash)?;
            if symbols.len() >= min_count {
                groups.push(DuplicateGroup {
                    content_hash: hash,
                    symbols,
                });
            }
        }

        Ok(groups)
    }

    /// Find all symbols with a specific content hash.
    pub fn symbols_by_content_hash(&self, hash: &str) -> Result<Vec<SymbolRecord>> {
        let conn = self.conn.borrow();
        let mut stmt = conn.prepare_cached(
            "SELECT id, file, kind, name, start, end, qualifier, visibility, container, content_hash
             FROM symbols WHERE content_hash = ?1"
        )?;
        let rows = stmt
            .query_map(params![hash], |row| {
                Ok(SymbolRecord {
                    id: row.get(0)?,
                    file: row.get(1)?,
                    kind: row.get(2)?,
                    name: row.get(3)?,
                    start: row.get(4)?,
                    end: row.get(5)?,
                    qualifier: row.get(6)?,
                    visibility: row.get(7)?,
                    container: row.get(8)?,
                    content_hash: row.get(9)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Get content hashes for symbols in specific files.
    /// Used for --uncommitted flag to find duplicates involving changed files.
    #[allow(dead_code)]
    pub fn content_hashes_in_files(&self, files: &[String]) -> Result<HashSet<String>> {
        if files.is_empty() {
            return Ok(HashSet::new());
        }
        let conn = self.conn.borrow();
        let placeholders = std::iter::repeat_n("?", files.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT DISTINCT content_hash FROM symbols WHERE file IN ({}) AND content_hash IS NOT NULL",
            placeholders
        );
        let mut stmt = conn.prepare(&sql)?;
        let hashes: HashSet<String> = stmt
            .query_map(params_from_iter(files.iter()), |row| row.get(0))?
            .collect::<rusqlite::Result<HashSet<_>>>()?;
        Ok(hashes)
    }

    /// Search symbols using FTS5 full-text search.
    /// Supports prefix queries (e.g., "getUser*") and substring matching via trigram tokenization.
    /// Uses cached prepared statement for repeated searches.
    #[allow(dead_code)]
    pub fn search_symbols_fts(&self, query: &str) -> Result<Vec<SymbolRecord>> {
        let conn = self.conn.borrow();
        let mut stmt = conn.prepare_cached(
            r#"
            SELECT s.id, s.file, s.kind, s.name, s.start, s.end, s.qualifier, s.visibility, s.container, s.content_hash
            FROM symbols s
            JOIN symbols_fts fts ON s.rowid = fts.rowid
            WHERE symbols_fts MATCH ?1
            ORDER BY rank
            "#,
        )?;
        let rows = stmt
            .query_map(params![query], |row| {
                Ok(SymbolRecord {
                    id: row.get(0)?,
                    file: row.get(1)?,
                    kind: row.get(2)?,
                    name: row.get(3)?,
                    start: row.get(4)?,
                    end: row.get(5)?,
                    qualifier: row.get(6)?,
                    visibility: row.get(7)?,
                    container: row.get(8)?,
                    content_hash: row.get(9)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Query symbols with cursor-based pagination for streaming large result sets.
    /// Returns (results, next_cursor) where next_cursor can be used to fetch the next page.
    #[allow(dead_code)]
    pub fn list_symbols_paginated(
        &self,
        file: Option<&str>,
        kind: Option<&str>,
        name: Option<&str>,
        cursor: Option<&str>,
        page_size: usize,
    ) -> Result<(Vec<SymbolRecord>, Option<String>)> {
        let file_norm = file.map(|f| normalize_path(Path::new(f)));
        let mut sql = String::from(
            "SELECT id, file, kind, name, start, end, qualifier, visibility, container, content_hash FROM symbols",
        );
        let mut values: Vec<Value> = Vec::new();
        let mut clauses: Vec<&str> = Vec::new();

        if let Some(f) = &file_norm {
            clauses.push("file = ?");
            values.push(Value::from(f.clone()));
        }

        if let Some(k) = kind {
            clauses.push("kind = ?");
            values.push(Value::from(k.to_string()));
        }

        if let Some(n) = name {
            clauses.push("name = ?");
            values.push(Value::from(n.to_string()));
        }

        // Cursor-based pagination using id as cursor (keyset pagination)
        if let Some(c) = cursor {
            clauses.push("id > ?");
            values.push(Value::from(c.to_string()));
        }

        if !clauses.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&clauses.join(" AND "));
        }

        // Order by id for consistent pagination
        sql.push_str(" ORDER BY id");

        // Fetch one extra to determine if there's a next page
        sql.push_str(" LIMIT ?");
        values.push(Value::from((page_size + 1) as i64));

        let conn = self.conn.borrow();
        let mut stmt = conn.prepare(&sql)?;
        let mut rows: Vec<SymbolRecord> = stmt
            .query_map(params_from_iter(values.iter()), |row| {
                Ok(SymbolRecord {
                    id: row.get(0)?,
                    file: row.get(1)?,
                    kind: row.get(2)?,
                    name: row.get(3)?,
                    start: row.get(4)?,
                    end: row.get(5)?,
                    qualifier: row.get(6)?,
                    visibility: row.get(7)?,
                    container: row.get(8)?,
                    content_hash: row.get(9)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        // Determine next cursor
        let next_cursor = if rows.len() > page_size {
            rows.pop(); // Remove the extra row
            rows.last().map(|r| r.id.clone())
        } else {
            None
        };

        Ok((rows, next_cursor))
    }
}

pub fn normalize_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

pub fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn mk_file_record(path: &Path) -> FileRecord {
        FileRecord {
            path: normalize_path(path),
            hash: "abc".into(),
            mtime: 0,
            indexed_at: now_unix(),
        }
    }

    fn mk_symbol(path: &Path, name: &str) -> SymbolRecord {
        SymbolRecord {
            id: format!("{}#0-1", normalize_path(path)),
            file: normalize_path(path),
            kind: "function".into(),
            name: name.into(),
            start: 0,
            end: 1,
            qualifier: None,
            visibility: None,
            container: None,
            content_hash: None,
        }
    }

    #[test]
    fn store_roundtrip_save_list_and_remove() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("index.db");
        let store = IndexStore::open(&db_path).unwrap();

        let file_path = dir.path().join("foo.ts");
        let file_rec = mk_file_record(&file_path);
        let sym = mk_symbol(&file_path, "hello");
        let edges = vec![EdgeRecord {
            src: sym.id.clone(),
            dst: "target".into(),
            kind: "implements".into(),
        }];
        let refs = vec![ReferenceRecord {
            file: sym.file.clone(),
            start: 0,
            end: 1,
            symbol_id: sym.id.clone(),
        }];

        store
            .save_file_index(&file_rec, std::slice::from_ref(&sym), &edges, &refs)
            .unwrap();

        let paths = store.list_paths().unwrap();
        assert!(paths.contains(&file_rec.path));

        let symbols = store
            .list_symbols(Some(&file_rec.path), None, Some("hello"), None)
            .unwrap();
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "hello");

        let edges_back = store.edges_to("target").unwrap();
        assert_eq!(edges_back.len(), 1);
        assert_eq!(edges_back[0].src, sym.id);

        let edges_out = store.edges_from(&sym.id).unwrap();
        assert_eq!(edges_out.len(), 1);
        assert_eq!(edges_out[0].dst, "target");

        store.remove_file(&file_path).unwrap();
        let paths_after = store.list_paths().unwrap();
        assert!(!paths_after.contains(&file_rec.path));
    }

    /// Test that B-tree indices exist for O(log n) lookups
    #[test]
    fn btree_indices_exist() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("index.db");
        let store = IndexStore::open(&db_path).unwrap();

        let conn = store.conn.borrow();
        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='index' ORDER BY name")
            .unwrap();
        let indices: Vec<String> = stmt
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();

        // Verify critical indices for O(log n) performance exist
        assert!(
            indices.iter().any(|n| n == "idx_symbols_name"),
            "Missing idx_symbols_name index for symbol name lookups. Found: {:?}",
            indices
        );
        assert!(
            indices.iter().any(|n| n == "idx_symbols_position"),
            "Missing idx_symbols_position index for position queries. Found: {:?}",
            indices
        );
        assert!(
            indices.iter().any(|n| n == "idx_symbols_kind_name"),
            "Missing idx_symbols_kind_name compound index. Found: {:?}",
            indices
        );
        assert!(
            indices.iter().any(|n| n == "idx_refs_file_position"),
            "Missing idx_refs_file_position index for reference lookups. Found: {:?}",
            indices
        );
        // Verify covering indices for index-only scans
        assert!(
            indices.iter().any(|n| n == "idx_edges_src_covering"),
            "Missing idx_edges_src_covering covering index. Found: {:?}",
            indices
        );
        assert!(
            indices.iter().any(|n| n == "idx_edges_dst_covering"),
            "Missing idx_edges_dst_covering covering index. Found: {:?}",
            indices
        );
        assert!(
            indices.iter().any(|n| n == "idx_refs_symbol_covering"),
            "Missing idx_refs_symbol_covering covering index. Found: {:?}",
            indices
        );
        // Verify compound index for multi-filter queries
        assert!(
            indices.iter().any(|n| n == "idx_symbols_file_kind_name"),
            "Missing idx_symbols_file_kind_name compound index. Found: {:?}",
            indices
        );
        // Verify tertiary index for kind+visibility
        assert!(
            indices.iter().any(|n| n == "idx_symbols_kind_visibility"),
            "Missing idx_symbols_kind_visibility tertiary index. Found: {:?}",
            indices
        );
    }

    /// Test that symbol name lookup uses the index (O(log n))
    #[test]
    fn symbol_name_lookup_uses_index() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("index.db");
        let store = IndexStore::open(&db_path).unwrap();

        let conn = store.conn.borrow();
        let mut stmt = conn
            .prepare("EXPLAIN QUERY PLAN SELECT * FROM symbols WHERE name = ?")
            .unwrap();
        let plan: String = stmt
            .query_map(["test"], |row| row.get::<_, String>(3))
            .unwrap()
            .next()
            .unwrap()
            .unwrap();

        assert!(
            plan.contains("idx_symbols_name") || plan.contains("USING INDEX"),
            "Symbol name lookup not using index. Query plan: {}",
            plan
        );
    }

    /// Test that position-based symbol lookup uses covering index
    #[test]
    fn position_lookup_uses_covering_index() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("index.db");
        let store = IndexStore::open(&db_path).unwrap();

        let conn = store.conn.borrow();
        let mut stmt = conn
            .prepare(
                "EXPLAIN QUERY PLAN SELECT * FROM symbols WHERE file = ? AND start <= ? AND end >= ?",
            )
            .unwrap();
        let plan: String = stmt
            .query_map(["test.ts", "100", "100"], |row| row.get::<_, String>(3))
            .unwrap()
            .next()
            .unwrap()
            .unwrap();

        assert!(
            plan.contains("idx_symbols_position") || plan.contains("USING INDEX"),
            "Position lookup not using index. Query plan: {}",
            plan
        );
    }

    /// Test that ANALYZE updates optimizer statistics
    #[test]
    fn analyze_updates_statistics() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("index.db");
        let store = IndexStore::open(&db_path).unwrap();

        // Insert some test data
        let file_path = dir.path().join("test.ts");
        let file_rec = mk_file_record(&file_path);
        let symbols: Vec<SymbolRecord> = (0..100)
            .map(|i| SymbolRecord {
                id: format!("sym_{}", i),
                file: normalize_path(&file_path),
                kind: "function".into(),
                name: format!("func_{}", i),
                start: i * 10,
                end: i * 10 + 5,
                qualifier: None,
                visibility: None,
                container: None,
                content_hash: None,
            })
            .collect();

        store
            .save_file_index(&file_rec, &symbols, &[], &[])
            .unwrap();

        // Run ANALYZE
        store.analyze().unwrap();

        // Verify sqlite_stat1 table exists and has data
        let conn = store.conn.borrow();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM sqlite_stat1", [], |row| row.get(0))
            .unwrap();
        assert!(count > 0, "ANALYZE should populate sqlite_stat1 table");
    }

    /// Test O(log n) performance characteristic by verifying index usage on filtered queries
    #[test]
    fn filtered_queries_use_compound_index() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("index.db");
        let store = IndexStore::open(&db_path).unwrap();

        let conn = store.conn.borrow();

        // Test kind+name compound index
        let mut stmt = conn
            .prepare("EXPLAIN QUERY PLAN SELECT * FROM symbols WHERE kind = ? AND name = ?")
            .unwrap();
        let plan: String = stmt
            .query_map(["function", "test"], |row| row.get::<_, String>(3))
            .unwrap()
            .next()
            .unwrap()
            .unwrap();

        assert!(
            plan.contains("idx_symbols_kind_name") || plan.contains("USING INDEX"),
            "Kind+name query not using compound index. Query plan: {}",
            plan
        );
    }

    /// Test that reference lookups use covering index for symbol_id queries
    #[test]
    fn reference_symbol_lookup_uses_covering_index() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("index.db");
        let store = IndexStore::open(&db_path).unwrap();

        let conn = store.conn.borrow();

        // Query for references by symbol_id - this should use the covering index
        let mut stmt = conn
            .prepare(
                "EXPLAIN QUERY PLAN SELECT file, start, end, symbol_id FROM references_tbl WHERE symbol_id = ?",
            )
            .unwrap();
        let plan: String = stmt
            .query_map(["test_sym"], |row| row.get::<_, String>(3))
            .unwrap()
            .next()
            .unwrap()
            .unwrap();

        // Must show "COVERING INDEX" to avoid table lookups
        assert!(
            plan.contains("COVERING INDEX"),
            "Reference symbol lookup must use COVERING INDEX to avoid table lookups. Query plan: {}",
            plan
        );
    }

    /// Test that edges lookup by dst achieves index-only scan
    #[test]
    fn edges_dst_lookup_uses_covering_index() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("index.db");
        let store = IndexStore::open(&db_path).unwrap();

        let conn = store.conn.borrow();

        // Query edges by dst - should use covering index
        let mut stmt = conn
            .prepare("EXPLAIN QUERY PLAN SELECT src, dst, kind FROM edges WHERE dst = ?")
            .unwrap();
        let plan: String = stmt
            .query_map(["target"], |row| row.get::<_, String>(3))
            .unwrap()
            .next()
            .unwrap()
            .unwrap();

        // Must show "COVERING INDEX" to avoid table lookups
        assert!(
            plan.contains("COVERING INDEX"),
            "Edges dst lookup must use COVERING INDEX to avoid table lookups. Query plan: {}",
            plan
        );
    }

    /// Test that edges lookup by src achieves index-only scan
    #[test]
    fn edges_src_lookup_uses_covering_index() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("index.db");
        let store = IndexStore::open(&db_path).unwrap();

        let conn = store.conn.borrow();

        // Query edges by src - should use covering index
        let mut stmt = conn
            .prepare("EXPLAIN QUERY PLAN SELECT src, dst, kind FROM edges WHERE src = ?")
            .unwrap();
        let plan: String = stmt
            .query_map(["source"], |row| row.get::<_, String>(3))
            .unwrap()
            .next()
            .unwrap()
            .unwrap();

        // Must show "COVERING INDEX" to avoid table lookups
        assert!(
            plan.contains("COVERING INDEX"),
            "Edges src lookup must use COVERING INDEX to avoid table lookups. Query plan: {}",
            plan
        );
    }

    /// Test compound index for file+name queries (common pattern in resolve_symbol_at)
    #[test]
    fn file_and_name_query_uses_index() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("index.db");
        let store = IndexStore::open(&db_path).unwrap();

        let conn = store.conn.borrow();

        // Query by file and name - should use an index
        let mut stmt = conn
            .prepare("EXPLAIN QUERY PLAN SELECT * FROM symbols WHERE file = ? AND name = ?")
            .unwrap();
        let plan: String = stmt
            .query_map(["test.ts", "foo"], |row| row.get::<_, String>(3))
            .unwrap()
            .next()
            .unwrap()
            .unwrap();

        // Should use an index, not full table scan
        assert!(
            plan.contains("USING INDEX") || plan.contains("SEARCH"),
            "File+name query should use index. Query plan: {}",
            plan
        );
    }

    /// Test compound index for file+kind queries
    #[test]
    fn file_and_kind_query_uses_index() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("index.db");
        let store = IndexStore::open(&db_path).unwrap();

        let conn = store.conn.borrow();

        // Query by file and kind - should use an index
        let mut stmt = conn
            .prepare("EXPLAIN QUERY PLAN SELECT * FROM symbols WHERE file = ? AND kind = ?")
            .unwrap();
        let plan: String = stmt
            .query_map(["test.ts", "function"], |row| row.get::<_, String>(3))
            .unwrap()
            .next()
            .unwrap()
            .unwrap();

        // Should use an index, not full table scan
        assert!(
            plan.contains("USING INDEX") || plan.contains("SEARCH"),
            "File+kind query should use index. Query plan: {}",
            plan
        );
    }

    /// Test compound index for three-way filter (file + kind + name)
    #[test]
    fn file_kind_name_query_uses_compound_index() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("index.db");
        let store = IndexStore::open(&db_path).unwrap();

        let conn = store.conn.borrow();

        // Query by file, kind, and name - should use the compound index
        let mut stmt = conn
            .prepare(
                "EXPLAIN QUERY PLAN SELECT * FROM symbols WHERE file = ? AND kind = ? AND name = ?",
            )
            .unwrap();
        let plan: String = stmt
            .query_map(["test.ts", "function", "foo"], |row| {
                row.get::<_, String>(3)
            })
            .unwrap()
            .next()
            .unwrap()
            .unwrap();

        // Should use the compound index for best performance
        assert!(
            plan.contains("idx_symbols_file_kind_name"),
            "File+kind+name query should use compound index idx_symbols_file_kind_name. Query plan: {}",
            plan
        );
    }

    /// Test that FTS5 table exists for full-text symbol search
    #[test]
    fn fts5_symbols_table_exists() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("index.db");
        let store = IndexStore::open(&db_path).unwrap();

        let conn = store.conn.borrow();
        let table_exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='symbols_fts'",
                [],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(table_exists, 1, "symbols_fts FTS5 table should exist");
    }

    /// Test FTS5 prefix search on symbol names
    #[test]
    fn fts5_prefix_search_works() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("index.db");
        let store = IndexStore::open(&db_path).unwrap();

        // Insert test symbols
        let file_path = dir.path().join("test.ts");
        let file_rec = mk_file_record(&file_path);
        let symbols: Vec<SymbolRecord> = vec![
            SymbolRecord {
                id: "sym_1".into(),
                file: normalize_path(&file_path),
                kind: "function".into(),
                name: "getUserProfile".into(),
                start: 0,
                end: 10,
                qualifier: None,
                visibility: None,
                container: None,
                content_hash: None,
            },
            SymbolRecord {
                id: "sym_2".into(),
                file: normalize_path(&file_path),
                kind: "function".into(),
                name: "getUserSettings".into(),
                start: 20,
                end: 30,
                qualifier: None,
                visibility: None,
                container: None,
                content_hash: None,
            },
            SymbolRecord {
                id: "sym_3".into(),
                file: normalize_path(&file_path),
                kind: "function".into(),
                name: "setUserProfile".into(),
                start: 40,
                end: 50,
                qualifier: None,
                visibility: None,
                container: None,
                content_hash: None,
            },
        ];

        store
            .save_file_index(&file_rec, &symbols, &[], &[])
            .unwrap();

        // Search with prefix "getUser*"
        let results = store.search_symbols_fts("getUser*").unwrap();
        assert_eq!(
            results.len(),
            2,
            "Should find 2 symbols starting with 'getUser'"
        );
        assert!(results.iter().any(|s| s.name == "getUserProfile"));
        assert!(results.iter().any(|s| s.name == "getUserSettings"));
    }

    /// Test that kind+visibility queries use the tertiary index
    #[test]
    fn kind_visibility_query_uses_tertiary_index() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("index.db");
        let store = IndexStore::open(&db_path).unwrap();

        let conn = store.conn.borrow();

        // Query by kind and visibility - should use tertiary index
        let mut stmt = conn
            .prepare("EXPLAIN QUERY PLAN SELECT * FROM symbols WHERE kind = ? AND visibility = ?")
            .unwrap();
        let plan: String = stmt
            .query_map(["function", "public"], |row| row.get::<_, String>(3))
            .unwrap()
            .next()
            .unwrap()
            .unwrap();

        // Should use idx_symbols_kind_visibility index
        assert!(
            plan.contains("idx_symbols_kind_visibility") || plan.contains("USING INDEX"),
            "Kind+visibility query should use idx_symbols_kind_visibility index. Query plan: {}",
            plan
        );
    }

    /// Test that position queries use the idx_symbols_position index
    #[test]
    fn position_query_uses_secondary_index() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("index.db");
        let store = IndexStore::open(&db_path).unwrap();

        let conn = store.conn.borrow();

        // Query symbols at a specific position (file + byte offset range)
        let mut stmt = conn
            .prepare(
                "EXPLAIN QUERY PLAN SELECT * FROM symbols WHERE file = ? AND start <= ? AND ? < end",
            )
            .unwrap();
        let plan: String = stmt
            .query_map(["test.ts", "100", "100"], |row| row.get::<_, String>(3))
            .unwrap()
            .next()
            .unwrap()
            .unwrap();

        // Should use idx_symbols_position index
        assert!(
            plan.contains("idx_symbols_position") || plan.contains("USING INDEX"),
            "Position query should use idx_symbols_position index. Query plan: {}",
            plan
        );
    }

    /// Test FTS5 substring/trigram search
    #[test]
    fn fts5_substring_search_works() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("index.db");
        let store = IndexStore::open(&db_path).unwrap();

        // Insert test symbols
        let file_path = dir.path().join("test.ts");
        let file_rec = mk_file_record(&file_path);
        let symbols: Vec<SymbolRecord> = vec![
            SymbolRecord {
                id: "sym_1".into(),
                file: normalize_path(&file_path),
                kind: "function".into(),
                name: "getUserProfile".into(),
                start: 0,
                end: 10,
                qualifier: None,
                visibility: None,
                container: None,
                content_hash: None,
            },
            SymbolRecord {
                id: "sym_2".into(),
                file: normalize_path(&file_path),
                kind: "class".into(),
                name: "UserProfileService".into(),
                start: 20,
                end: 30,
                qualifier: None,
                visibility: None,
                container: None,
                content_hash: None,
            },
        ];

        store
            .save_file_index(&file_rec, &symbols, &[], &[])
            .unwrap();

        // Search for "Profile" substring
        let results = store.search_symbols_fts("Profile").unwrap();
        assert_eq!(
            results.len(),
            2,
            "Should find 2 symbols containing 'Profile'"
        );
    }

    /// Test that FTS5 efficiently handles prefix queries for autocomplete
    #[test]
    fn fts5_handles_prefix_autocomplete() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("index.db");
        let store = IndexStore::open(&db_path).unwrap();

        let conn = store.conn.borrow();

        // FTS5 prefix queries use the trigram index efficiently
        let mut stmt = conn
            .prepare("EXPLAIN QUERY PLAN SELECT * FROM symbols_fts WHERE symbols_fts MATCH 'get*'")
            .unwrap();
        let plan: String = stmt
            .query_map([], |row| row.get::<_, String>(3))
            .unwrap()
            .next()
            .unwrap()
            .unwrap();

        // FTS5 uses its internal index structure for matching
        assert!(
            plan.contains("symbols_fts") || plan.contains("VIRTUAL TABLE"),
            "FTS5 prefix query should use virtual table index. Query plan: {}",
            plan
        );
    }

    /// Test prefix search functionality using list_symbols
    #[test]
    fn prefix_search_returns_matching_symbols() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("index.db");
        let store = IndexStore::open(&db_path).unwrap();

        // Insert test symbols
        let file_path = dir.path().join("test.ts");
        let file_rec = mk_file_record(&file_path);
        let symbols: Vec<SymbolRecord> = vec![
            SymbolRecord {
                id: "sym_1".into(),
                file: normalize_path(&file_path),
                kind: "function".into(),
                name: "getUser".into(),
                start: 0,
                end: 10,
                qualifier: None,
                visibility: Some("public".into()),
                container: None,
                content_hash: None,
            },
            SymbolRecord {
                id: "sym_2".into(),
                file: normalize_path(&file_path),
                kind: "function".into(),
                name: "getProfile".into(),
                start: 20,
                end: 30,
                qualifier: None,
                visibility: Some("public".into()),
                container: None,
                content_hash: None,
            },
            SymbolRecord {
                id: "sym_3".into(),
                file: normalize_path(&file_path),
                kind: "function".into(),
                name: "setUser".into(),
                start: 40,
                end: 50,
                qualifier: None,
                visibility: Some("private".into()),
                container: None,
                content_hash: None,
            },
        ];

        store
            .save_file_index(&file_rec, &symbols, &[], &[])
            .unwrap();

        // Use FTS5 for prefix search
        let results = store.search_symbols_fts("get*").unwrap();
        assert_eq!(
            results.len(),
            2,
            "Should find 2 symbols starting with 'get'"
        );
        assert!(results.iter().all(|s| s.name.starts_with("get")));
    }

    /// Test cursor-based pagination for streaming large result sets
    #[test]
    fn pagination_streams_results_in_pages() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("index.db");
        let store = IndexStore::open(&db_path).unwrap();

        // Insert 10 test symbols
        let file_path = dir.path().join("test.ts");
        let file_rec = mk_file_record(&file_path);
        let symbols: Vec<SymbolRecord> = (0..10)
            .map(|i| SymbolRecord {
                id: format!("sym_{:02}", i), // Zero-padded for consistent ordering
                file: normalize_path(&file_path),
                kind: "function".into(),
                name: format!("func_{}", i),
                start: i * 10,
                end: i * 10 + 5,
                qualifier: None,
                visibility: None,
                container: None,
                content_hash: None,
            })
            .collect();

        store
            .save_file_index(&file_rec, &symbols, &[], &[])
            .unwrap();

        // First page (3 items)
        let (page1, cursor1) = store
            .list_symbols_paginated(None, None, None, None, 3)
            .unwrap();
        assert_eq!(page1.len(), 3, "First page should have 3 items");
        assert!(cursor1.is_some(), "Should have cursor for next page");

        // Second page using cursor
        let (page2, cursor2) = store
            .list_symbols_paginated(None, None, None, cursor1.as_deref(), 3)
            .unwrap();
        assert_eq!(page2.len(), 3, "Second page should have 3 items");
        assert!(cursor2.is_some(), "Should have cursor for next page");

        // Verify no overlap between pages
        let page1_ids: Vec<_> = page1.iter().map(|s| &s.id).collect();
        let page2_ids: Vec<_> = page2.iter().map(|s| &s.id).collect();
        assert!(
            page1_ids.iter().all(|id| !page2_ids.contains(id)),
            "Pages should not overlap"
        );

        // Continue until exhausted
        let (page3, cursor3) = store
            .list_symbols_paginated(None, None, None, cursor2.as_deref(), 3)
            .unwrap();
        assert_eq!(page3.len(), 3, "Third page should have 3 items");

        let (page4, cursor4) = store
            .list_symbols_paginated(None, None, None, cursor3.as_deref(), 3)
            .unwrap();
        assert_eq!(page4.len(), 1, "Fourth page should have 1 item");
        assert!(cursor4.is_none(), "No more pages");
    }

    /// Test cold-start query performance (<50ms requirement)
    #[test]
    fn cold_start_query_completes_under_50ms() {
        use std::time::Instant;

        let dir = tempdir().unwrap();
        let db_path = dir.path().join("index.db");

        // Create and populate index with substantial data
        {
            let store = IndexStore::open(&db_path).unwrap();
            let file_path = dir.path().join("test.ts");
            let file_rec = mk_file_record(&file_path);

            // Insert 1000 symbols to simulate a real codebase
            let symbols: Vec<SymbolRecord> = (0..1000)
                .map(|i| SymbolRecord {
                    id: format!("sym_{:04}", i),
                    file: normalize_path(&file_path),
                    kind: if i % 3 == 0 {
                        "function"
                    } else if i % 3 == 1 {
                        "class"
                    } else {
                        "interface"
                    }
                    .into(),
                    name: format!("symbol_{}", i),
                    start: i * 100,
                    end: i * 100 + 50,
                    qualifier: Some(format!("module{}", i % 10)),
                    visibility: Some(if i % 2 == 0 { "public" } else { "private" }.into()),
                    container: None,
                    content_hash: None,
                })
                .collect();

            store
                .save_file_index(&file_rec, &symbols, &[], &[])
                .unwrap();
            store.analyze().unwrap();
        } // Close the store to simulate cold start

        // Cold start: open fresh connection and query
        let start = Instant::now();
        let store = IndexStore::open(&db_path).unwrap();

        // Perform typical queries
        let _symbols = store.list_symbols(None, Some("function"), None, Some(10));
        let _search = store.search_symbols_fts("symbol*");
        let _paginated = store.list_symbols_paginated(None, None, None, None, 10);

        let elapsed = start.elapsed();
        assert!(
            elapsed.as_millis() < 50,
            "Cold-start queries should complete in <50ms, took {}ms",
            elapsed.as_millis()
        );
    }

    /// Test pre-computed file statistics are maintained correctly
    #[test]
    fn file_stats_aggregates_computed_on_index() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("index.db");
        let store = IndexStore::open(&db_path).unwrap();

        let file_path = dir.path().join("test.ts");
        let file_rec = mk_file_record(&file_path);
        let symbols: Vec<SymbolRecord> = vec![
            SymbolRecord {
                id: "sym_1".into(),
                file: normalize_path(&file_path),
                kind: "function".into(),
                name: "func1".into(),
                start: 0,
                end: 10,
                qualifier: None,
                visibility: None,
                container: None,
                content_hash: None,
            },
            SymbolRecord {
                id: "sym_2".into(),
                file: normalize_path(&file_path),
                kind: "function".into(),
                name: "func2".into(),
                start: 20,
                end: 30,
                qualifier: None,
                visibility: None,
                container: None,
                content_hash: None,
            },
            SymbolRecord {
                id: "sym_3".into(),
                file: normalize_path(&file_path),
                kind: "class".into(),
                name: "MyClass".into(),
                start: 40,
                end: 50,
                qualifier: None,
                visibility: None,
                container: None,
                content_hash: None,
            },
            SymbolRecord {
                id: "sym_4".into(),
                file: normalize_path(&file_path),
                kind: "interface".into(),
                name: "MyInterface".into(),
                start: 60,
                end: 70,
                qualifier: None,
                visibility: None,
                container: None,
                content_hash: None,
            },
        ];

        store
            .save_file_index(&file_rec, &symbols, &[], &[])
            .unwrap();

        // Verify file stats
        let stats = store
            .get_file_stats(&normalize_path(&file_path))
            .unwrap()
            .expect("file stats should exist");
        assert_eq!(stats.symbol_count, 4);
        assert_eq!(stats.function_count, 2);
        assert_eq!(stats.class_count, 1);
        assert_eq!(stats.interface_count, 1);
    }

    /// Test total stats aggregate across multiple files
    #[test]
    fn total_stats_aggregates_all_files() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("index.db");
        let store = IndexStore::open(&db_path).unwrap();

        // Index first file
        let file1 = dir.path().join("file1.ts");
        let rec1 = mk_file_record(&file1);
        let syms1: Vec<SymbolRecord> = (0..5)
            .map(|i| SymbolRecord {
                id: format!("f1_sym_{}", i),
                file: normalize_path(&file1),
                kind: "function".into(),
                name: format!("func_{}", i),
                start: i * 10,
                end: i * 10 + 5,
                qualifier: None,
                visibility: None,
                container: None,
                content_hash: None,
            })
            .collect();
        store.save_file_index(&rec1, &syms1, &[], &[]).unwrap();

        // Index second file
        let file2 = dir.path().join("file2.ts");
        let rec2 = mk_file_record(&file2);
        let syms2: Vec<SymbolRecord> = (0..3)
            .map(|i| SymbolRecord {
                id: format!("f2_sym_{}", i),
                file: normalize_path(&file2),
                kind: "class".into(),
                name: format!("Class_{}", i),
                start: i * 10,
                end: i * 10 + 5,
                qualifier: None,
                visibility: None,
                container: None,
                content_hash: None,
            })
            .collect();
        store.save_file_index(&rec2, &syms2, &[], &[]).unwrap();

        // Verify total stats
        let total = store.get_total_stats().unwrap();
        assert_eq!(total.symbol_count, 8);
        assert_eq!(total.function_count, 5);
        assert_eq!(total.class_count, 3);
    }

    /// Test file dependency graph basic operations
    #[test]
    fn file_dependency_graph_save_and_query() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("index.db");
        let store = IndexStore::open(&db_path).unwrap();

        // Save dependencies for main.ts -> [utils.ts, types.ts]
        let main_deps = vec![
            FileDependency {
                from_file: "src/main.ts".into(),
                to_file: "src/utils.ts".into(),
                kind: "import".into(),
            },
            FileDependency {
                from_file: "src/main.ts".into(),
                to_file: "src/types.ts".into(),
                kind: "import".into(),
            },
        ];
        store
            .save_file_dependencies("src/main.ts", &main_deps)
            .unwrap();

        // Save dependencies for utils.ts -> [types.ts]
        let utils_deps = vec![FileDependency {
            from_file: "src/utils.ts".into(),
            to_file: "src/types.ts".into(),
            kind: "import".into(),
        }];
        store
            .save_file_dependencies("src/utils.ts", &utils_deps)
            .unwrap();

        // Query dependencies of main.ts
        let main_imports = store.get_file_dependencies("src/main.ts").unwrap();
        assert_eq!(main_imports.len(), 2);

        // Query reverse dependencies (what files depend on types.ts)
        let types_dependents = store.get_dependents("src/types.ts").unwrap();
        assert_eq!(types_dependents.len(), 2);
        assert!(types_dependents.contains(&"src/main.ts".to_string()));
        assert!(types_dependents.contains(&"src/utils.ts".to_string()));
    }

    /// Test dependency graph replacement on re-index
    #[test]
    fn file_dependency_replaces_on_reindex() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("index.db");
        let store = IndexStore::open(&db_path).unwrap();

        // Initial dependencies
        let deps1 = vec![FileDependency {
            from_file: "src/main.ts".into(),
            to_file: "src/old.ts".into(),
            kind: "import".into(),
        }];
        store.save_file_dependencies("src/main.ts", &deps1).unwrap();

        // Re-index with new dependencies
        let deps2 = vec![FileDependency {
            from_file: "src/main.ts".into(),
            to_file: "src/new.ts".into(),
            kind: "import".into(),
        }];
        store.save_file_dependencies("src/main.ts", &deps2).unwrap();

        // Verify old dependencies are replaced
        let deps = store.get_file_dependencies("src/main.ts").unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].to_file, "src/new.ts");
    }

    /// Test topological sort orders dependencies before dependents
    #[test]
    fn topological_sort_orders_dependencies_first() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("index.db");
        let store = IndexStore::open(&db_path).unwrap();

        // Create dependency chain: main -> utils -> types
        let deps = vec![FileDependency {
            from_file: "main.ts".into(),
            to_file: "utils.ts".into(),
            kind: "import".into(),
        }];
        store.save_file_dependencies("main.ts", &deps).unwrap();

        let deps = vec![FileDependency {
            from_file: "utils.ts".into(),
            to_file: "types.ts".into(),
            kind: "import".into(),
        }];
        store.save_file_dependencies("utils.ts", &deps).unwrap();

        store.save_file_dependencies("types.ts", &[]).unwrap();

        // Sort all three files
        let files = vec!["main.ts".into(), "utils.ts".into(), "types.ts".into()];
        let sorted = store.topological_sort(&files).unwrap();

        // types.ts must come before utils.ts, which must come before main.ts
        let types_pos = sorted.iter().position(|f| f == "types.ts").unwrap();
        let utils_pos = sorted.iter().position(|f| f == "utils.ts").unwrap();
        let main_pos = sorted.iter().position(|f| f == "main.ts").unwrap();

        assert!(
            types_pos < utils_pos,
            "types.ts should come before utils.ts"
        );
        assert!(utils_pos < main_pos, "utils.ts should come before main.ts");
    }

    /// Test topological sort handles independent files
    #[test]
    fn topological_sort_handles_independent_files() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("index.db");
        let store = IndexStore::open(&db_path).unwrap();

        // No dependencies between files
        store.save_file_dependencies("a.ts", &[]).unwrap();
        store.save_file_dependencies("b.ts", &[]).unwrap();
        store.save_file_dependencies("c.ts", &[]).unwrap();

        let files = vec!["a.ts".into(), "b.ts".into(), "c.ts".into()];
        let sorted = store.topological_sort(&files).unwrap();

        // All files should be present
        assert_eq!(sorted.len(), 3);
        assert!(sorted.contains(&"a.ts".into()));
        assert!(sorted.contains(&"b.ts".into()));
        assert!(sorted.contains(&"c.ts".into()));
    }

    /// Test invalidation propagation finds all affected files
    #[test]
    fn invalidation_propagates_through_dependency_chain() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("index.db");
        let store = IndexStore::open(&db_path).unwrap();

        // Create chain: main -> utils -> types
        store
            .save_file_dependencies(
                "main.ts",
                &[FileDependency {
                    from_file: "main.ts".into(),
                    to_file: "utils.ts".into(),
                    kind: "import".into(),
                }],
            )
            .unwrap();
        store
            .save_file_dependencies(
                "utils.ts",
                &[FileDependency {
                    from_file: "utils.ts".into(),
                    to_file: "types.ts".into(),
                    kind: "import".into(),
                }],
            )
            .unwrap();
        store.save_file_dependencies("types.ts", &[]).unwrap();

        // When types.ts changes, all three files need reindexing
        let invalidated = store.get_invalidation_set("types.ts").unwrap();
        assert_eq!(invalidated.len(), 3);
        assert!(invalidated.contains(&"types.ts".to_string()));
        assert!(invalidated.contains(&"utils.ts".to_string()));
        assert!(invalidated.contains(&"main.ts".to_string()));

        // When main.ts changes, only main.ts needs reindexing
        let invalidated = store.get_invalidation_set("main.ts").unwrap();
        assert_eq!(invalidated.len(), 1);
        assert_eq!(invalidated[0], "main.ts");
    }

    /// Test batch invalidation handles multiple changed files
    #[test]
    fn batch_invalidation_unions_affected_files() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("index.db");
        let store = IndexStore::open(&db_path).unwrap();

        // Create two independent chains
        // chain1: a -> b
        // chain2: c -> d
        store
            .save_file_dependencies(
                "a.ts",
                &[FileDependency {
                    from_file: "a.ts".into(),
                    to_file: "b.ts".into(),
                    kind: "import".into(),
                }],
            )
            .unwrap();
        store.save_file_dependencies("b.ts", &[]).unwrap();

        store
            .save_file_dependencies(
                "c.ts",
                &[FileDependency {
                    from_file: "c.ts".into(),
                    to_file: "d.ts".into(),
                    kind: "import".into(),
                }],
            )
            .unwrap();
        store.save_file_dependencies("d.ts", &[]).unwrap();

        // When both b.ts and d.ts change
        let changed = vec!["b.ts".into(), "d.ts".into()];
        let invalidated = store.get_batch_invalidation_set(&changed).unwrap();

        // All four files should be invalidated
        assert_eq!(invalidated.len(), 4);
    }

    /// Test DependencyCache provides O(1) lookups
    #[test]
    fn dependency_cache_provides_o1_lookup() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("index.db");
        let store = IndexStore::open(&db_path).unwrap();

        // Create dependencies
        store
            .save_file_dependencies(
                "main.ts",
                &[
                    FileDependency {
                        from_file: "main.ts".into(),
                        to_file: "utils.ts".into(),
                        kind: "import".into(),
                    },
                    FileDependency {
                        from_file: "main.ts".into(),
                        to_file: "types.ts".into(),
                        kind: "import".into(),
                    },
                ],
            )
            .unwrap();
        store
            .save_file_dependencies(
                "utils.ts",
                &[FileDependency {
                    from_file: "utils.ts".into(),
                    to_file: "types.ts".into(),
                    kind: "import".into(),
                }],
            )
            .unwrap();

        // Load cache
        let cache = store.load_dependency_cache().unwrap();
        assert!(cache.is_populated());

        // O(1) forward lookup
        let main_deps = cache.get_dependencies("main.ts").unwrap();
        assert_eq!(main_deps.len(), 2);
        assert!(main_deps.contains(&"utils.ts".to_string()));
        assert!(main_deps.contains(&"types.ts".to_string()));

        // O(1) reverse lookup
        let types_dependents = cache.get_dependents("types.ts").unwrap();
        assert_eq!(types_dependents.len(), 2);
        assert!(types_dependents.contains(&"main.ts".to_string()));
        assert!(types_dependents.contains(&"utils.ts".to_string()));
    }

    /// Test DependencyCache invalidation
    #[test]
    fn dependency_cache_invalidates_correctly() {
        let mut cache = DependencyCache::new();
        let deps = vec![
            FileDependency {
                from_file: "a.ts".into(),
                to_file: "b.ts".into(),
                kind: "import".into(),
            },
            FileDependency {
                from_file: "b.ts".into(),
                to_file: "c.ts".into(),
                kind: "import".into(),
            },
        ];
        cache.populate(&deps);

        // Verify initial state
        assert!(cache.get_dependencies("a.ts").is_some());
        assert!(cache.get_dependents("b.ts").is_some());

        // Invalidate b.ts
        cache.invalidate_file("b.ts");

        // b.ts should have no entries
        assert!(cache.get_dependencies("b.ts").is_none());
        assert!(cache.get_dependents("b.ts").is_none());

        // a.ts forward deps should no longer include b.ts
        let a_deps = cache.get_dependencies("a.ts");
        assert!(a_deps.is_none() || a_deps.unwrap().is_empty());

        // c.ts reverse deps should no longer include b.ts
        let c_dependents = cache.get_dependents("c.ts");
        assert!(c_dependents.is_none() || c_dependents.unwrap().is_empty());
    }

    /// Test DependencyCache clear
    #[test]
    fn dependency_cache_clears_all_entries() {
        let mut cache = DependencyCache::new();
        let deps = vec![FileDependency {
            from_file: "a.ts".into(),
            to_file: "b.ts".into(),
            kind: "import".into(),
        }];
        cache.populate(&deps);
        assert!(cache.is_populated());

        cache.clear();
        assert!(!cache.is_populated());
        assert!(cache.get_dependencies("a.ts").is_none());
    }

    /// Test invalidation handles diamond dependency pattern
    #[test]
    fn invalidation_handles_diamond_dependencies() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("index.db");
        let store = IndexStore::open(&db_path).unwrap();

        // Diamond: main -> [utils, helpers] -> shared
        store
            .save_file_dependencies(
                "main.ts",
                &[
                    FileDependency {
                        from_file: "main.ts".into(),
                        to_file: "utils.ts".into(),
                        kind: "import".into(),
                    },
                    FileDependency {
                        from_file: "main.ts".into(),
                        to_file: "helpers.ts".into(),
                        kind: "import".into(),
                    },
                ],
            )
            .unwrap();
        store
            .save_file_dependencies(
                "utils.ts",
                &[FileDependency {
                    from_file: "utils.ts".into(),
                    to_file: "shared.ts".into(),
                    kind: "import".into(),
                }],
            )
            .unwrap();
        store
            .save_file_dependencies(
                "helpers.ts",
                &[FileDependency {
                    from_file: "helpers.ts".into(),
                    to_file: "shared.ts".into(),
                    kind: "import".into(),
                }],
            )
            .unwrap();
        store.save_file_dependencies("shared.ts", &[]).unwrap();

        // When shared.ts changes, all four files need reindexing
        let invalidated = store.get_invalidation_set("shared.ts").unwrap();
        assert_eq!(invalidated.len(), 4);

        // Verify topological order: shared before utils/helpers before main
        let shared_pos = invalidated.iter().position(|f| f == "shared.ts").unwrap();
        let main_pos = invalidated.iter().position(|f| f == "main.ts").unwrap();
        assert!(shared_pos < main_pos, "shared.ts must come before main.ts");
    }

    /// Test topological sort handles cycles gracefully
    #[test]
    fn topological_sort_handles_cycles() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("index.db");
        let store = IndexStore::open(&db_path).unwrap();

        // Create a cycle: a -> b -> c -> a
        store
            .save_file_dependencies(
                "a.ts",
                &[FileDependency {
                    from_file: "a.ts".into(),
                    to_file: "b.ts".into(),
                    kind: "import".into(),
                }],
            )
            .unwrap();
        store
            .save_file_dependencies(
                "b.ts",
                &[FileDependency {
                    from_file: "b.ts".into(),
                    to_file: "c.ts".into(),
                    kind: "import".into(),
                }],
            )
            .unwrap();
        store
            .save_file_dependencies(
                "c.ts",
                &[FileDependency {
                    from_file: "c.ts".into(),
                    to_file: "a.ts".into(),
                    kind: "import".into(),
                }],
            )
            .unwrap();

        let files = vec!["a.ts".into(), "b.ts".into(), "c.ts".into()];
        let sorted = store.topological_sort(&files).unwrap();

        // All files should still be present (cycles handled gracefully)
        assert_eq!(sorted.len(), 3);
        assert!(sorted.contains(&"a.ts".into()));
        assert!(sorted.contains(&"b.ts".into()));
        assert!(sorted.contains(&"c.ts".into()));
    }

    /// Test dependencies are cleaned up when file is removed
    #[test]
    fn file_dependencies_removed_with_file() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("index.db");
        let store = IndexStore::open(&db_path).unwrap();

        // Create a file to remove
        let file_path = dir.path().join("removeme.ts");
        let file_rec = mk_file_record(&file_path);
        let sym = mk_symbol(&file_path, "test");
        store.save_file_index(&file_rec, &[sym], &[], &[]).unwrap();

        // Add dependencies both directions
        let deps = vec![FileDependency {
            from_file: normalize_path(&file_path),
            to_file: "other.ts".into(),
            kind: "import".into(),
        }];
        store
            .save_file_dependencies(&normalize_path(&file_path), &deps)
            .unwrap();

        // Also make another file depend on removeme.ts
        let other_deps = vec![FileDependency {
            from_file: "depends_on_removeme.ts".into(),
            to_file: normalize_path(&file_path),
            kind: "import".into(),
        }];
        store
            .save_file_dependencies("depends_on_removeme.ts", &other_deps)
            .unwrap();

        // Verify dependencies exist
        assert_eq!(
            store
                .get_file_dependencies(&normalize_path(&file_path))
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            store
                .get_dependents(&normalize_path(&file_path))
                .unwrap()
                .len(),
            1
        );

        // Remove file
        store.remove_file(&file_path).unwrap();

        // Verify both directions of dependencies are cleaned up
        assert!(store
            .get_file_dependencies(&normalize_path(&file_path))
            .unwrap()
            .is_empty());
        assert!(store
            .get_dependents(&normalize_path(&file_path))
            .unwrap()
            .is_empty());
    }

    /// Test that repeated queries use statement caching for better performance
    #[test]
    fn query_plan_caching_improves_repeated_query_performance() {
        use std::time::Instant;

        let dir = tempdir().unwrap();
        let db_path = dir.path().join("index.db");
        let store = IndexStore::open(&db_path).unwrap();

        // Insert test data
        let file_path = dir.path().join("test.ts");
        let file_rec = mk_file_record(&file_path);
        let symbols: Vec<SymbolRecord> = (0..100)
            .map(|i| SymbolRecord {
                id: format!("sym_{:03}", i),
                file: normalize_path(&file_path),
                kind: "function".into(),
                name: format!("func_{}", i),
                start: i * 10,
                end: i * 10 + 5,
                qualifier: None,
                visibility: None,
                container: None,
                content_hash: None,
            })
            .collect();

        let edges: Vec<EdgeRecord> = (0..50)
            .map(|i| EdgeRecord {
                src: format!("sym_{:03}", i),
                dst: format!("sym_{:03}", i + 50),
                kind: "implements".into(),
            })
            .collect();

        store
            .save_file_index(&file_rec, &symbols, &edges, &[])
            .unwrap();

        // Warm up (first query compiles statement)
        let _ = store.edges_to("sym_050");

        // Time repeated cached queries (should be faster than compilation)
        let start = Instant::now();
        for i in 50..100 {
            let _ = store.edges_to(&format!("sym_{:03}", i));
        }
        let cached_duration = start.elapsed();

        // 50 cached queries should complete very quickly
        assert!(
            cached_duration.as_millis() < 100,
            "50 cached queries should complete quickly, took {}ms",
            cached_duration.as_millis()
        );
    }

    /// Test file stats are cleaned up on file removal
    #[test]
    fn file_stats_removed_with_file() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("index.db");
        let store = IndexStore::open(&db_path).unwrap();

        let file_path = dir.path().join("test.ts");
        let file_rec = mk_file_record(&file_path);
        let sym = mk_symbol(&file_path, "test");

        store.save_file_index(&file_rec, &[sym], &[], &[]).unwrap();

        // Verify stats exist
        assert!(store
            .get_file_stats(&normalize_path(&file_path))
            .unwrap()
            .is_some());

        // Remove file
        store.remove_file(&file_path).unwrap();

        // Verify stats removed
        assert!(store
            .get_file_stats(&normalize_path(&file_path))
            .unwrap()
            .is_none());
    }

    /// Test that SQLite pragmas are configured for performance
    #[test]
    fn sqlite_performance_pragmas_configured() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("index.db");
        let store = IndexStore::open(&db_path).unwrap();

        let conn = store.conn.borrow();

        // Check WAL mode
        let journal_mode: String = conn
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .unwrap();
        assert_eq!(journal_mode.to_lowercase(), "wal", "Should use WAL mode");

        // Check mmap is enabled (non-zero)
        let mmap_size: i64 = conn
            .query_row("PRAGMA mmap_size", [], |row| row.get(0))
            .unwrap();
        assert!(
            mmap_size > 0,
            "mmap should be enabled for memory-mapped I/O"
        );

        // Check cache size is configured
        let cache_size: i64 = conn
            .query_row("PRAGMA cache_size", [], |row| row.get(0))
            .unwrap();
        assert!(
            !(0..=1000).contains(&cache_size),
            "Cache should be configured (got {})",
            cache_size
        );
    }
}
