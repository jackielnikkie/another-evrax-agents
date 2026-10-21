//! Incremental project indexer.
//!
//! Walk the project, sha256 each source file, re-parse only what changed
//! (tracked in `file_hashes`), then resolve calls/imports. Parsing is
//! parallel via rayon; DB writes stay sequential.

use rayon::prelude::*;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

use crate::extract::{extract_file, FileGraph, RawCall};
use crate::lang::{find_spec, language_for};
use crate::resolve::RustResolver;
use crate::schema::{EdgeKind, NodeLabel};
use crate::store::Store;
use crate::Result;

const SKIP_DIRS: &[&str] = &[
    ".git",
    ".evrax",
    "target",
    "node_modules",
    "dist",
    "build",
    "vendor",
    "__pycache__",
    ".venv",
    "venv",
    "agent-reference",
    "skill-evrax",
    ".serena",
    ".codebase-memory",
];
const MAX_FILE_BYTES: u64 = 512 * 1024;

/// Default graph location inside a workspace: `<root>/.evrax/graph.db`.
pub fn default_db_path(project_root: &Path) -> PathBuf {
    project_root.join(".evrax").join("graph.db")
}

#[derive(Debug, Default)]
pub struct Stats {
    pub files_seen: usize,
    pub files_indexed: usize,
    pub nodes: usize,
    pub edges: usize,
}

