use anyhow::{Context, Result};
use log::info;
use rusqlite::types::Value;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension};
use serde::Serialize;
use std::cell::RefCell;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

// Schema version constants
pub const SCHEMA_MAJOR: u32 = 1;
pub const SCHEMA_MINOR: u32 = 0;

/// Schema version for database compatibility checking.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SchemaVersion {
    pub major: u32,
    pub minor: u32,
}

impl SchemaVersion {
    pub fn current() -> Self {
        Self {
            major: SCHEMA_MAJOR,
            minor: SCHEMA_MINOR,
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        let parts: Vec<&str> = s.split('.').collect();
        if parts.len() == 2 {
            Some(Self {
                major: parts[0].parse().ok()?,
                minor: parts[1].parse().ok()?,
            })
        } else {
            None
        }
    }

    pub fn requires_regeneration(&self, current: &Self) -> bool {
        self.major != current.major
    }

    pub fn requires_migration(&self, current: &Self) -> bool {
        self.major == current.major && self.minor < current.minor
    }
}

impl std::fmt::Display for SchemaVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

/// Result of attempting to open a database.
pub enum DbOpenResult {
    /// Database is ready to use
    Ready(IndexStore),
    /// Database needs regeneration before use
    NeedsRegeneration {
        reason: RegenerationReason,
        path: PathBuf,
    },
}

/// Reason why database regeneration is needed.
#[derive(Debug)]
pub enum RegenerationReason {
    /// Schema major version is incompatible
    MajorVersionMismatch {
        db_version: String,
        app_version: String,
    },
    /// Database predates version tracking
    LegacyDatabase,
    /// Database file is corrupted
    CorruptDatabase(String),
    /// User explicitly requested rebuild
    UserRequested,
}

impl RegenerationReason {
    /// Get a user-friendly message explaining the regeneration reason.
    pub fn message(&self) -> String {
        match self {
            RegenerationReason::MajorVersionMismatch {
                db_version,
                app_version,
            } => {
                format!(
                    "Index schema version {} is incompatible with gabb schema {}",
                    db_version, app_version
                )
            }
            RegenerationReason::LegacyDatabase => {
                "Found legacy index without version tracking".to_string()
            }
            RegenerationReason::CorruptDatabase(err) => {
                format!("Index database appears corrupted: {}", err)
            }
            RegenerationReason::UserRequested => "Rebuild requested by user".to_string(),
        }
    }
}

/// A database migration from one schema version to another.
struct Migration {
    from_version: SchemaVersion,
    to_version: SchemaVersion,
    description: &'static str,
    migrate: fn(&Connection) -> Result<()>,
}

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
    /// Whether this symbol is inside test code (#[cfg(test)] or has #[test] attribute)
    pub is_test: bool,
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

/// Stored import binding for displaying import chains.
/// Maps how a symbol is imported in a file.
#[derive(Debug, Clone, Serialize)]
pub struct ImportBindingRecord {
    /// The file containing the import statement
    pub file: String,
    /// The local name used in the importing file (may be aliased)
    pub local_name: String,
    /// The original name exported from the source
    pub original_name: String,
    /// The resolved source file path
    pub source_file: String,
    /// The full import statement text (e.g., "import { foo } from './bar'")
    pub import_text: String,
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

/// Comprehensive index statistics for the `gabb stats` command and MCP tool.
#[derive(Debug, Clone, Serialize)]
pub struct IndexStats {
    /// File statistics
    pub files: FileCountStats,
    /// Symbol statistics
    pub symbols: SymbolCountStats,
    /// Index metadata
    pub index: IndexMetadata,
    /// Parse error summary
    pub errors: ParseErrorStats,
}

/// File count statistics broken down by language.
#[derive(Debug, Clone, Serialize)]
pub struct FileCountStats {
    /// Total number of indexed files
    pub total: i64,
    /// File counts by language (e.g., "typescript": 150, "rust": 50)
    pub by_language: HashMap<String, i64>,
}

/// Symbol count statistics broken down by kind.
#[derive(Debug, Clone, Serialize)]
pub struct SymbolCountStats {
    /// Total number of symbols
    pub total: i64,
    /// Symbol counts by kind (e.g., "function": 2000, "class": 500)
    pub by_kind: HashMap<String, i64>,
}

/// Index metadata including size and timestamps.
#[derive(Debug, Clone, Serialize)]
pub struct IndexMetadata {
    /// Database file size in bytes
    pub size_bytes: u64,
    /// Last update timestamp (ISO 8601)
    pub last_updated: Option<String>,
    /// Schema version string
    pub schema_version: String,
}

/// Parse error summary.
#[derive(Debug, Clone, Serialize)]
pub struct ParseErrorStats {
    /// Number of files that failed to parse
    pub parse_failures: i64,
    /// List of failed file paths (limited to avoid huge output)
    pub failed_files: Vec<String>,
}

/// Query options for searching symbols with flexible filtering.
#[derive(Debug, Clone, Default)]
pub struct SymbolQuery<'a> {
    /// Filter to symbols in this path. Supports:
    /// - Exact file path: `/path/to/file.ts`
    /// - Directory prefix: `src/` or `src/components/`
    /// - Glob pattern: `src/**/*.ts`, `*.test.ts`
    pub file: Option<&'a str>,
    /// Filter by symbol kind (function, class, interface, etc.)
    pub kind: Option<&'a str>,
    /// Filter by exact symbol name
    pub name: Option<&'a str>,
    /// Filter by glob-style pattern (e.g., "get*", "*Handler", "*User*")
    /// Uses SQL LIKE with `*` converted to `%`
    pub name_pattern: Option<&'a str>,
    /// Filter by substring match (case-sensitive by default)
    pub name_contains: Option<&'a str>,
    /// Fuzzy/prefix search using FTS5 trigram matching
    /// Supports prefix patterns (e.g., "getUser*") and fuzzy substrings
    pub name_fts: Option<&'a str>,
    /// Make name matching case-insensitive (applies to name, name_pattern, name_contains)
    pub case_insensitive: bool,
    /// Maximum number of results to return
    pub limit: Option<usize>,
    /// Number of results to skip (for offset-based pagination)
    pub offset: Option<usize>,
    /// Cursor for keyset pagination (symbol ID to start after)
    /// More efficient than offset for large result sets
    pub after: Option<&'a str>,
    /// Filter by namespace/qualifier prefix. Supports:
    /// - Exact prefix: `std::collections` matches `std::collections::HashMap`
    /// - Glob pattern: `std::*` matches any symbol in std namespace
    pub namespace: Option<&'a str>,
    /// Filter by container/scope (class, module, etc. that contains the symbol)
    /// Useful for finding methods within a specific class
    pub scope: Option<&'a str>,
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
    /// Open a database, creating it if it doesn't exist.
    /// This method always succeeds if the file can be created/opened.
    /// Use `try_open()` for version-aware opening with migration support.
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

