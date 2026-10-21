//! evrax-graph CLI: index a project into `.evrax/graph.db` and query it.
//!
//!   evrax-graph index [DIR] [--project NAME]
//!   evrax-graph search <PATTERN> [DIR]
//!   evrax-graph callers <SYMBOL> [DIR]
//!   evrax-graph callees <SYMBOL> [DIR]
//!   evrax-graph impact <SYMBOL> [DIR] [--depth N]
//!   evrax-graph dead-code [DIR]
//!   evrax-graph outline <FILE> [DIR]
//!
//! DIR defaults to the current directory. `--project` defaults to the
//! directory name (must match between `index` and query commands).

use std::path::{Path, PathBuf};

use evrax_graph::{default_db_path, index_project, query, schema::NodeRow, Store};

type Res<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

fn main() -> Res<()> {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let cmd = raw.first().map(|s| s.as_str()).unwrap_or("help");

    // tiny flag parser: --project NAME, --depth N (both `--x v` and `--x=v`)
    let mut project: Option<String> = None;
    let mut depth: usize = 5;
    let mut pos: Vec<String> = Vec::new();
    let mut it = raw.iter().skip(1).peekable();
    while let Some(a) = it.next() {
        if let Some(v) = a.strip_prefix("--project=") {
            project = Some(v.to_string());
        } else if let Some(v) = a.strip_prefix("--depth=") {
            depth = v.parse().unwrap_or(5);
        } else {
            match a.as_str() {
                "--project" => project = it.next().cloned(),
                "--depth" => depth = it.next().and_then(|s| s.parse().ok()).unwrap_or(5),
                _ => pos.push(a.clone()),
            }
        }
    }
    let _ = it;

    match cmd {
        "index" => {
            let dir = dir_of(pos.first());
            let name = project.unwrap_or_else(|| project_name(&dir));
            let db = default_db_path(&dir);
            let stats = index_project(&dir, &db, &name)?;
            println!("project: {name}");
            println!("db: {}", db.display());
            println!(
                "files: {} seen, {} indexed | nodes: {} | edges: {}",
                stats.files_seen, stats.files_indexed, stats.nodes, stats.edges
            );
        }
        "search" | "callers" | "callees" | "impact" | "dead-code" | "outline" => {
            let need_arg = cmd != "dead-code";
            let sym = if need_arg {
                match pos.first() {
                    Some(s) => s.clone(),
                    None => die(&format!("{cmd} needs an argument")),
                }
            } else {
                String::new()
            };
            let dir = dir_of(pos.get(if need_arg { 1 } else { 0 }));
            let name = project.unwrap_or_else(|| project_name(&dir));
            let store = Store::open(&default_db_path(&dir))?;
            let conn = store.connection();
            let rows: Vec<NodeRow> = match cmd {
                "search" => query::search_symbols(conn, &name, &sym, 50)?,
                "callers" => query::callers(conn, &name, &sym)?,
                "callees" => query::callees(conn, &name, &sym)?,
                "impact" => query::impact(conn, &name, &sym, depth)?,
                "dead-code" => query::dead_code(conn, &name)?,
                "outline" => query::file_outline(conn, &name, &sym)?,
                _ => unreachable!(),
            };
            for n in rows {
                println!(
                    "[{}] {} ({}:{}-{})",
                    n.label, n.qualified_name, n.file_path, n.start_line, n.end_line
                );
            }
        }
        _ => print_help(),
    }
    Ok(())
}

fn dir_of(arg: Option<&String>) -> PathBuf {
    let p = arg.map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    p.canonicalize().unwrap_or(p)
}

fn project_name(dir: &Path) -> String {
    dir.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "project".to_string())
}

fn die(msg: &str) -> ! {
    eprintln!("error: {msg}");
    std::process::exit(1);
}

fn print_help() {
    println!(
        "evrax-graph — lightweight multi-language code graph

USAGE:
    evrax-graph index [DIR] [--project NAME]
    evrax-graph search <PATTERN> [DIR]
    evrax-graph callers <SYMBOL> [DIR]
    evrax-graph callees <SYMBOL> [DIR]
    evrax-graph impact <SYMBOL> [DIR] [--depth N]
    evrax-graph dead-code [DIR]
    evrax-graph outline <FILE> [DIR]

SYMBOL is a qualified name like `src/main.rs::run`.
The graph is stored at <DIR>/.evrax/graph.db."
    );
}