pub fn index_project(root: &Path, db_path: &Path, project: &str) -> Result<Stats> {
    let store = Store::open(db_path)?;
    store.upsert_project(project, &root.to_string_lossy())?;

    // 1. collect candidate files
    let mut files: Vec<(PathBuf, String, String)> = Vec::new(); // (abs, rel, ext)
    for entry in WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            if e.file_type().is_dir() {
                !SKIP_DIRS.contains(&e.file_name().to_string_lossy().as_ref())
            } else {
                true
            }
        })
    {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        if !entry.file_type().is_file() {
            continue;
        }
        let ext = entry
            .path()
            .extension()
            .map(|s| s.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        if find_spec(&ext).is_none() {
            continue;
        }
        let rel = entry
            .path()
            .strip_prefix(root)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        files.push((entry.path().to_path_buf(), rel, ext));
    }
    let file_set: HashSet<String> = files.iter().map(|(_, r, _)| r.clone()).collect();

    // 2. hash + keep only changed files (read once, reuse bytes below)
    struct Changed {
        rel: String,
        ext: String,
        bytes: Vec<u8>,
        sha256: String,
        mtime_ns: i64,
        size: i64,
    }
    let mut changed: Vec<Changed> = Vec::new();
    for (abs, rel, ext) in &files {
        let meta = match std::fs::metadata(abs) {
            Ok(m) => m,
            Err(_) => continue,
        };
        if meta.len() > MAX_FILE_BYTES {
            continue;
        }
        let mtime_ns = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos() as i64)
            .unwrap_or(0);
        let bytes = match std::fs::read(abs) {
            Ok(b) => b,
            Err(_) => continue,
        };
        let mut h = Sha256::new();
        h.update(&bytes);
        let sha = format!("{:x}", h.finalize());
        let fresh = match store.get_file_hash(project, rel)? {
            Some((old_sha, old_mtime)) => old_sha == sha && old_mtime == mtime_ns,
            None => false,
        };
        if !fresh {
            changed.push(Changed {
                rel: rel.clone(),
                ext: ext.clone(),
                bytes,
                sha256: sha,
                mtime_ns,
                size: meta.len() as i64,
            });
        }
    }

    // 3. parse changed files in parallel
    struct Parsed {
        rel: String,
        sha256: String,
        mtime_ns: i64,
        size: i64,
        graph: FileGraph,
    }
    let parsed: Vec<Parsed> = changed
        .par_iter()
        .filter_map(|c| {
            let source = String::from_utf8_lossy(&c.bytes);
            let spec = find_spec(&c.ext)?;
            let language = language_for(&c.ext)?;
            let graph = extract_file(spec, language, &c.rel, &source, &file_set);
            Some(Parsed {
                rel: c.rel.clone(),
                sha256: c.sha256.clone(),
                mtime_ns: c.mtime_ns,
                size: c.size,
                graph,
            })
        })
        .collect();

    // 4. write to DB (sequential)
    // qualified_name -> id for everything inserted this run
    let mut qn_to_id: HashMap<String, i64> = HashMap::new();
    // raw calls for phase 2
    let mut pending_calls: Vec<RawCall> = Vec::new();
    // (importer_file, target_file) for phase 2b (after ALL files are in)
    let mut pending_imports: Vec<(String, String)> = Vec::new();

    for p in &parsed {
        store.delete_file_data(project, &p.rel)?;
        store.delete_file_aux(project, &p.rel)?;
        // nodes[0] is always the file node
        let mut idx_to_id: Vec<i64> = Vec::with_capacity(p.graph.nodes.len());
        for node in &p.graph.nodes {
            let id = store.insert_node(project, node)?;
            idx_to_id.push(id);
            qn_to_id.insert(node.qualified_name.clone(), id);
        }
        // DEFINES / DEFINES_METHOD / CONTAINS_FILE edges
        for (i, node) in p.graph.nodes.iter().enumerate() {
            if i == 0 {
                continue; // file node itself
            }
            let parent_idx = node.parent.unwrap_or(0);
            let parent_id = idx_to_id[parent_idx];
            let child_id = idx_to_id[i];
            let kind = if node.label == NodeLabel::Method {
                EdgeKind::DefinesMethod
            } else {
                EdgeKind::Defines
            };
            store.insert_edge(project, parent_id, child_id, kind)?;
            // every symbol is also contained in its file
            store.insert_edge(project, idx_to_id[0], child_id, EdgeKind::ContainsFile)?;
        }
        for c in &p.graph.calls {
            pending_calls.push(c.clone());
        }
        for im in &p.graph.imports {
            pending_imports.push((im.importer_file.clone(), im.target_file.clone()));
        }
        // phase-2 aux data
        for u in &p.graph.uses {
            store.insert_use(project, u)?;
        }
        for l in &p.graph.lets {
            store.insert_let(project, &p.rel, l)?;
        }
        store.set_file_hash(project, &p.rel, &p.sha256, p.mtime_ns, p.size)?;
    }

    // 4b. resolve IMPORTS (file -> file), now that every file node exists
    for (importer, target) in &pending_imports {
        if let (Some(src), Some(dst)) = (
            store.node_id(project, importer).ok().flatten(),
            store.node_id(project, target).ok().flatten(),
        ) {
            store.insert_edge(project, src, dst, EdgeKind::Imports)?;
        }
    }

    // 5. resolve CALLS
    // 5a. name-based index (fallback for every language)
    // name -> [(id, file)]
    let mut name_to_nodes: HashMap<String, Vec<(i64, String)>> = HashMap::new();
    {
        let conn = store.connection();
        let mut stmt = conn.prepare(
            "SELECT id, name, file_path FROM nodes
             WHERE project = ?1 AND label IN ('Function', 'Method')",
        )?;
        let rows = stmt.query_map([project], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        for row in rows {
            let (id, name, file) = row?;
            name_to_nodes.entry(name).or_default().push((id, file));
        }
    }
    // 5b. Rust smart resolver (phase 2): import/path/receiver aware
    let rs_files: Vec<String> = files
        .iter()
        .filter(|(_, _, ext)| ext == "rs")
        .map(|(_, rel, _)| rel.clone())
        .collect();
    let resolver = RustResolver::build(
        &rs_files,
        store.all_uses(project)?,
        store.all_lets(project)?,
        store.all_node_infos(project)?,
    );
    let id_of = |qn: &str| -> Option<i64> {
        if let Some(id) = qn_to_id.get(qn).copied() {
            Some(id)
        } else {
            store.node_id(project, qn).ok().flatten()
        }
    };
    let name_based = |callee: &str, caller_file: &str| -> Option<i64> {
        let cands = name_to_nodes.get(callee)?;
        // prefer a target in the same file, else first candidate
        cands
            .iter()
            .find(|(_, f)| f == caller_file)
            .or_else(|| cands.first())
            .map(|(id, _)| *id)
    };
    for call in &pending_calls {
        let caller_id = match id_of(&call.caller_qualified) {
            Some(id) => id,
            None => continue,
        };
        // Rust: smart resolver first, name-based fallback.
        // Other languages: name-based only.
        let target = if call.caller_file.ends_with(".rs") {
            resolver
                .resolve(call, &call.caller_qualified)
                .or_else(|| name_based(&call.callee_name, &call.caller_file))
        } else {
            name_based(&call.callee_name, &call.caller_file)
        };
        if let Some(tid) = target {
            if tid != caller_id {
                store.insert_edge(project, caller_id, tid, EdgeKind::Calls)?;
            }
        }
    }

    let (nodes, edges) = store.count(project)?;
    Ok(Stats {
        files_seen: files.len(),
        files_indexed: parsed.len(),
        nodes: nodes as usize,
        edges: edges as usize,
    })
}
