# Bedah codebase-memory-mcp (DeusData) — buat codegraph native Rust Evrax

Repo: https://github.com/DeusData/codebase-memory-mcp — MCP server (ditulis C) yang ngasih coding agent persistent code knowledge graph. Dibedah via `git clone --depth 1`, tanpa install/run.

## 1. Cara kerjanya (ringkas)

- Parse tiap file pake tree-sitter (162 grammar di-vendor ke dalam binary), jalanin **unified AST walker** generik + tabel node-type per bahasa → keluarin simbol & relasi.
- Hasilnya disimpan sebagai knowledge graph di **SQLite** (`~/.cache/codebase-memory-mcp/`), satu DB per user, multi-project.
- Agent query via 17 MCP tools (search, trace call path, impact analysis, Cypher read-only, dsb). Klaim: query sub-milidetik, token ~120x lebih hemat vs baca file satu-satu.
- Ada "Hybrid LSP": implementasi ringan algoritma type-resolution per bahasa (terinspirasi rust-analyzer, pyright, gopls, tsserver) yang di-embed ke binary — buat resolusi `CALLS` yang presisi, tanpa jalanin language server beneran.
- Auto-sync: background watcher deteksi file berubah → re-index incremental otomatis.

## 2. Ekstraksi per bahasa: BUKAN file .scm!

Temuan penting: repo ini **tidak pakai tree-sitter query `.scm` sama sekali**. Sebagai gantinya ada tabel spesifikasi per bahasa di `internal/cbm/lang_specs.c` — struct `CBMLangSpec` berisi daftar **nama node-type tree-sitter** per kategori semantik:

`function_node_types`, `class_node_types`, `field_node_types`, `module_node_types`, `call_node_types`, `import_node_types`, `import_from_types`, `branching_node_types`, `variable_node_types`, `assignment_node_types`, `throw_node_types`, `decorator_node_types` (+ grammar factory per bahasa).

Contoh isi tabel (langsung dari source):

**Rust** (`rust_*`): func = `function_item, function_signature_item, closure_expression`; class = `struct_item, enum_item, union_item, trait_item, type_item, impl_item`; field = `field_declaration`; module = `source_file, mod_item`; call = `call_expression, macro_invocation`; import = `use_declaration, extern_crate_declaration`; var = `static_item, const_item`.

**Python** (`py_*`): func = `function_definition`; class = `class_definition`; module = `module`; call = `call`; import = `import_statement, import`; import_from = `import_from_statement, future_import_statement`; decorator = `decorator`.

**TypeScript** (`ts_*`): func = `function_declaration, generator_function_declaration, function_expression, arrow_function, method_definition, function_signature`; class = `class_declaration, class, abstract_class_declaration, enum_declaration, interface_declaration, type_alias_declaration, internal_module`; decorator = `decorator`.

**Go** (`go_*`): func = `function_declaration, method_declaration, method_elem, func_literal`; class = `type_spec, type_alias, type_declaration`; module = `source_file`; call = `call_expression`; import = `import_declaration, import`; var = `var_declaration, const_declaration`.

Sisanya (155+ bahasa total) polanya sama — tiap bahasa cuma nambah ~10 baris tabel. File: `internal/cbm/lang_specs.c` (~2800 baris), definisi struct di `internal/cbm/lang_specs.h`.

## 3. Node labels & edge types

**Node labels (28):** `Project`, `Package`, `Folder`, `File`, `Module`, `Section`, `Class`, `Struct`, `Interface`, `Trait`, `Enum`, `Type`, `Function`, `Method`, `Field`, `Variable`, `Constant`, `Macro`, `Decorator`, `Route`, `Channel`, `Table`, `Process`, `EnvVar`, `Resource`, `Chart`, `Image`, `Branch`.

**Edge types (inti):** `CONTAINS_FOLDER`, `CONTAINS_FILE`, `DEFINES`, `DEFINES_METHOD`, `IMPORTS`, `CALLS`, `CALL_REFERENCE`, `USAGE`, `INHERITS`, `IMPLEMENTS`, `OVERRIDE`, `DECORATES`, `RAISES`, `THROWS`, `WRITES`, `DATA_FLOWS`, `HTTP_CALLS`, `GRPC_CALLS`, `ASYNC_CALLS`, `EMITS`, `LISTENS_ON`, `HANDLES`, `SPAWNS`, `CONFIGURES`, `INFRA_MAPS`, `CONNECTS_TO`, `DEPLOYS`, `DEPENDS_ON`, `REFERENCES_FILE`, `MENTIONS`, `TESTS`, `TESTS_FILE`, `SIMILAR_TO`, `SEMANTICALLY_RELATED`, `FILE_CHANGES_WITH`, `HAS_BRANCH`, plus cross-repo `CROSS_*` (HTTP/GRPC/GraphQL/tRPC/async/channel).

Buat MVP Evrax, yang kepake beneran cuma ~8: `DEFINES`, `DEFINES_METHOD`, `IMPORTS`, `CALLS`, `USAGE`, `INHERITS`, `IMPLEMENTS`, `CONTAINS_FILE`. Sisanya (infra/K8s/HTTP) bisa nyusul.

