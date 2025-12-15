use anyhow::Result;
use rusqlite::types::Value;
use rusqlite::{Connection, params, params_from_iter};
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

#[derive(Debug, Clone)]
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
}

#[derive(Debug, Clone)]
pub struct EdgeRecord {
    pub src: String,
    pub dst: String,
    pub kind: String,
}

#[derive(Debug, Clone)]
pub struct ReferenceRecord {
    pub file: String,
    pub start: i64,
    pub end: i64,
    pub symbol_id: String,
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
                container TEXT
            );
            -- B-tree indices for O(log n) lookups
            CREATE INDEX IF NOT EXISTS symbols_file_idx ON symbols(file);
            CREATE INDEX IF NOT EXISTS idx_symbols_name ON symbols(name);
            CREATE INDEX IF NOT EXISTS idx_symbols_position ON symbols(file, start, end);
            CREATE INDEX IF NOT EXISTS idx_symbols_kind_name ON symbols(kind, name);

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
            "#,
        )?;
        self.ensure_column("symbols", "qualifier", "TEXT")?;
        self.ensure_column("symbols", "visibility", "TEXT")?;
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
        self.conn
            .borrow()
            .execute("DELETE FROM symbols WHERE file = ?1", params![path_str])?;
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
                "INSERT INTO symbols(id, file, kind, name, start, end, qualifier, visibility, container) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    sym.id,
                    sym.file,
                    sym.kind,
                    sym.name,
                    sym.start,
                    sym.end,
                    sym.qualifier,
                    sym.visibility,
                    sym.container
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

    pub fn list_symbols(
        &self,
        file: Option<&str>,
        kind: Option<&str>,
        name: Option<&str>,
        limit: Option<usize>,
    ) -> Result<Vec<SymbolRecord>> {
        let file_norm = file.map(|f| normalize_path(Path::new(f)));
        let mut sql = String::from(
            "SELECT id, file, kind, name, start, end, qualifier, visibility, container FROM symbols",
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
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn edges_to(&self, dst: &str) -> Result<Vec<EdgeRecord>> {
        let conn = self.conn.borrow();
        let mut stmt = conn.prepare("SELECT src, dst, kind FROM edges WHERE dst = ?1")?;
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

    pub fn edges_from(&self, src: &str) -> Result<Vec<EdgeRecord>> {
        let conn = self.conn.borrow();
        let mut stmt = conn.prepare("SELECT src, dst, kind FROM edges WHERE src = ?1")?;
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
            "SELECT id, file, kind, name, start, end, qualifier, visibility, container FROM symbols WHERE id IN ({})",
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
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn references_for_symbol(&self, symbol_id: &str) -> Result<Vec<ReferenceRecord>> {
        let conn = self.conn.borrow();
        let mut stmt = conn.prepare(
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
            .save_file_index(&file_rec, &[sym.clone()], &edges, &refs)
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
}
