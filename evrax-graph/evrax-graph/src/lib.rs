//! evrax-graph — lightweight multi-language code graph, built into Evrax.
//!
//! Design ported (not code-copied) from codebase-memory-mcp (MIT, DeusData):
//! per-language tree-sitter node-type tables, node/edge taxonomy, and the
//! SQLite storage pattern with bidirectional edge indexes.
//!
//! Layout on disk: `<project>/.evrax/graph.db` (single SQLite file).

pub mod extract;
pub mod index;
pub mod lang;
pub mod query;
pub mod resolve;
pub mod schema;
pub mod store;

pub use index::{default_db_path, index_project, Stats};
pub use store::Store;

/// Convenience error alias used across the crate.
pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