## 4. Schema SQLite

Dari `internal/cbm/sqlite_writer.c` — 7 tabel:

- `projects(name PK, indexed_at, root_path)` — daftar project ter-index.
- `file_hashes(project, rel_path, sha256, mtime_ns, size)` — buat deteksi file berubah (incremental).
- `nodes(id PK, project, label, name, qualified_name, file_path, start_line, end_line, properties JSON)` — UNIQUE(project, qualified_name). Index di (project,label), (project,name), (project,file_path).
- `edges(id PK, project, source_id, target_id, type, properties JSON)` — UNIQUE(source_id, target_id, type, local_name_gen). Index di (source_id,type), (target_id,type), (project,target_id,type), (project,source_id,type). Ini yang bikin query callers/callees cepat dua arah.
- `project_summaries(project PK, summary, source_hash, created_at, updated_at)` — ringkasan arsitektur per project.
- `node_vectors(node_id PK, project, vector BLOB)` + `token_vectors(...)` — embedding buat semantic search (opsional, bisa di-skip di MVP).

Pola qualified name: `<project>.<path_parts>.<name>` — dipakai buat lookup presisi.

## 5. 17 MCP tools (nama + fungsi 1 baris)

**Indexing (5):** `index_repository` (index repo ke graf, support async), `list_projects` (daftar project + statistik node/edge), `delete_project` (hapus graf project), `index_status` (status indexing), `check_index_coverage` (cek path/scope udah ter-index & fresh apa belum).

**Querying (12):** `search_graph` (search struktural + BM25 + semantik), `trace_path` (trace caller/callee/data-flow/cross-service, depth 1–15; alias `trace_call_path`), `detect_changes` (petakan git diff → simbol terdampak + blast radius), `query_graph` (Cypher read-only subset), `get_graph_schema` (statistik label/edge — "panggil ini dulu"), `compare_graphs` (diff dua snapshot graf), `get_code_snippet` (baca source fungsi via qualified name), `get_file_outline` (outline deklarasi 1 file), `get_architecture` (overview: bahasa, package, route, hotspot), `search_code` (grep dalam project ter-index), `manage_adr` (CRUD Architecture Decision Records), `ingest_traces` (simpan call path observasi runtime).

## 6. Yang worth dicontek vs ditulis ulang

**CONTEK (desainnya, bukan kodenya):**
1. **Tabel `lang_specs` per bahasa** — ini harta karunnya. Daftar node-type tree-sitter per kategori buat Rust/Python/TS/Go (+150 bahasa lain) tinggal diadaptasi jadi `match` statement atau tabel di Rust. Jauh lebih gampang daripada nulis `.scm` dari nol.
2. **Taksonomi node/edge** — 28 label + edge types di atas udah battle-tested; pakai subset ~8 edge buat MVP.
3. **Schema SQLite** — tabel `nodes`/`edges` + index dua arah `(source_id,type)` & `(target_id,type)` + `file_hashes` buat incremental. Copy polanya 1:1 ke `rusqlite`.
4. **Pola query inti**: `trace_path` inbound/outbound (callers/callees), `detect_changes` (git diff → blast radius = impact analysis), dead-code via "node tanpa edge CALLS masuk" (di Cypher mereka: `WHERE NOT EXISTS { (f)<-[:CALLS]-() }`).
5. **RAM-first pipeline**: index di memory dulu, dump sekali ke SQLite di akhir (LZ4). Memory dilepas setelah selesai.
6. **Hybrid LSP sebagai phase 2** — resolusi tipe ringan per bahasa tanpa jalanin language server; buat Rust bisa manfaatin rust-analyzer sebagai library (sesuai rencana awal).

**TULIS ULANG (jangan port kode C-nya):**
- Seluruh kode C (`internal/cbm/`, `src/`) — translasi literal C→Rust bakal kerasa "C ber-syntax Rust" dan susah dirawat. Tulis ulang native: `tree-sitter` crate + `rusqlite` + `rayon` buat paralel.
- MCP server layer-nya — Evrax butuh built-in (function call langsung), bukan proses eksternal + JSON-RPC. Ngilangin layer ini justru yang bikin ringan.
- CBM Lens (UI browser), session daemon, multi-client coordination — nggak relevan buat built-in.

## 7. Catatan lisensi

**MIT License** (Copyright 2025 DeusData) — boleh dipakai, dimodifikasi, di-port, dipakai di proyek komersial, tanpa wajib buka source. Satu-satunya kewajiban: sertakan teks lisensi + copyright notice kalau lo redistribusi kode yang diambil dari sana. Nggak ada kewajiban copyleft. Aman buat Evrax.

---
*Dibedah 2026-10-20. File sumber acuan: `internal/cbm/lang_specs.c`, `internal/cbm/lang_specs.h`, `internal/cbm/sqlite_writer.c`, README bagian Graph Data Model / MCP Tools / Indexing pipeline / Hybrid LSP.*