    /// Try to open a database with version checking.
    /// Returns `DbOpenResult::Ready` if the database is compatible.
    /// Returns `DbOpenResult::NeedsRegeneration` if the database needs to be rebuilt.
    pub fn try_open(path: &Path) -> Result<DbOpenResult> {
        // If file doesn't exist, create new database
        if !path.exists() {
            return Ok(DbOpenResult::Ready(Self::open(path)?));
        }

        // Open existing database for inspection
        let conn = Connection::open(path).context("failed to open index database")?;

        // Quick integrity check
        if let Err(e) = conn.query_row("PRAGMA quick_check", [], |row| row.get::<_, String>(0)) {
            return Ok(DbOpenResult::NeedsRegeneration {
                reason: RegenerationReason::CorruptDatabase(e.to_string()),
                path: path.to_path_buf(),
            });
        }

        // Check for schema_meta table (indicates versioned database)
        if !Self::has_schema_meta(&conn) {
            return Ok(DbOpenResult::NeedsRegeneration {
                reason: RegenerationReason::LegacyDatabase,
                path: path.to_path_buf(),
            });
        }

        // Read version from database
        let db_version_str: Option<String> = conn
            .query_row(
                "SELECT value FROM schema_meta WHERE key = 'schema_version'",
                [],
                |row| row.get(0),
            )
            .optional()?;

        let db_version = match db_version_str.and_then(|s| SchemaVersion::parse(&s)) {
            Some(v) => v,
            None => {
                return Ok(DbOpenResult::NeedsRegeneration {
                    reason: RegenerationReason::LegacyDatabase,
                    path: path.to_path_buf(),
                });
            }
        };

        let current = SchemaVersion::current();

        // Check for major version mismatch (requires regeneration)
        if db_version.requires_regeneration(&current) {
            return Ok(DbOpenResult::NeedsRegeneration {
                reason: RegenerationReason::MajorVersionMismatch {
                    db_version: db_version.to_string(),
                    app_version: current.to_string(),
                },
                path: path.to_path_buf(),
            });
        }

        // Close the inspection connection and properly open with schema init
        drop(conn);
        let store = Self::open(path)?;

        // Apply migrations if needed (minor version upgrade)
        if db_version.requires_migration(&current) {
            info!(
                "Migrating index from schema {} to {}...",
                db_version, current
            );
            store.apply_migrations(&db_version, &current)?;
            info!("Migration complete");
        }

        Ok(DbOpenResult::Ready(store))
    }

    /// Apply migrations from one version to another.
    fn apply_migrations(&self, from: &SchemaVersion, to: &SchemaVersion) -> Result<()> {
        let migrations = Self::get_migrations();
        let mut current = from.clone();

        for migration in migrations {
            if migration.from_version == current && migration.to_version <= *to {
                info!("Applying migration: {}", migration.description);
                (migration.migrate)(&self.conn.borrow())?;

                // Update stored version
                self.conn.borrow().execute(
                    "UPDATE schema_meta SET value = ?1 WHERE key = 'schema_version'",
                    params![migration.to_version.to_string()],
                )?;
                self.conn.borrow().execute(
                    "UPDATE schema_meta SET value = ?1 WHERE key = 'last_migration'",
                    params![now_unix().to_string()],
                )?;

                current = migration.to_version.clone();
            }
        }

        Ok(())
    }

    /// Get the list of available migrations.
    /// Migrations are applied in order from older to newer versions.
    fn get_migrations() -> Vec<Migration> {
        vec![
            // Future migrations will be added here, e.g.:
            // Migration {
            //     from_version: SchemaVersion { major: 1, minor: 0 },
            //     to_version: SchemaVersion { major: 1, minor: 1 },
            //     description: "Add symbol signature column",
            //     migrate: |conn| {
            //         conn.execute("ALTER TABLE symbols ADD COLUMN signature TEXT", [])?;
            //         Ok(())
            //     },
            // },
        ]
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
                content_hash TEXT,
                is_test INTEGER NOT NULL DEFAULT 0
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

            -- Import bindings for displaying import chains in usages
            CREATE TABLE IF NOT EXISTS import_bindings (
                file TEXT NOT NULL,
                local_name TEXT NOT NULL,
                original_name TEXT NOT NULL,
                source_file TEXT NOT NULL,
                import_text TEXT NOT NULL,
                PRIMARY KEY (file, local_name)
            );
            -- Index for looking up imports by source file
            CREATE INDEX IF NOT EXISTS idx_imports_source ON import_bindings(source_file, original_name);

            -- Schema metadata for version tracking and migrations
            CREATE TABLE IF NOT EXISTS schema_meta (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );

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
        self.ensure_column("symbols", "is_test", "INTEGER NOT NULL DEFAULT 0")?;
        self.ensure_index(
            "idx_symbols_content_hash",
            "CREATE INDEX IF NOT EXISTS idx_symbols_content_hash ON symbols(content_hash)",
        )?;
        // Initialize schema version if not present (new database)
        self.ensure_schema_version()?;
        Ok(())
    }

