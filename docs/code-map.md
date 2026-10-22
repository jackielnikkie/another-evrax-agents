# Peta kode evrax

Di-refresh 2026-10-21. **Jangan pakai nomor baris** — kode sering geser; cari simbol pakai tool `evrax-graph` (`graph search`/`outline`/`callers`) atau `rg`. Halaman ini cuma peta modul + alur.

## Crate (workspace `Cargo.toml`)

| Crate | Isi |
|---|---|
| `crates/evrax-core` | Kontrak: `TurnRequest { prompt, session, plan_mode }`, `AgentEvent`, trait `AgentBackend`, `MockBackend`, `DiffLine`/`DiffKind` |
| `crates/evrax-agent` | Backend LLM asli + agent loop + tools + catalog |
| `crates/evrax-remote` | Server web remote (`tiny_http`) + SSE + tunnel |
| `crates/evrax-tui` | Aplikasi TUI (ratatui) + banner + markdown + tema |
| `evrax-graph/evrax-graph` | Indexer graph kode (SQLite) + CLI, dipakai tool `evrax-graph` |

### evrax-agent
- `lib.rs` — `ChatBackend`: config multi-provider (`agent.json` `{active, providers[]}` → `EVRAX_*` → CLI, migrasi file flat lama di `file_state()`), `resolve`/`ConfigSettings::resolved()`, `list_models` (cek key via `GET /models`), `test_provider`, `stream_round` (cabang `ProviderKind`), loop agent (default 500 iterasi tool-round, `commit_partial`, `finish_budget`/summary toolless), `suggest_title`, `provider_error` (hint 9router hanya untuk URL 20128), `authorize_and_run`.
- `anthropic.rs` — konversi [OI]→Messages API (`convert_messages` system/tool_use/tool_result, `convert_tools` `input_schema`, `request_body` `max_tokens=8192`), `StreamAcc`+`apply_event` (SSE `content_block_*`, `input_json_delta`, `message_stop`, `error`), `reply_text`/`error_message`. History di disk tetap format [OI].
- `sse.rs` — `SseDecoder` + `ToolCallAccumulator` (menangani frame dengan `"id":""`).
- `retry.rs` — retry 429/5xx/408/transport/`x-should-retry`, tanpa retry 4xx lain.
- `guardrails.rs` — deteksi streak/siklus tool-call identik.
- `diff.rs` — diff `similar`.
- `tools.rs` — `specs()` 12 tool (`read list grep write edit shell web_fetch todo git ast_grep evrax-graph subagent`), `Context::classify()`, `outside_target()`, `approval_key()`, `is_mutating/requires_approval/approval_key_for`, plan mode read-only.
- `tools/{web,todo,git,ast_grep,graph}.rs` — implementasi tool non-core.
- `catalog.rs` — `Catalog::load()`: tools + skills (scan `skill-evrax/` + `%APPDATA%\evrax\skills`, override `EVRAX_SKILLS_DIR`) + MCP (`%APPDATA%\evrax\mcp.json`, override `EVRAX_MCP_CONFIG`).
- `subagent.rs` — state `%APPDATA%\evrax\subagents\<id>\` (inbox `prompt.txt`, `stop`).

### evrax-remote
- `lib.rs` — `RemoteConfig{password,bind,port,public_url}`, `Permit`, `Session`, `RemoteEvent` (termasuk `Approval`/`ApprovalResolved`), token bearer + expiry 24 h, rate-limit `/login`, `spawn_stream` SSE flush per frame + heartbeat, endpoint `/api/session/{id}/{prompt,mode,approval}`, `rename_session`, `take_pending_prompt/approval`, web UI Vue 3 CDN single HTML (`include_str!`).
- `tunnel.rs` — `TunnelManager` quick tunnel + `start_domain()` (cloudflared login/create/route dns), `generation` counter anti-orphan.

### evrax-tui
- `main.rs` — event loop, pilih backend real/mock, polling remote + subagent, drain prompt/approval web, spawn job judul LLM, arg parsing.
- `app.rs` — `App`: transcript per sesi, composer/`LineEditor`, slash command + palette, dialog (session/theme/remote/tunnel/approval/preview/model), wizard `WizardStep` + `ModelDialog` (fetch `/models` async via `FetchResult`), `TranscriptCache`, retensi sesi, `titles`/`manual_titles`, `backend_label`, iteration footer, `providers`/`active_provider` + `apply_settings`.
- `ui.rs` — render: header/transcript/composer/footer, modal (`dialog_window` + PgUp/PgDn), diff wrap, click target sebagian (tunnel URL copy).
- `banner.rs` + `banner_wide.txt`/`banner_compact.txt` — sprite half-block truecolor, glow, bob, blink, wordmark `E V R A X`, signature `FAUST55`.
- `welcome.rs` — empty state 3 kolom: skills | banner | MCP + tools.
- `theme.rs` — 7 tema (`evrax`, `spectrum`, catppuccin, dracula, gruvbox, nord, tokyonight), simpan pilihan ke `%APPDATA%\evrax\theme`.
- `markdown.rs`, `image.rs` (paste + preview kitty), `window.rs` (maximized/resize), `subagent_link.rs` (poll inbox 250 ms).

### evrax-graph
`schema.rs`/`extract.rs`/`lang.rs`/`resolve.rs`/`index.rs`/`store.rs`/`query.rs` + `main.rs` (CLI). DB `<root>/.evrax/graph.db`; indexer skip `agent-reference`, `skill-evrax`, `.serena`, `.codebase-memory`. Catatan desain (hasil bedah tool graph lain) ada di `docs/evrax-graph-notes.md`.

## Alur utama
1. Prompt → `App::start_turn` → `AgentBackend::run(TurnRequest)` → `ChatBackend` loop: request SSE → delta `AgentEvent` (`TextDelta`/`ThinkingDelta`/`Iteration`/`ToolCall`/`Notice`/`Retrying`) → eksekusi tool → commit history per turn → `TurnFinished`.
2. Tool mutating / path luar workspace → `AgentEvent::ApprovalRequested{key}` → dialog TUI (Once/Always/Reject) atau tombol di web → `authorize_and_run`.
3. Plan mode → tools di-filter read-only; mutating → `blocked: plan mode is read-only`.
4. Remote ON → `App` register ke `RemoteServer`; prompt/mode/approval dua arah via antrean + SSE broadcast; TUI authoritative, web ikut poll.
5. Judul sesi: heuristik instan saat prompt pertama, LLM (`suggest_title`) menyusul, `/rename` manual sticky, id sesi tidak pernah berubah.

## Keybind & interaksi
Ctrl+C quit · Ctrl+P palette · Ctrl+O sesi · Ctrl+N sesi baru · Ctrl+Y copy transcript · Tab/BackTab mode Plan/Build · Enter kirim · Ctrl+J newline · PgUp/PgDn scroll & dialog page · Up/Down history/move · Esc cancel/close · drag seleksi auto-copy · klik URL tunnel = copy.

## Test
- `cargo test --workspace` → 240 (72 agent, 5 core, 5 graph unit, 1 graph integration, 29 remote, 128 tui).
- Live smoke (butuh 9router + key): `cargo test -p evrax-agent -- --ignored` → 2 test.
- Render test TUI pakai `ratatui` `TestBackend` (mis. 100x60); hindari kata "Build"/"Plan" di teks demo.
- Gate: `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`.

## Konvensi
- Toolchain GNU pinned; prefix PATH tiap shell (lihat `AGENTS.md`).
- Jangan `Set-Content`/`Out-File` PS 5.1 untuk file source (BOM).
- `build`/`run` terkunci kalau instance evrax hidup.