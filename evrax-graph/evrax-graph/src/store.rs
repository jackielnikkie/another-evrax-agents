//! SQLite storage: single `.evrax/graph.db` file per project.
//!
//! Schema pattern from codebase-memory-mcp's `sqlite_writer.c` (MIT):
//! `file_hashes` for incremental indexing, bidirectional edge indexes
//! `(source_id, type)` + `(target_id, type)` so callers/callees are fast.

use rusqlite::{params, Connection};
use std::path::Path;

use crate::extract::{RawLet, RawNode, RawUse};
use crate::schema::{EdgeKind, NodeLabel, NodeRow};
use crate::Result;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS projects(
    name TEXT PRIMARY KEY,
    root_path TEXT NOT NULL,
    indexed_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS file_hashes(
    project TEXT NOT NULL,
    rel_path TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    mtime_ns INTEGER NOT NULL,
    size INTEGER NOT NULL,
    PRIMARY KEY(project, rel_path)
);
CREATE TABLE IF NOT EXISTS nodes(
    id INTEGER PRIMARY KEY,
    project TEXT NOT NULL,
    label TEXT NOT NULL,
    name TEXT NOT NULL,
    qualified_name TEXT NOT NULL,
    file_path TEXT NOT NULL,
    start_line INTEGER NOT NULL,
    end_line INTEGER NOT NULL,
    UNIQUE(project, qualified_name)
);
CREATE INDEX IF NOT EXISTS idx_nodes_proj_label ON nodes(project, label);
CREATE INDEX IF NOT EXISTS idx_nodes_proj_name ON nodes(project, name);
CREATE INDEX IF NOT EXISTS idx_nodes_proj_file ON nodes(project, file_path);
CREATE TABLE IF NOT EXISTS edges(
    id INTEGER PRIMARY KEY,
    project TEXT NOT NULL,
    source_id INTEGER NOT NULL,
    target_id INTEGER NOT NULL,
    type TEXT NOT NULL,
    UNIQUE(project, source_id, target_id, type)
);
CREATE INDEX IF NOT EXISTS idx_edges_src ON edges(project, source_id, type);
CREATE INDEX IF NOT EXISTS idx_edges_dst ON edges(project, target_id, type);
-- phase-2 resolver aux tables (per-file, refreshed on re-index)
CREATE TABLE IF NOT EXISTS use_map(
    project TEXT NOT NULL,
    file_path TEXT NOT NULL,
    local TEXT NOT NULL,
    full_path TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_usemap ON use_map(project, file_path);
CREATE TABLE IF NOT EXISTS let_bindings(
    project TEXT NOT NULL,
    file_path TEXT NOT NULL,
    func_qualified TEXT NOT NULL,
    var TEXT NOT NULL,
    ty TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_letbind ON let_bindings(project, func_qualified);
";

pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn })
    }

    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    pub fn upsert_project(&self, name: &str, root_path: &str) -> Result<()> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        self.conn.execute(
            "INSERT INTO projects(name, root_path, indexed_at) VALUES(?1, ?2, ?3)
             ON CONFLICT(name) DO UPDATE SET indexed_at = excluded.indexed_at",
            params![name, root_path, now],
        )?;
        Ok(())
    }

    pub fn get_file_hash(&self, project: &str, rel_path: &str) -> Result<Option<(String, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT sha256, mtime_ns FROM file_hashes WHERE project = ?1 AND rel_path = ?2",
        )?;
        let mut rows = stmt.query(params![project, rel_path])?;
        if let Some(r) = rows.next()? {
            Ok(Some((r.get(0)?, r.get(1)?)))
        } else {
            Ok(None)
        }
    }

    pub fn set_file_hash(
        &self,
        project: &str,
        rel_path: &str,
        sha256: &str,
        mtime_ns: i64,
        size: i64,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO file_hashes(project, rel_path, sha256, mtime_ns, size)
             VALUES(?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(project, rel_path) DO UPDATE SET
               sha256 = excluded.sha256, mtime_ns = excluded.mtime_ns, size = excluded.size",
            params![project, rel_path, sha256, mtime_ns, size],
        )?;
        Ok(())
    }

    /// Remove all nodes/edges that came from one file (before re-indexing it).
    pub fn delete_file_data(&self, project: &str, rel_path: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM edges WHERE project = ?1 AND (
               source_id IN (SELECT id FROM nodes WHERE project = ?1 AND file_path = ?2) OR
               target_id IN (SELECT id FROM nodes WHERE project = ?1 AND file_path = ?2))",
            params![project, rel_path],
        )?;
        self.conn.execute(
            "DELETE FROM nodes WHERE project = ?1 AND file_path = ?2",
            params![project, rel_path],
        )?;
        Ok(())
    }

    /// Insert a node; returns its id (existing id if qualified_name collided).
    pub fn insert_node(&self, project: &str, node: &RawNode) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO nodes(project, label, name, qualified_name, file_path, start_line, end_line)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(project, qualified_name) DO NOTHING",
            params![
                project,
                node.label.as_str(),
                node.name,
                node.qualified_name,
                // file node: file_path == qualified_name == rel path
                rel_of(node),
                node.start_line as i64,
                node.end_line as i64,
            ],
        )?;
        let id: i64 = self.conn.query_row(
            "SELECT id FROM nodes WHERE project = ?1 AND qualified_name = ?2",
            params![project, node.qualified_name],
            |r| r.get(0),
        )?;
        Ok(id)
    }

    pub fn insert_edge(
        &self,
        project: &str,
        source_id: i64,
        target_id: i64,
        kind: EdgeKind,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO edges(project, source_id, target_id, type)
             VALUES(?1, ?2, ?3, ?4)
             ON CONFLICT(project, source_id, target_id, type) DO NOTHING",
            params![project, source_id, target_id, kind.as_str()],
        )?;
        Ok(())
    }

    pub fn node_id(&self, project: &str, qualified_name: &str) -> Result<Option<i64>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id FROM nodes WHERE project = ?1 AND qualified_name = ?2")?;
        let mut rows = stmt.query(params![project, qualified_name])?;
        if let Some(r) = rows.next()? {
            Ok(Some(r.get(0)?))
        } else {
            Ok(None)
        }
    }

    /// Remove aux resolver data for one file (called alongside delete_file_data).
    pub fn delete_file_aux(&self, project: &str, rel_path: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM use_map WHERE project = ?1 AND file_path = ?2",
            params![project, rel_path],
        )?;
        self.conn.execute(
            "DELETE FROM let_bindings WHERE project = ?1 AND file_path = ?2",
            params![project, rel_path],
        )?;
        Ok(())
    }

    pub fn insert_use(&self, project: &str, u: &RawUse) -> Result<()> {
        self.conn.execute(
            "INSERT INTO use_map(project, file_path, local, full_path) VALUES(?1, ?2, ?3, ?4)",
            params![project, u.file, u.local, u.full_path],
        )?;
        Ok(())
    }

    pub fn insert_let(&self, project: &str, file_path: &str, l: &RawLet) -> Result<()> {
        self.conn.execute(
            "INSERT INTO let_bindings(project, file_path, func_qualified, var, ty)
             VALUES(?1, ?2, ?3, ?4, ?5)",
            params![project, file_path, l.func_qualified, l.var, l.ty],
        )?;
        Ok(())
    }

    pub fn all_uses(&self, project: &str) -> Result<Vec<RawUse>> {
        let mut stmt = self
            .conn
            .prepare("SELECT file_path, local, full_path FROM use_map WHERE project = ?1")?;
        let rows = stmt.query_map([project], |r| {
            Ok(RawUse {
                file: r.get(0)?,
                local: r.get(1)?,
                full_path: r.get(2)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn all_lets(&self, project: &str) -> Result<Vec<RawLet>> {
        let mut stmt = self
            .conn
            .prepare("SELECT func_qualified, var, ty FROM let_bindings WHERE project = ?1")?;
        let rows = stmt.query_map([project], |r| {
            Ok(RawLet {
                func_qualified: r.get(0)?,
                var: r.get(1)?,
                ty: r.get(2)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    /// Every node in the project: (qualified_name, id, file_path, label).
    pub fn all_node_infos(&self, project: &str) -> Result<Vec<(String, i64, String, String)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT qualified_name, id, file_path, label FROM nodes WHERE project = ?1")?;
        let rows = stmt.query_map([project], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn count(&self, project: &str) -> Result<(i64, i64)> {
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM nodes WHERE project = ?1",
            params![project],
            |r| r.get(0),
        )?;
        let e: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM edges WHERE project = ?1",
            params![project],
            |r| r.get(0),
        )?;
        Ok((n, e))
    }
}

/// file_path for a raw node: the file node's qualified_name is the rel path;
/// symbol nodes derive it from their qualified_name prefix.
fn rel_of(node: &RawNode) -> &str {
    if node.label == NodeLabel::File {
        &node.qualified_name
    } else {
        match node.qualified_name.find("::") {
            Some(i) => &node.qualified_name[..i],
            None => &node.qualified_name,
        }
    }
}

pub fn row_to_node(r: &rusqlite::Row) -> rusqlite::Result<NodeRow> {
    Ok(NodeRow {
        id: r.get(0)?,
        label: r.get(1)?,
        name: r.get(2)?,
        qualified_name: r.get(3)?,
        file_path: r.get(4)?,
        start_line: r.get(5)?,
        end_line: r.get(6)?,
    })
}

pub const NODE_COLS: &str = "id, label, name, qualified_name, file_path, start_line, end_line";

/// Same columns, qualified with the `n` table alias (for JOIN queries).
pub const NODE_COLS_N: &str =
    "n.id, n.label, n.name, n.qualified_name, n.file_path, n.start_line, n.end_line";
