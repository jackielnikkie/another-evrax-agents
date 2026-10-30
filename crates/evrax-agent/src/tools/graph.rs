use std::path::Path;

use evrax_graph::{Store, default_db_path, index_project, query};
use serde_json::Value;

use super::{Context, ToolOutcome, truncate};

pub(super) fn run(args: &Value, ctx: &Context) -> Result<ToolOutcome, String> {
    let action = args
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or("status")
        .to_ascii_lowercase();
    let project = project_name(args, &ctx.root);
    let db = default_db_path(&ctx.root);
    if action == "index" {
        let stats = index_project(&ctx.root, &db, &project).map_err(|e| e.to_string())?;
        return Ok(ToolOutcome {
            ok: true,
            detail: format!(
                "graph indexed: {project}\ndb: {}\nfiles: {} seen, {} indexed\nnodes: {}\nedges: {}",
                db.display(),
                stats.files_seen,
                stats.files_indexed,
                stats.nodes,
                stats.edges
            ),
            diff: None,
        });
    }
    if !db.is_file() {
        // Not being indexed yet is informational, not a failure: a red `✗`
        // here reads like a real error. Real failures (corrupt DB, IO) still
        // surface as `ok:false` below when the store cannot be opened/read.
        if action == "status" {
            return Ok(ToolOutcome {
                ok: true,
                detail: format!(
                    "graph belum di-index · jalankan evrax-graph action=index\nproject: {project}\ndb: {}",
                    db.display()
                ),
                diff: None,
            });
        }
        return Err(format!(
            "graph not indexed for {project}; run action=index first ({})",
            db.display()
        ));
    }
    let store = Store::open(&db).map_err(|e| e.to_string())?;
    let conn = store.connection();
    match action.as_str() {
        "status" => {
            let nodes: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM nodes WHERE project = ?1",
                    [project.as_str()],
                    |row| row.get(0),
                )
                .unwrap_or(0);
            let edges: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM edges WHERE project = ?1",
                    [project.as_str()],
                    |row| row.get(0),
                )
                .unwrap_or(0);
            Ok(ToolOutcome {
                ok: true,
                detail: format!(
                    "graph: {project}\ndb: {}\nnodes: {nodes}\nedges: {edges}",
                    db.display()
                ),
                diff: None,
            })
        }
        "search" => {
            let pattern = required(args, "pattern")?;
            let limit = limit(args);
            let rows = query::search_symbols(conn, &project, &pattern, limit)
                .map_err(|e| e.to_string())?;
            Ok(outcome(format_nodes(rows), true))
        }
        "callers" | "callees" | "impact" => {
            let symbol = resolve_symbol(&store, &project, &required(args, "symbol")?)?;
            let rows = match action.as_str() {
                "callers" => query::callers(conn, &project, &symbol),
                "callees" => query::callees(conn, &project, &symbol),
                _ => {
                    let depth = args
                        .get("depth")
                        .and_then(Value::as_u64)
                        .unwrap_or(3)
                        .clamp(1, 10) as usize;
                    query::impact(conn, &project, &symbol, depth)
                }
            }
            .map_err(|e| e.to_string())?;
            Ok(outcome(
                format!("{action} {symbol}\n{}", format_nodes(rows)),
                true,
            ))
        }
        "dead_code" => {
            let rows = query::dead_code(conn, &project).map_err(|e| e.to_string())?;
            Ok(outcome(format_nodes(rows), true))
        }
        "outline" => {
            let raw = required(args, "path")?;
            let resolved = ctx.classify(&raw)?;
            if resolved.outside {
                return Err(format!(
                    "{} is outside the workspace",
                    resolved.path.display()
                ));
            }
            let rel = resolved
                .path
                .strip_prefix(&ctx.root)
                .unwrap_or(&resolved.path)
                .to_string_lossy()
                .replace('\\', "/");
            let rows = query::file_outline(conn, &project, &rel).map_err(|e| e.to_string())?;
            Ok(outcome(format_nodes(rows), true))
        }
        other => Err(format!("unknown graph action: {other}")),
    }
}

