//! Demo: index a directory into `.evrax/graph.db` and run the 4 core queries.
//!
//! Usage: `cargo run --example demo -- [dir]`
//! Dogfood default: indexes this crate's own source.

use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let dir: PathBuf = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let dir = dir.canonicalize()?;
    let project = "demo";

    let db = evrax_graph::default_db_path(&dir);
    println!("indexing {} ...", dir.display());
    let stats = evrax_graph::index_project(&dir, &db, project)?;
    println!("db: {}", db.display());
    println!(
        "files seen: {}, indexed this run: {}, nodes: {}, edges: {}",
        stats.files_seen, stats.files_indexed, stats.nodes, stats.edges
    );

    let store = evrax_graph::Store::open(&db)?;
    let conn = store.connection();
    use evrax_graph::query;

    // 1. symbol search
    println!("\n-- search 'index' --");
    for n in query::search_symbols(conn, project, "index", 5)? {
        println!(
            "  [{}] {} ({}:{})",
            n.label, n.qualified_name, n.file_path, n.start_line
        );
    }

    // 2. callers / callees of the indexer entry point
    let target = query::search_symbols(conn, project, "index_project", 3)?
        .into_iter()
        .find(|n| n.label == "Function");
    if let Some(t) = target {
        println!("\n-- callers of {} --", t.qualified_name);
        for n in query::callers(conn, project, &t.qualified_name)? {
            println!("  {} ({}:{})", n.qualified_name, n.file_path, n.start_line);
        }
        println!("\n-- impact (transitive callers, depth 3) --");
        for n in query::impact(conn, project, &t.qualified_name, 3)? {
            println!("  {} ({}:{})", n.qualified_name, n.file_path, n.start_line);
        }
    }

    // 3. dead code candidates
    println!("\n-- dead code candidates (first 10) --");
    for n in query::dead_code(conn, project)?.into_iter().take(10) {
        println!(
            "  [{}] {} ({}:{})",
            n.label, n.qualified_name, n.file_path, n.start_line
        );
    }

    Ok(())
}