    /// Ensure schema_meta has version info. Only inserts if not already present.
    fn ensure_schema_version(&self) -> Result<()> {
        let conn = self.conn.borrow();
        let version = SchemaVersion::current();
        let now = now_unix();

        // Insert version if not exists
        conn.execute(
            "INSERT OR IGNORE INTO schema_meta (key, value) VALUES ('schema_version', ?1)",
            params![version.to_string()],
        )?;
        conn.execute(
            "INSERT OR IGNORE INTO schema_meta (key, value) VALUES ('gabb_version', ?1)",
            params![env!("CARGO_PKG_VERSION")],
        )?;
        conn.execute(
            "INSERT OR IGNORE INTO schema_meta (key, value) VALUES ('created_at', ?1)",
            params![now.to_string()],
        )?;
        conn.execute(
            "INSERT OR IGNORE INTO schema_meta (key, value) VALUES ('last_migration', ?1)",
            params![now.to_string()],
        )?;
        Ok(())
    }

    /// Check if schema_meta table exists (indicates versioned database).
    fn has_schema_meta(conn: &Connection) -> bool {
        conn.query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='schema_meta'",
            [],
            |_| Ok(true),
        )
        .unwrap_or(false)
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
            params![path_str.clone()],
        )?;
        self.conn.borrow().execute(
            "DELETE FROM import_bindings WHERE file = ?1",
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
                "INSERT INTO symbols(id, file, kind, name, start, end, qualifier, visibility, container, content_hash, is_test) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
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
                    sym.content_hash,
                    sym.is_test
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
                "INSERT INTO symbols(id, file, kind, name, start, end, qualifier, visibility, container, content_hash, is_test) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
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
                    sym.content_hash,
                    sym.is_test
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

    /// Get comprehensive index statistics for the `gabb stats` command.
    /// Returns file counts by language, symbol counts by kind, index metadata, and parse errors.
    pub fn get_index_stats(&self) -> Result<IndexStats> {
        let conn = self.conn.borrow();

        // File counts by language (inferred from file extension)
        let mut by_language: HashMap<String, i64> = HashMap::new();
        let total_files: i64;
        {
            let mut stmt = conn.prepare("SELECT path FROM files")?;
            let paths = stmt
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;

            total_files = paths.len() as i64;
            for path in paths {
                let lang = infer_language_from_path(&path);
                *by_language.entry(lang).or_insert(0) += 1;
            }
        }

        // Symbol counts by kind
        let mut by_kind: HashMap<String, i64> = HashMap::new();
        let mut total_symbols: i64 = 0;
        {
            let mut stmt = conn.prepare("SELECT kind, COUNT(*) FROM symbols GROUP BY kind")?;
            let rows = stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?;
            for row in rows {
                let (kind, count) = row?;
                total_symbols += count;
                by_kind.insert(kind, count);
            }
        }

        // Index metadata
        let size_bytes = fs::metadata(self.db_path()).map(|m| m.len()).unwrap_or(0);

        let last_updated = {
            let mut stmt = conn.prepare("SELECT MAX(indexed_at) FROM files")?;
            let max_ts: Option<i64> = stmt.query_row([], |row| row.get(0)).ok().flatten();
            max_ts.map(|ts| {
                // Convert Unix timestamp to ISO 8601
                chrono::DateTime::from_timestamp(ts, 0)
                    .map(|dt| dt.format("%Y-%m-%dT%H:%M:%SZ").to_string())
                    .unwrap_or_else(|| ts.to_string())
            })
        };

        let schema_version = {
            let version: Option<String> = conn
                .query_row(
                    "SELECT value FROM schema_meta WHERE key = 'version'",
                    [],
                    |row| row.get(0),
                )
                .optional()?;
            version.unwrap_or_else(|| "unknown".to_string())
        };

        // Parse error stats - for now we don't track failures explicitly,
        // but we can detect files with no symbols as potential failures
        // In the future, we could add a parse_errors table
        let parse_failures: i64 = 0;
        let failed_files: Vec<String> = Vec::new();

        Ok(IndexStats {
            files: FileCountStats {
                total: total_files,
                by_language,
            },
            symbols: SymbolCountStats {
                total: total_symbols,
                by_kind,
            },
            index: IndexMetadata {
                size_bytes,
                last_updated,
                schema_version,
            },
            errors: ParseErrorStats {
                parse_failures,
                failed_files,
            },
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

    /// Save import bindings for a file, replacing any existing bindings.
    pub fn save_import_bindings(&self, file: &str, bindings: &[ImportBindingRecord]) -> Result<()> {
        let file_norm = normalize_path(Path::new(file));
        let conn = &mut *self.conn.borrow_mut();
        let tx = conn.transaction()?;

        // Remove existing bindings for this file
        tx.execute(
            "DELETE FROM import_bindings WHERE file = ?1",
            params![file_norm],
        )?;

        // Insert new bindings
        for binding in bindings {
            tx.execute(
                "INSERT OR REPLACE INTO import_bindings(file, local_name, original_name, source_file, import_text) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    file_norm,
                    binding.local_name,
                    binding.original_name,
                    normalize_path(Path::new(&binding.source_file)),
                    binding.import_text
                ],
            )?;
        }

        tx.commit()?;
        Ok(())
    }

    /// Get import binding for a symbol in a file.
    /// Looks up how a symbol from source_file was imported into file.
    pub fn get_import_binding(
        &self,
        file: &str,
        source_file: &str,
        original_name: &str,
    ) -> Result<Option<ImportBindingRecord>> {
        let file_norm = normalize_path(Path::new(file));
        let source_norm = normalize_path(Path::new(source_file));
        let conn = self.conn.borrow();
        let mut stmt = conn.prepare_cached(
            "SELECT file, local_name, original_name, source_file, import_text
             FROM import_bindings
             WHERE file = ?1 AND source_file = ?2 AND original_name = ?3",
        )?;
        let mut rows = stmt.query(params![file_norm, source_norm, original_name])?;
        if let Some(row) = rows.next()? {
            Ok(Some(ImportBindingRecord {
                file: row.get(0)?,
                local_name: row.get(1)?,
                original_name: row.get(2)?,
                source_file: row.get(3)?,
                import_text: row.get(4)?,
            }))
        } else {
            Ok(None)
        }
    }

    /// Get all import bindings for a file.
    #[allow(dead_code)]
    pub fn get_import_bindings_for_file(&self, file: &str) -> Result<Vec<ImportBindingRecord>> {
        let file_norm = normalize_path(Path::new(file));
        let conn = self.conn.borrow();
        let mut stmt = conn.prepare_cached(
            "SELECT file, local_name, original_name, source_file, import_text
             FROM import_bindings WHERE file = ?1",
        )?;
        let rows = stmt
            .query_map(params![file_norm], |row| {
                Ok(ImportBindingRecord {
                    file: row.get(0)?,
                    local_name: row.get(1)?,
                    original_name: row.get(2)?,
                    source_file: row.get(3)?,
                    import_text: row.get(4)?,
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

    /// Get all files that a given file transitively depends on (forward dependency traversal).
    /// This is the opposite of get_invalidation_set - it walks forward through imports/includes.
    pub fn get_transitive_dependencies(&self, file: &str) -> Result<Vec<String>> {
        let file_norm = normalize_path(Path::new(file));
        let mut visited = HashSet::new();
        let mut to_visit = vec![file_norm.clone()];
        let mut result = Vec::new();

        while let Some(current) = to_visit.pop() {
            if visited.contains(&current) {
                continue;
            }
            visited.insert(current.clone());
            result.push(current.clone());

            // Get all files that this file depends on
            let deps = self.get_file_dependencies(&current)?;
            for dep in deps {
                if !visited.contains(&dep.to_file) {
                    to_visit.push(dep.to_file);
                }
            }
        }

        // Remove the starting file from the result (we want dependencies, not the file itself)
        result.retain(|f| f != &file_norm);
        Ok(result)
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
            "SELECT id, file, kind, name, start, end, qualifier, visibility, container, content_hash, is_test FROM symbols",
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
                    is_test: row.get::<_, i64>(10)? != 0,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Query symbols with flexible filtering options.
    /// Supports exact name, pattern matching (glob-style with `*`), substring search, and FTS5 fuzzy search.
    /// File filtering supports exact paths, directory prefixes, and glob patterns.
    pub fn list_symbols_filtered(&self, query: &SymbolQuery) -> Result<Vec<SymbolRecord>> {
        // If FTS5 search is requested, use the FTS5 table directly and apply other filters in Rust
        if let Some(fts_query) = query.name_fts {
            let mut results = self.search_symbols_fts(fts_query)?;

            // Apply additional filters
            if let Some(file_path) = query.file {
                let file_normalized = normalize_path(Path::new(file_path));
                if file_path.contains('*') {
                    // Simple glob matching: split by * and check contains
                    results.retain(|s| glob_match(file_path, &s.file));
                } else if file_path.ends_with('/') {
                    results.retain(|s| s.file.starts_with(&file_normalized));
                } else if file_path.starts_with('/') {
                    // Absolute path - exact match or directory prefix
                    results.retain(|s| {
                        s.file == file_normalized
                            || s.file.starts_with(&format!("{}/", file_normalized))
                    });
                } else {
                    // Relative path - match as suffix (DB stores absolute paths)
                    results.retain(|s| {
                        s.file == file_normalized
                            || s.file.ends_with(&format!("/{}", file_normalized))
                    });
                }
            }

            if let Some(k) = query.kind {
                results.retain(|s| s.kind == k);
            }

            if let Some(ns) = query.namespace {
                if ns.contains('*') {
                    results.retain(|s| s.qualifier.as_ref().is_some_and(|q| glob_match(ns, q)));
                } else {
                    results.retain(|s| {
                        s.qualifier
                            .as_ref()
                            .is_some_and(|q| q == ns || q.starts_with(&format!("{}::", ns)))
                    });
                }
            }

            if let Some(scope) = query.scope {
                results.retain(|s| s.container.as_ref().is_some_and(|c| c == scope));
            }

            if let Some(lim) = query.limit {
                results.truncate(lim);
            }

            return Ok(results);
        }

        let mut sql = String::from(
            "SELECT id, file, kind, name, start, end, qualifier, visibility, container, content_hash, is_test FROM symbols",
        );
        let mut values: Vec<Value> = Vec::new();
        let mut clauses: Vec<String> = Vec::new();

        // Handle file path filtering with support for exact, directory, and glob patterns
        if let Some(file_path) = query.file {
            if file_path.contains('*') {
                // Glob pattern: convert * to SQL LIKE % wildcard
                // Both * and ** become % (consecutive % in LIKE is same as single %)
                let like_pattern = file_path.replace('*', "%");
                clauses.push("file LIKE ?".to_string());
                values.push(Value::from(like_pattern));
            } else if file_path.ends_with('/') {
                // Directory prefix: match files starting with this path
                let prefix = normalize_path(Path::new(file_path));
                clauses.push("file LIKE ?".to_string());
                values.push(Value::from(format!("{}%", prefix)));
            } else {
                // Check if it looks like a directory (no extension and not an existing exact match concept)
                // For simplicity, treat paths without '.' in the last component as potential directories
                let last_component = Path::new(file_path)
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("");
                if !last_component.contains('.') && !file_path.is_empty() {
                    // Could be a directory - match both exact and as prefix
                    let normalized = normalize_path(Path::new(file_path));
                    clauses.push("(file = ? OR file LIKE ?)".to_string());
                    values.push(Value::from(normalized.clone()));
                    values.push(Value::from(format!("{}/%", normalized)));
                } else {
                    // File path match
                    let normalized = normalize_path(Path::new(file_path));
                    if file_path.starts_with('/') {
                        // Absolute path - exact match only
                        clauses.push("file = ?".to_string());
                        values.push(Value::from(normalized));
                    } else {
                        // Relative path - match as suffix (DB stores absolute paths)
                        // Match either exact or ending with /relative/path
                        clauses.push("(file = ? OR file LIKE ?)".to_string());
                        values.push(Value::from(normalized.clone()));
                        values.push(Value::from(format!("%/{}", normalized)));
                    }
                }
            }
        }

        if let Some(k) = query.kind {
            clauses.push("kind = ?".to_string());
            values.push(Value::from(k.to_string()));
        }

        // Handle name filtering with priority: exact > pattern > contains
        // Use LOWER() for case-insensitive matching when requested
        if let Some(n) = query.name {
            if query.case_insensitive {
                clauses.push("LOWER(name) = LOWER(?)".to_string());
            } else {
                clauses.push("name = ?".to_string());
            }
            values.push(Value::from(n.to_string()));
        } else if let Some(pattern) = query.name_pattern {
            // Convert glob pattern to SQL LIKE pattern: * -> %, ? -> _
            let like_pattern = pattern.replace('*', "%").replace('?', "_");
            if query.case_insensitive {
                clauses.push("LOWER(name) LIKE LOWER(?)".to_string());
            } else {
                clauses.push("name LIKE ?".to_string());
            }
            values.push(Value::from(like_pattern));
        } else if let Some(contains) = query.name_contains {
            // Substring match: wrap with %
            let like_pattern = format!("%{}%", contains);
            if query.case_insensitive {
                clauses.push("LOWER(name) LIKE LOWER(?)".to_string());
            } else {
                clauses.push("name LIKE ?".to_string());
            }
            values.push(Value::from(like_pattern));
        }

        // Handle namespace filtering on qualifier column
        if let Some(ns) = query.namespace {
            if ns.contains('*') {
                // Glob pattern: convert * to SQL LIKE % wildcard
                let like_pattern = ns.replace('*', "%");
                clauses.push("qualifier LIKE ?".to_string());
                values.push(Value::from(like_pattern));
            } else {
                // Prefix match: namespace should match the start of qualifier
                // e.g., "std::collections" matches "std::collections::HashMap"
                clauses.push("(qualifier = ? OR qualifier LIKE ?)".to_string());
                values.push(Value::from(ns.to_string()));
                values.push(Value::from(format!("{}::%", ns)));
            }
        }

        // Handle scope filtering on container column
        if let Some(scope) = query.scope {
            clauses.push("container = ?".to_string());
            values.push(Value::from(scope.to_string()));
        }

        // Handle cursor-based pagination (more efficient for large result sets)
        if let Some(cursor) = query.after {
            clauses.push("id > ?".to_string());
            values.push(Value::from(cursor.to_string()));
        }

        if !clauses.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&clauses.join(" AND "));
        }

        // Order by id for consistent cursor-based pagination
        sql.push_str(" ORDER BY id");

        if let Some(lim) = query.limit {
            sql.push_str(" LIMIT ?");
            values.push(Value::from(lim as i64));
        }

        if let Some(off) = query.offset {
            sql.push_str(" OFFSET ?");
            values.push(Value::from(off as i64));
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
                    is_test: row.get::<_, i64>(10)? != 0,
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

    /// Get all edges with placeholder destinations (used for two-phase resolution).
    /// Returns edges where dst contains "::" but not "#" (indicating unresolved import reference).
    pub fn get_unresolved_edges(&self) -> Result<Vec<EdgeRecord>> {
        let conn = self.conn.borrow();
        let mut stmt = conn.prepare_cached(
            "SELECT src, dst, kind FROM edges WHERE dst LIKE '%::%' AND dst NOT LIKE '%#%'",
        )?;
        let edges = stmt
            .query_map([], |row| {
                Ok(EdgeRecord {
                    src: row.get(0)?,
                    dst: row.get(1)?,
                    kind: row.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(edges)
    }

    /// Update an edge destination (used for two-phase resolution).
    /// Updates the edge where src and old_dst match.
    pub fn update_edge_destination(
        &self,
        src: &str,
        old_dst: &str,
        new_dst: &str,
    ) -> Result<usize> {
        let conn = self.conn.borrow();
        let updated = conn.execute(
            "UPDATE edges SET dst = ?3 WHERE src = ?1 AND dst = ?2",
            params![src, old_dst, new_dst],
        )?;
        Ok(updated)
    }

    pub fn symbols_by_ids(&self, ids: &[String]) -> Result<Vec<SymbolRecord>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = std::iter::repeat_n("?", ids.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT id, file, kind, name, start, end, qualifier, visibility, container, content_hash, is_test FROM symbols WHERE id IN ({})",
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
                    is_test: row.get::<_, i64>(10)? != 0,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Find supertypes (parent classes, implemented interfaces/traits) of a symbol.
    /// Returns symbols that the given symbol extends or implements.
    ///
    /// If `transitive` is true, follows the hierarchy chain recursively.
    pub fn supertypes(&self, symbol_id: &str, transitive: bool) -> Result<Vec<SymbolRecord>> {
        let inheritance_kinds = ["extends", "implements", "trait_impl"];

        if transitive {
            self.supertypes_transitive(symbol_id, &inheritance_kinds)
        } else {
            // Get direct supertypes: edges FROM this symbol TO its parents
            let edges = self.edges_from(symbol_id)?;
            let parent_ids: Vec<String> = edges
                .into_iter()
                .filter(|e| inheritance_kinds.contains(&e.kind.as_str()))
                .map(|e| e.dst)
                .collect();
            self.symbols_by_ids(&parent_ids)
        }
    }

    /// Find subtypes (subclasses, implementors) of a symbol.
    /// Returns symbols that extend or implement the given symbol.
    ///
    /// If `transitive` is true, follows the hierarchy chain recursively.
    pub fn subtypes(&self, symbol_id: &str, transitive: bool) -> Result<Vec<SymbolRecord>> {
        let inheritance_kinds = ["extends", "implements", "trait_impl"];

        if transitive {
            self.subtypes_transitive(symbol_id, &inheritance_kinds)
        } else {
            // Get direct subtypes: edges TO this symbol FROM its children
            let edges = self.edges_to(symbol_id)?;
            let child_ids: Vec<String> = edges
                .into_iter()
                .filter(|e| inheritance_kinds.contains(&e.kind.as_str()))
                .map(|e| e.src)
                .collect();
            self.symbols_by_ids(&child_ids)
        }
    }

    /// Helper for transitive supertype lookup
    fn supertypes_transitive(&self, symbol_id: &str, kinds: &[&str]) -> Result<Vec<SymbolRecord>> {
        use std::collections::HashSet;

        let mut visited: HashSet<String> = HashSet::new();
        let mut result: Vec<SymbolRecord> = Vec::new();
        let mut queue: Vec<String> = vec![symbol_id.to_string()];

        while let Some(current_id) = queue.pop() {
            if visited.contains(&current_id) {
                continue;
            }
            visited.insert(current_id.clone());

            let edges = self.edges_from(&current_id)?;
            let parent_ids: Vec<String> = edges
                .into_iter()
                .filter(|e| kinds.contains(&e.kind.as_str()))
                .map(|e| e.dst)
                .collect();

            let parents = self.symbols_by_ids(&parent_ids)?;
            for parent in parents {
                if !visited.contains(&parent.id) {
                    queue.push(parent.id.clone());
                }
                // Only add if not the original symbol
                if parent.id != symbol_id {
                    result.push(parent);
                }
            }
        }

        Ok(result)
    }

    /// Helper for transitive subtype lookup
    fn subtypes_transitive(&self, symbol_id: &str, kinds: &[&str]) -> Result<Vec<SymbolRecord>> {
        use std::collections::HashSet;

        let mut visited: HashSet<String> = HashSet::new();
        let mut result: Vec<SymbolRecord> = Vec::new();
        let mut queue: Vec<String> = vec![symbol_id.to_string()];

        while let Some(current_id) = queue.pop() {
            if visited.contains(&current_id) {
                continue;
            }
            visited.insert(current_id.clone());

            let edges = self.edges_to(&current_id)?;
            let child_ids: Vec<String> = edges
                .into_iter()
                .filter(|e| kinds.contains(&e.kind.as_str()))
                .map(|e| e.src)
                .collect();

            let children = self.symbols_by_ids(&child_ids)?;
            for child in children {
                if !visited.contains(&child.id) {
                    queue.push(child.id.clone());
                }
                // Only add if not the original symbol
                if child.id != symbol_id {
                    result.push(child);
                }
            }
        }

        Ok(result)
    }

    /// Find callers of a function/method.
    /// Returns symbols that call the given function.
    ///
    /// If `transitive` is true, follows the call chain recursively (who calls the callers, etc.).
    pub fn callers(&self, symbol_id: &str, transitive: bool) -> Result<Vec<SymbolRecord>> {
        if transitive {
            self.callers_transitive(symbol_id)
        } else {
            // Get direct callers: edges TO this symbol with kind="calls"
            let edges = self.edges_to(symbol_id)?;
            let caller_ids: Vec<String> = edges
                .into_iter()
                .filter(|e| e.kind == "calls")
                .map(|e| e.src)
                .collect();
            self.symbols_by_ids(&caller_ids)
        }
    }

    /// Find callees of a function/method.
    /// Returns symbols that are called by the given function.
    ///
    /// If `transitive` is true, follows the call chain recursively (what do callees call, etc.).
    pub fn callees(&self, symbol_id: &str, transitive: bool) -> Result<Vec<SymbolRecord>> {
        if transitive {
            self.callees_transitive(symbol_id)
        } else {
            // Get direct callees: edges FROM this symbol with kind="calls"
            let edges = self.edges_from(symbol_id)?;
            let callee_ids: Vec<String> = edges
                .into_iter()
                .filter(|e| e.kind == "calls")
                .map(|e| e.dst)
                .collect();
            self.symbols_by_ids(&callee_ids)
        }
    }

    /// Helper for transitive callers lookup
    fn callers_transitive(&self, symbol_id: &str) -> Result<Vec<SymbolRecord>> {
        use std::collections::HashSet;

        let mut visited: HashSet<String> = HashSet::new();
        let mut result: Vec<SymbolRecord> = Vec::new();
        let mut queue: Vec<String> = vec![symbol_id.to_string()];

        while let Some(current_id) = queue.pop() {
            if visited.contains(&current_id) {
                continue;
            }
            visited.insert(current_id.clone());

            let edges = self.edges_to(&current_id)?;
            let caller_ids: Vec<String> = edges
                .into_iter()
                .filter(|e| e.kind == "calls")
                .map(|e| e.src)
                .collect();

            let callers = self.symbols_by_ids(&caller_ids)?;
            for caller in callers {
                if !visited.contains(&caller.id) {
                    queue.push(caller.id.clone());
                }
                if caller.id != symbol_id {
                    result.push(caller);
                }
            }
        }

        Ok(result)
    }

    /// Helper for transitive callees lookup
    fn callees_transitive(&self, symbol_id: &str) -> Result<Vec<SymbolRecord>> {
        use std::collections::HashSet;

        let mut visited: HashSet<String> = HashSet::new();
        let mut result: Vec<SymbolRecord> = Vec::new();
        let mut queue: Vec<String> = vec![symbol_id.to_string()];

        while let Some(current_id) = queue.pop() {
            if visited.contains(&current_id) {
                continue;
            }
            visited.insert(current_id.clone());

            let edges = self.edges_from(&current_id)?;
            let callee_ids: Vec<String> = edges
                .into_iter()
                .filter(|e| e.kind == "calls")
                .map(|e| e.dst)
                .collect();

            let callees = self.symbols_by_ids(&callee_ids)?;
            for callee in callees {
                if !visited.contains(&callee.id) {
                    queue.push(callee.id.clone());
                }
                if callee.id != symbol_id {
                    result.push(callee);
                }
            }
        }

        Ok(result)
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
            "SELECT id, file, kind, name, start, end, qualifier, visibility, container, content_hash, is_test
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
                    is_test: row.get::<_, i64>(10)? != 0,
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
            SELECT s.id, s.file, s.kind, s.name, s.start, s.end, s.qualifier, s.visibility, s.container, s.content_hash, s.is_test
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
                    is_test: row.get::<_, i64>(10)? != 0,
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
            "SELECT id, file, kind, name, start, end, qualifier, visibility, container, content_hash, is_test FROM symbols",
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
                    is_test: row.get::<_, i64>(10)? != 0,
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

/// Simple glob pattern matching with * and ** wildcards.
/// Does not support ? or character classes, but handles common cases efficiently.
fn glob_match(pattern: &str, text: &str) -> bool {
    // Replace ** with a temporary marker, then * with .*, then restore **
    // This is a simple implementation that handles most common glob patterns
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.is_empty() {
        return pattern == text;
    }

    let mut pos = 0;
    let first = parts[0];
    let last = parts[parts.len() - 1];

    // Check prefix if pattern doesn't start with *
    if !pattern.starts_with('*') && !text.starts_with(first) {
        return false;
    }
    if !first.is_empty() {
        pos = first.len();
    }

    // Check suffix if pattern doesn't end with *
    if !pattern.ends_with('*') && !text.ends_with(last) {
        return false;
    }

    // Check middle parts
    for part in parts.iter().skip(1).take(parts.len() - 2) {
        if part.is_empty() {
            continue;
        }
        if let Some(found) = text[pos..].find(part) {
            pos += found + part.len();
        } else {
            return false;
        }
    }

    true
}

pub fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

/// Infer programming language from file path extension.
fn infer_language_from_path(path: &str) -> String {
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");

    match ext.to_lowercase().as_str() {
        "ts" | "tsx" | "mts" | "cts" => "typescript".to_string(),
        "js" | "jsx" | "mjs" | "cjs" => "javascript".to_string(),
        "rs" => "rust".to_string(),
        "kt" | "kts" => "kotlin".to_string(),
        "java" => "java".to_string(),
        "py" | "pyi" => "python".to_string(),
        "go" => "go".to_string(),
        "c" | "h" => "c".to_string(),
        "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx" => "cpp".to_string(),
        "cs" => "csharp".to_string(),
        "rb" => "ruby".to_string(),
        "php" => "php".to_string(),
        "swift" => "swift".to_string(),
        "scala" => "scala".to_string(),
        "lua" => "lua".to_string(),
        "sh" | "bash" | "zsh" => "shell".to_string(),
        "json" => "json".to_string(),
        "yaml" | "yml" => "yaml".to_string(),
        "toml" => "toml".to_string(),
        "xml" => "xml".to_string(),
        "html" | "htm" => "html".to_string(),
        "css" => "css".to_string(),
        "scss" | "sass" => "scss".to_string(),
        "sql" => "sql".to_string(),
        "md" | "markdown" => "markdown".to_string(),
        _ => "other".to_string(),
    }
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
            is_test: false,
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
                is_test: false,
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
                is_test: false,
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
                is_test: false,
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
                is_test: false,
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
                is_test: false,
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
                is_test: false,
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
                is_test: false,
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
                is_test: false,
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
                is_test: false,
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
                is_test: false,
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
                    is_test: false,
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
                is_test: false,
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
                is_test: false,
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
                is_test: false,
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
                is_test: false,
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
                is_test: false,
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
                is_test: false,
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
                is_test: false,
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

    // ==================== Schema Migration Tests ====================

    /// Test that new database creation includes version tracking
    #[test]
    fn new_database_has_schema_version() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("index.db");
        let store = IndexStore::open(&db_path).unwrap();

        let conn = store.conn.borrow();

        // Verify schema_meta table exists
        let table_exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='schema_meta'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(table_exists, 1, "schema_meta table should exist");

        // Verify schema_version is set
        let version: String = conn
            .query_row(
                "SELECT value FROM schema_meta WHERE key = 'schema_version'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            version,
            format!("{}.{}", SCHEMA_MAJOR, SCHEMA_MINOR),
            "Schema version should be set to current version"
        );

        // Verify gabb_version is set
        let gabb_version: String = conn
            .query_row(
                "SELECT value FROM schema_meta WHERE key = 'gabb_version'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            gabb_version,
            env!("CARGO_PKG_VERSION"),
            "Gabb version should be set"
        );
    }

    /// Test that try_open returns Ready for current version database
    #[test]
    fn try_open_returns_ready_for_current_version() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("index.db");

        // Create a fresh database
        let _store = IndexStore::open(&db_path).unwrap();
        drop(_store);

        // try_open should return Ready
        match IndexStore::try_open(&db_path).unwrap() {
            DbOpenResult::Ready(_) => {}
            DbOpenResult::NeedsRegeneration { reason, .. } => {
                panic!(
                    "Expected Ready, got NeedsRegeneration: {}",
                    reason.message()
                );
            }
        }
    }

    /// Test that legacy database (no schema_meta) triggers regeneration
    #[test]
    fn legacy_database_triggers_regeneration() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("index.db");

        // Create a legacy database without schema_meta table
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute(
                "CREATE TABLE files (path TEXT PRIMARY KEY, hash TEXT, mtime INTEGER)",
                [],
            )
            .unwrap();
            conn.execute(
                "CREATE TABLE symbols (id TEXT PRIMARY KEY, file TEXT, kind TEXT, name TEXT)",
                [],
            )
            .unwrap();
            // Note: No schema_meta table
        }

        // try_open should detect legacy database
        match IndexStore::try_open(&db_path).unwrap() {
            DbOpenResult::Ready(_) => {
                panic!("Expected NeedsRegeneration for legacy database");
            }
            DbOpenResult::NeedsRegeneration { reason, .. } => {
                assert!(
                    matches!(reason, RegenerationReason::LegacyDatabase),
                    "Expected LegacyDatabase reason"
                );
            }
        }
    }

    /// Test that major version mismatch triggers regeneration
    #[test]
    fn major_version_mismatch_triggers_regeneration() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("index.db");

        // Create a database with a different major version
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute(
                "CREATE TABLE schema_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
                [],
            )
            .unwrap();
            // Insert a future major version
            conn.execute(
                "INSERT INTO schema_meta (key, value) VALUES ('schema_version', '99.0')",
                [],
            )
            .unwrap();
        }

        // try_open should detect major version mismatch
        match IndexStore::try_open(&db_path).unwrap() {
            DbOpenResult::Ready(_) => {
                panic!("Expected NeedsRegeneration for major version mismatch");
            }
            DbOpenResult::NeedsRegeneration { reason, .. } => match reason {
                RegenerationReason::MajorVersionMismatch {
                    db_version,
                    app_version,
                } => {
                    assert_eq!(db_version, "99.0");
                    assert_eq!(app_version, format!("{}.{}", SCHEMA_MAJOR, SCHEMA_MINOR));
                }
                _ => panic!("Expected MajorVersionMismatch reason"),
            },
        }
    }

    /// Test SchemaVersion comparison and parsing
    #[test]
    fn schema_version_parsing_and_comparison() {
        // Test parsing
        assert_eq!(
            SchemaVersion::parse("1.0"),
            Some(SchemaVersion { major: 1, minor: 0 })
        );
        assert_eq!(
            SchemaVersion::parse("2.15"),
            Some(SchemaVersion {
                major: 2,
                minor: 15
            })
        );
        assert_eq!(SchemaVersion::parse("invalid"), None);
        assert_eq!(SchemaVersion::parse("1"), None);
        assert_eq!(SchemaVersion::parse(""), None);

        // Test requires_regeneration (major version difference)
        let v1_0 = SchemaVersion { major: 1, minor: 0 };
        let v1_5 = SchemaVersion { major: 1, minor: 5 };
        let v2_0 = SchemaVersion { major: 2, minor: 0 };

        assert!(!v1_0.requires_regeneration(&v1_5)); // Same major, no regen
        assert!(v1_0.requires_regeneration(&v2_0)); // Different major, regen
        assert!(v2_0.requires_regeneration(&v1_0)); // Different major, regen

        // Test requires_migration (same major, lower minor)
        assert!(v1_0.requires_migration(&v1_5)); // 1.0 needs migration to 1.5
        assert!(!v1_5.requires_migration(&v1_0)); // 1.5 doesn't need migration to 1.0
        assert!(!v1_5.requires_migration(&v1_5)); // Same version, no migration
        assert!(!v1_0.requires_migration(&v2_0)); // Different major, use regen not migration
    }

    /// Test RegenerationReason message formatting
    #[test]
    fn regeneration_reason_messages() {
        let legacy = RegenerationReason::LegacyDatabase;
        assert!(legacy.message().contains("legacy"));

        let mismatch = RegenerationReason::MajorVersionMismatch {
            db_version: "1.0".into(),
            app_version: "2.0".into(),
        };
        assert!(mismatch.message().contains("1.0"));
        assert!(mismatch.message().contains("2.0"));

        let corrupt = RegenerationReason::CorruptDatabase("test error".into());
        assert!(corrupt.message().contains("test error"));

        let user = RegenerationReason::UserRequested;
        assert!(user.message().contains("requested"));
    }

    /// Test that relative file paths match symbols stored with absolute paths.
    /// This is issue #66: file filter should support relative paths.
    #[test]
    fn relative_file_path_matches_absolute_stored_path() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("index.db");
        let store = IndexStore::open(&db_path).unwrap();

        // Create a symbol with an absolute path (as the indexer does)
        let absolute_path = "/workspace/src/languages/rust.rs";
        let sym = SymbolRecord {
            id: format!("{}#0-100", absolute_path),
            file: absolute_path.to_string(),
            kind: "function".to_string(),
            name: "index_file".to_string(),
            start: 0,
            end: 100,
            qualifier: None,
            visibility: Some("pub".to_string()),
            container: None,
            content_hash: None,
            is_test: false,
        };

        store
            .save_file_index(
                &FileRecord {
                    path: absolute_path.to_string(),
                    hash: "abc123".to_string(),
                    mtime: 12345,
                    indexed_at: 12345,
                },
                &[sym],
                &[],
                &[],
            )
            .unwrap();

        // Query with relative path (without leading slash) - should find the symbol
        let query = SymbolQuery {
            file: Some("src/languages/rust.rs"),
            ..Default::default()
        };
        let results = store.list_symbols_filtered(&query).unwrap();
        assert_eq!(
            results.len(),
            1,
            "Relative path 'src/languages/rust.rs' should match absolute path '{}'",
            absolute_path
        );
        assert_eq!(results[0].name, "index_file");
    }
}