fn project_name(args: &Value, root: &Path) -> String {
    args.get("project")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| {
            root.file_name()
                .map(|name| name.to_string_lossy().to_string())
        })
        .unwrap_or_else(|| "workspace".to_string())
}

fn required(args: &Value, key: &str) -> Result<String, String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| format!("missing string argument `{key}`"))
}

fn limit(args: &Value) -> i64 {
    args.get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(50)
        .clamp(1, 200) as i64
}

fn resolve_symbol(store: &Store, project: &str, symbol: &str) -> Result<String, String> {
    if symbol.contains("::") {
        return Ok(symbol.to_string());
    }
    let conn = store.connection();
    let rows =
        query::search_symbols(conn, project, symbol, 5).map_err(|error| error.to_string())?;
    let mut exact: Vec<&evrax_graph::schema::NodeRow> =
        rows.iter().filter(|row| row.name == symbol).collect();
    match exact.len() {
        0 => match rows.len() {
            0 => Err(format!(
                "symbol `{symbol}` not found; try action=search pattern={symbol}"
            )),
            1 => Ok(rows[0].qualified_name.clone()),
            _ => {
                let names = rows
                    .iter()
                    .map(|row| row.qualified_name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                Err(format!("ambiguous symbol `{symbol}`: {names}"))
            }
        },
        _ => {
            exact.sort_by_key(|row| row.qualified_name.len());
            Ok(exact[0].qualified_name.clone())
        }
    }
}

fn format_nodes(rows: Vec<evrax_graph::schema::NodeRow>) -> String {
    if rows.is_empty() {
        return "no matches".into();
    }
    let mut out = format!("{} results\n", rows.len());
    for row in rows {
        out.push_str(&format!(
            "[{}] {} ({}:{}-{})\n",
            row.label, row.qualified_name, row.file_path, row.start_line, row.end_line
        ));
    }
    truncate(&mut out);
    out.trim_end().to_string()
}

fn outcome(detail: String, ok: bool) -> ToolOutcome {
    ToolOutcome {
        ok,
        detail,
        diff: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ctx() -> (tempfile::TempDir, Context) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(
            dir.path().join("src/main.rs"),
            "fn helper() -> i32 { 1 }\nfn main() { let _ = helper(); }\n",
        )
        .unwrap();
        let root = dir.path().to_path_buf();
        (dir, Context::new(root))
    }

    #[test]
    fn index_search_and_outline() {
        let (_dir, ctx) = ctx();
        let indexed = run(&json!({"action":"index"}), &ctx).unwrap();
        assert!(indexed.detail.contains("nodes:"));
        let search = run(&json!({"action":"search","pattern":"helper"}), &ctx).unwrap();
        assert!(
            search.detail.contains("src/main.rs::helper"),
            "{}",
            search.detail
        );
        let outline = run(&json!({"action":"outline","path":"src/main.rs"}), &ctx).unwrap();
        assert!(outline.detail.contains("helper"), "{}", outline.detail);
        let callers = run(&json!({"action":"callers","symbol":"helper"}), &ctx).unwrap();
        assert!(
            callers.detail.contains("src/main.rs::main"),
            "{}",
            callers.detail
        );
    }

    #[test]
    fn query_before_index_is_actionable() {
        let (_dir, ctx) = ctx();
        let error = run(&json!({"action":"search","pattern":"x"}), &ctx).unwrap_err();
        assert!(error.contains("action=index"), "{error}");
    }

    #[test]
    fn status_before_index_is_info_not_error() {
        let (_dir, ctx) = ctx();
        let status = run(&json!({"action":"status"}), &ctx).unwrap();
        assert!(status.ok, "unindexed status must not be ok:false");
        assert!(
            status.detail.contains("belum di-index"),
            "{}",
            status.detail
        );
        assert!(status.detail.contains("action=index"), "{}", status.detail);
    }

    #[test]
    fn status_on_corrupt_db_is_error() {
        let (_dir, ctx) = ctx();
        let db = default_db_path(&ctx.root);
        if let Some(parent) = db.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&db, b"not a sqlite database").unwrap();
        let err = run(&json!({"action":"status"}), &ctx).unwrap_err();
        assert!(!err.is_empty(), "{err}");
    }
}
