//! The 4 core agent queries: symbols, callers/callees, impact, dead code.
//!
//! These are the native equivalents of codebase-memory-mcp's `search_graph`
//! / `trace_path` / `detect_changes` (blast-radius part), minus the MCP
//! JSON-RPC layer — direct function calls instead.

use rusqlite::{params, Connection};
use std::collections::{HashSet, VecDeque};

use crate::schema::NodeRow;
use crate::store::{row_to_node, NODE_COLS, NODE_COLS_N};
use crate::Result;

fn by_id(conn: &Connection, id: i64) -> Result<Option<NodeRow>> {
    let mut stmt = conn.prepare(&format!("SELECT {NODE_COLS} FROM nodes WHERE id = ?1"))?;
    let mut rows = stmt.query([id])?;
    if let Some(r) = rows.next()? {
        Ok(Some(row_to_node(r)?))
    } else {
        Ok(None)
    }
}

fn node_id(conn: &Connection, project: &str, qualified_name: &str) -> Result<Option<i64>> {
    let mut stmt =
        conn.prepare("SELECT id FROM nodes WHERE project = ?1 AND qualified_name = ?2")?;
    let mut rows = stmt.query(params![project, qualified_name])?;
    if let Some(r) = rows.next()? {
        Ok(Some(r.get(0)?))
    } else {
        Ok(None)
    }
}

/// Fuzzy symbol search by name (LIKE %pattern%).
pub fn search_symbols(
    conn: &Connection,
    project: &str,
    pattern: &str,
    limit: i64,
) -> Result<Vec<NodeRow>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {NODE_COLS} FROM nodes
         WHERE project = ?1 AND name LIKE '%' || ?2 || '%'
         ORDER BY length(name) LIMIT ?3"
    ))?;
    let rows = stmt.query_map(params![project, pattern, limit], row_to_node)?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

/// 1-hop neighbours over CALLS edges.
fn neighbours(
    conn: &Connection,
    project: &str,
    qualified_name: &str,
    inbound: bool,
) -> Result<Vec<NodeRow>> {
    let id = match node_id(conn, project, qualified_name)? {
        Some(id) => id,
        None => return Ok(Vec::new()),
    };
    // inbound: who calls me  (edge.target = me)
    // outbound: what I call  (edge.source = me)
    let (join_col, filter_col) = if inbound {
        ("source_id", "target_id")
    } else {
        ("target_id", "source_id")
    };
    let mut stmt = conn.prepare(&format!(
        "SELECT {NODE_COLS_N} FROM nodes n
         JOIN edges e ON e.{join_col} = n.id
         WHERE e.project = ?1 AND e.{filter_col} = ?2 AND e.type = 'CALLS'"
    ))?;
    let rows = stmt.query_map(params![project, id], row_to_node)?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

/// Direct callers of a symbol.
pub fn callers(conn: &Connection, project: &str, qualified_name: &str) -> Result<Vec<NodeRow>> {
    neighbours(conn, project, qualified_name, true)
}

/// Direct callees of a symbol.
pub fn callees(conn: &Connection, project: &str, qualified_name: &str) -> Result<Vec<NodeRow>> {
    neighbours(conn, project, qualified_name, false)
}

/// Blast radius: all transitive callers of a symbol (BFS over reverse CALLS).
/// Answers "what breaks if I change this?".
pub fn impact(
    conn: &Connection,
    project: &str,
    qualified_name: &str,
    max_depth: usize,
) -> Result<Vec<NodeRow>> {
    let start = match node_id(conn, project, qualified_name)? {
        Some(id) => id,
        None => return Ok(Vec::new()),
    };
    let mut seen: HashSet<i64> = HashSet::from([start]);
    let mut queue: VecDeque<(i64, usize)> = VecDeque::from([(start, 0)]);
    let mut out: Vec<NodeRow> = Vec::new();
    while let Some((id, depth)) = queue.pop_front() {
        if depth >= max_depth {
            continue;
        }
        let mut stmt = conn.prepare(
            "SELECT source_id FROM edges
             WHERE project = ?1 AND target_id = ?2 AND type = 'CALLS'",
        )?;
        let rows = stmt.query_map(params![project, id], |r| r.get::<_, i64>(0))?;
        for row in rows {
            let cid = row?;
            if seen.insert(cid) {
                queue.push_back((cid, depth + 1));
                if let Some(n) = by_id(conn, cid)? {
                    out.push(n);
                }
            }
        }
    }
    Ok(out)
}

/// Functions/methods nobody calls (dead-code candidates).
pub fn dead_code(conn: &Connection, project: &str) -> Result<Vec<NodeRow>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {NODE_COLS} FROM nodes n
         WHERE n.project = ?1 AND n.label IN ('Function', 'Method')
           AND NOT EXISTS (
             SELECT 1 FROM edges e
             WHERE e.project = ?1 AND e.target_id = n.id AND e.type = 'CALLS'
           )
         ORDER BY n.file_path, n.start_line"
    ))?;
    let rows = stmt.query_map([project], row_to_node)?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

/// All symbols declared in one file, in source order.
pub fn file_outline(conn: &Connection, project: &str, rel_path: &str) -> Result<Vec<NodeRow>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {NODE_COLS} FROM nodes
         WHERE project = ?1 AND file_path = ?2 AND label != 'File'
         ORDER BY start_line"
    ))?;
    let rows = stmt.query_map(params![project, rel_path], row_to_node)?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}
