//! End-to-end: index a fixture Rust project and assert the smart
//! resolver picks the right targets (esp. the name-collision case).

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn fixture_root() -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join(format!("testwork-{n}"));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("src/main.rs"),
        r#"mod util;
mod other;

use crate::util::helper;
use crate::other::run as other_run;

struct Server;

impl Server {
    fn start(&self) -> String {
        helper()
    }
}

fn run() -> String {
    let s: Server = Server;
    let _ = s.start();
    let _ = other_run();
    crate::util::helper()
}

fn main() {
    run();
}
"#,
    )
    .unwrap();
    std::fs::write(
        root.join("src/util.rs"),
        "pub fn helper() -> String { \"h\".to_string() }\n",
    )
    .unwrap();
    std::fs::write(
        root.join("src/other.rs"),
        "pub fn run() -> String { \"o\".to_string() }\n",
    )
    .unwrap();
    root
}

fn call_targets(db: &std::path::Path, project: &str, caller_qn: &str) -> Vec<String> {
    let store = evrax_graph::Store::open(db).unwrap();
    let rows = evrax_graph::query::callees(store.connection(), project, caller_qn).unwrap();
    let mut v: Vec<String> = rows.into_iter().map(|n| n.qualified_name).collect();
    v.sort();
    v
}

#[test]
fn rust_smart_resolution() {
    let root = fixture_root();
    let db = evrax_graph::default_db_path(&root);
    let stats = evrax_graph::index_project(&root, &db, "t").unwrap();
    assert!(stats.nodes > 10, "expected nodes, got {}", stats.nodes);
    assert!(stats.edges > 10, "expected edges, got {}", stats.edges);

    // `other_run()` (aliased import) must resolve to other.rs::run,
    // NOT to the same-file `run` (the old name-based resolver got this wrong).
    let targets = call_targets(&db, "t", "src/main.rs::run");
    assert!(
        targets.iter().any(|t| t == "src/other.rs::run"),
        "aliased call should hit other::run, got {targets:?}"
    );

    // `helper()` via `use` import -> util.rs::helper
    assert!(
        targets.iter().any(|t| t == "src/util.rs::helper"),
        "imported call should hit util::helper, got {targets:?}"
    );

    // `s.start()` via `let s: Server` -> impl Server::start
    assert!(
        targets
            .iter()
            .any(|t| t == "src/main.rs::impl Server::start"),
        "receiver-typed call should hit impl Server::start, got {targets:?}"
    );

    // callers of util::helper include both run and Server::start
    let store = evrax_graph::Store::open(&db).unwrap();
    let callers =
        evrax_graph::query::callers(store.connection(), "t", "src/util.rs::helper").unwrap();
    let names: Vec<_> = callers.iter().map(|n| n.qualified_name.as_str()).collect();
    assert!(names.contains(&"src/main.rs::run"), "got {names:?}");
    assert!(
        names.contains(&"src/main.rs::impl Server::start"),
        "got {names:?}"
    );

    // dead code: only `main` (entry point); other::run must NOT be flagged
    let dead = evrax_graph::query::dead_code(store.connection(), "t").unwrap();
    let dead_names: Vec<_> = dead.iter().map(|n| n.qualified_name.as_str()).collect();
    assert!(
        !dead_names.contains(&"src/other.rs::run"),
        "other::run is called, shouldn't be dead: {dead_names:?}"
    );

    let _ = std::fs::remove_dir_all(&root);
}
