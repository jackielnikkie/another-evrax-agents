# Todofix v0.1.3 — batch perbaikan (2026-10-22)

Batch fix post-`v0.1.3`. Tujuh fix + seed config. **Tidak tag/release** — hanya commit lokal + push `origin` + `another`.

Status akhir: **SELESAI**, gate hijau (lihat bagian Gate).

## FIX-001 — aturan bahasa di system_prompt

- `crates/evrax-agent/src/lib.rs::system_prompt()` kini punya aturan: balas mengikuti bahasa user (Indonesia→Indonesia, Inggris→Inggris), jangan ganti bahasa sendiri; code/identitas/path/command/error string tetap aslinya; string yang tampil di UI evrax tetap Inggris.
- Test: `language_tests::system_prompt_matches_user_language_and_protects_english_ui`, `::system_prompt_includes_plan_mode_caveat`.

## FIX-002 — quiet transcript (default)

- `ui.rs::entry_lines(entry, tick, width, expanded)` — mode default **quiet**:
  - `Thinking` → 1 baris (`wrap_text` lalu potong ke baris pertama + `…`).
  - Tool `Done` → header saja (tanpa body).
  - Tool `Preparing`/`Running` → header + 1 baris status.
  - Tool `Error` → maks 4 baris (`QUIET_ERROR_LINES`) + `… +N lines · ctrl+t`.
  - Diff → maks 40 baris (`DIFF_PREVIEW_LINES`) + `… +N more · ctrl+t`.
- Toggle: **`ctrl+t` / `ctrl+o`** → `App::toggle_expanded()` (flag `App.expanded`, invalidate `TranscriptCache` via `TranscriptCache::set_expanded`).
  - Deviasi: `ctrl+o` dulunya membuka dialog sesi; sesi tetap bisa dibuka via `/sessions` + palette `ctrl+p`.
- Test: `quiet_transcript_hides_tool_bodies_until_expanded`, `quiet_errors_keep_four_lines_and_count_the_rest`, `quiet_diffs_stop_at_the_preview_cap`.

## FIX-003 — antre prompt saat streaming

- `App.queued: VecDeque<String>` (cap `MAX_QUEUED_PROMPTS = 8`).
- `submit_prompt()` busy → text-only diantre (`queue_prompt`, flash `queued (n)`); ada attachment → flash `not available while streaming (attachments)` dan draft tidak diambil.
- `submit_external_prompt()` (dari web) juga ikut antre.
- `App::take_queued_prompt()` (idle saja) → `begin_turn()` (push user msg, placeholder title, `busy`, `turn_started`, echo remote).
- Drain: `main.rs::drain_queued_prompt()` pada ticker 4 Hz, setelah `drain_remote_prompts`.
- Slash **tetap jalan saat streaming** (`slash_sync` buang syarat `!busy`); perintah `NewSession` + `Rename` ditolak dengan flash `not available while streaming`.
- `cancel_turn()` / `new_session` / `resume` → antrean dibersihkan.
- Footer menampilkan `queued N` di dalam status.
- Test: `queued_prompts_run_after_the_current_turn`, `queue_is_capped_and_cancel_clears_it`, `mutating_slash_is_blocked_while_streaming_read_only_is_not`.

## FIX-004 — embed 42 skills via build.rs

- Baru: `crates/evrax-agent/build.rs` → scan `skill-evrax/**/SKILL.md` (depth 6, skip `.git` + `bin/`), tulis `<OUT_DIR>/embedded_skills.rs` berisi `EMBEDDED_SKILLS: &[(&str, &str)]` (label relatif + `include_str!` absolut). Build mencetak `cargo:warning=evrax-agent@0.1.3: evrax: embedded 42 SKILL.md …`. Dir hilang → array kosong, build tetap jalan.
- `catalog.rs`: `SkillInfo` + field `content: String`; `Catalog::load()` = disk dulu (EVRAX_SKILLS_DIR → %APPDATA%\evrax\skills → `<exe_dir>\skills` → `cwd/skill-evrax`), lalu embedded di-merge `or_insert` (disk menang bila nama sama). `load_from()` tetap disk-only supaya test lama `skills_are_sorted_and_deduplicated` hijau. Path repo-relatif lama (`default_skill_dirs`) dihapus karena mati pada binary terinstal.
- `skill-evrax/` terverifikasi **42 SKILL.md** (222 file total termasuk `bin/sg.exe` 52 MB → bin di-exclude).
- Test: `catalog::tests::embedded_skills_are_bundled` (42, ada `humanizer`, `systematic-debugging`, `test-driven-development`, content non-kosong).

## FIX-005 — edit/write CRLF (tools.rs)

