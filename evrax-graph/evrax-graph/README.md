# evrax-graph

Lightweight multi-language **code graph** for Evrax — written in Rust, built in
(no MCP server, no extra process).

## What it does

Parses a codebase with tree-sitter, builds a symbol graph
(files, modules, classes/structs/enums/traits, functions/methods, variables)
plus relationships (defines, calls, imports), and stores it in a single
SQLite file: `<project>/.evrax/graph.db`.

The agent then queries the graph instead of reading whole files:

- `search_symbols` — find symbols by name
- `callers` / `callees` — 1-hop call graph
- `impact` — transitive callers ("what breaks if I change this?")
- `dead_code` — functions nobody calls
- `file_outline` — symbols in one file

## How it works

Per-language extraction uses **node-type tables** (`src/lang.rs`): for each
language, tree-sitter node-kind names grouped by semantic category
(function/class/call/import/...). Adding a language ≈ 10 lines of table +
its tree-sitter grammar crate. Currently 12 languages: Rust, Python,
TypeScript/TSX, Go, JavaScript (js/jsx/mjs/cjs), Java, C, C++, C#, PHP, Ruby,
Bash. Not yet: Kotlin, Swift, Lua, Dart (their crates.io grammar crates are
too old — tree-sitter ABI incompatible with the 0.24 runtime).

Indexing is incremental: `file_hashes` (sha256 + mtime) means only changed
files get re-parsed (parallel via rayon).

Call resolution has two tiers:
- **Phase 1 (all languages):** name-based, same-file preferred.
- **Phase 2 (Rust, `src/resolve.rs`):** smart resolver — `use`-import aware
  (incl. aliases & globs), qualified paths (`crate::`/`self::`/`super::`),
  associated functions (`Type::assoc_fn`), and receiver types from
  `let x: Type` bindings (`x.method()` -> `impl Type::method`).
  No rust-analyzer dependency — all derived from tree-sitter + the graph.

## Design credit

Schema pattern, node/edge taxonomy, and the lang-spec-table idea are ported
(as design, rewritten in Rust) from
[codebase-memory-mcp](https://github.com/DeusData/codebase-memory-mcp)
(MIT © 2025 DeusData). See NOTICE.

## Try it

```sh
# index a project (writes <dir>/.evrax/graph.db)
cargo run --bin evrax-graph -- index /path/to/project

# query it
cargo run --bin evrax-graph -- search helper /path/to/project
cargo run --bin evrax-graph -- callers src/main.rs::run /path/to/project
cargo run --bin evrax-graph -- callees src/main.rs::run /path/to/project
cargo run --bin evrax-graph -- impact src/util.rs::helper /path/to/project --depth 3
cargo run --bin evrax-graph -- dead-code /path/to/project
cargo run --bin evrax-graph -- outline src/main.rs /path/to/project
```

Or install it: `cargo install --path .`, then `evrax-graph` is on your PATH.

## Tests

```sh
cargo test   # 5 unit tests + 1 end-to-end fixture test
```