- Helper `eol_of` / `to_lf` / `from_lf` / `has_mixed_eol` / `nearest_lines` (`tools.rs`).
- `edit()`: kerja di haystack LF (`to_lf` teks + `old` + `new`), hitung match di LF, tulis `from_lf(updated, eol)`; diff dihitung LF vs LF (CRLF vs LF tidak lagi "seluruh file berubah"). Error `text not found` kini menyebut kandidat baris terdekat + `(file uses CRLF)` (dan hint mixed-EOL).
- `write()`: file ada → payload di-EOL-kan ke EOL file; file baru → ikut payload. `read()` tetap LF-view (dibiarkan).
- Test: `edit_matches_lf_needle_in_crlf_file`, `edit_keeps_lf_file_lf`, `edit_preserves_missing_trailing_newline`, `write_keeps_existing_crlf`, `edit_reports_crlf_hint`.
- Batas: file mixed-EOL dinormalisasi ke EOL dominan (tidak klaim byte-identik).

## FIX-006 — ToolPreparing + footer elapsed + diff cap

- `evrax-core`: varian baru `AgentEvent::ToolPreparing { name, bytes }`.
- `evrax-agent`: throttle `PREP_STEP_BYTES = 4 KB` / `PREP_INTERVAL = 200 ms` saat argumen tool streaming (`StreamState::prep`); sebelum `ApprovalRequested` juga dikirim (approval pending = preparing).
- `evrax-tui`: `ToolStatus::Preparing` baru; arm `ToolPreparing` di `apply_event` membuat/meng-upgrade baris placeholder — `ToolStarted` **meng-upgrade baris yang sama** (1 baris per tool call). `ToolFinished` menangani `Running | Preparing`.
- Footer: `App.turn_started: Option<Instant>` (set `begin_turn`, clear di TurnFinished/Failed/cancel/session switch) → status `working · 1m 42s · iteration 3/500 · queued N`. Hint `ctrl+c quit` saat busy dihapus (ruang untuk `queued N` di terminal 100 kolom).
- Diff render: cap default 40 baris + `… +N more · ctrl+t`; `ctrl+t` buka sampai 200 (batas produksi `diff.rs::MAX_DIFF_LINES`).
- Test: `streamed_tool_arguments_emit_throttled_preparing`, `tool_preparing_placeholder_is_upgraded_in_place`, `turn_started_is_tracked_for_the_footer_clock`, `footer_shows_elapsed_and_queue`, `elapsed_formatting_covers_seconds_minutes_hours`.

## FIX-007 — password lemah / tunnel

- `evrax-remote`: `pub fn is_weak_password()` = kosong / whitespace / `<8` / `"12345678"`; `is_default_password()` jadi alias. Dipakai di gate `allow_remote` (**fix: password kosong dulu lolos dari loopback-only**) dan `/login` (body password kosong → 401 sebelum `ct_eq`).
- `evrax-tui` guard: `tunnel_select_quick`, `tunnel_edit_commit` cabang `Hostname` (return `Err` SEBELUM editing dibuka + state diubah → `HostReady` tidak mungkin untuk password lemah), `tunnel_follow_remote`, `tunnel_run`, `remote_toggle` baris enable, `remote_edit_commit` (tolak `12345678` + whitespace: `choose a different password (the default is not allowed)`).
- `ui.rs` baris password: `not set` / `default · set a password` / bullets kuat (bukan 8 titik palsu); baris tunnel quick/domain diberi `· password required` selama password lemah.
- Test baru: remote `weak_password_covers_empty_blank_short_and_default`, `weak_password_is_loopback_only_like_the_default`, `weak_password_blocks_remote_clients_and_empty_login`; app `tunnel_hostname_commit_refused_with_weak_password`, `remote_toggle_enable_refused_with_weak_password`, `tunnel_follow_remote_waits_for_weak_password`, `tunnel_rows_mark_password_requirement`, `remote_edit_commit_rejects_default_and_blank_passwords`; ui `remote_modal_hides_fake_password_bullets`.

## Seed config

- `%APPDATA%\evrax\mcp.json` ditulis (tanpa key): `serena` + `codebase-memory-mcp`, keduanya `enabled: true`.

## Gate

- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace` → **308 test passed, 0 failed, 3 ignored** (agent 92, core 5, graph 5+1, remote 32, store 7, tui 166)
- `scripts/check-names.ps1` → clean
- `cargo build --release` → `target\release\evrax.exe`

Baseline sebelum batch: 279 passed / 3 ignored. Test tambahan: +29.

## Pembagian kerja

| Fix | Pengerjaan |
| --- | --- |
| FIX-005 | subagent (worker A) |
| FIX-004 | subagent (worker B), di-unblock + `welcome.rs` literal oleh orchestrator |
| FIX-007 | subagent (worker C) |
| FIX-001, FIX-006 (core+agent), FIX-002, FIX-003, FIX-006 (TUI), seed, gate, docs | orchestrator |

Riset pendahuluan (peta call-site `AgentEvent`/`ChatEntry`/render + inventaris test) dari 2 subagent explore.
