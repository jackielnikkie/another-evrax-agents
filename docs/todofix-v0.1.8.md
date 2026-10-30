# Todofix v0.1.8 — worker unification + auto-orchestration + bugfix + prompt ala hermes

Batch setelah `v0.1.7`. **Belum dieksekusi** — rencana dikunci, pengerjaan mulai besok.
Tujuan: satukan `subagent` → `worker`, worker otomatis saat tugas berat, perbaiki
scope/cleanup worker (+ perintah `/workers`), approval shell (workspace auto / luar allow),
chip paste besar, format chat + todolist ala opencode, delegasi wajib ke worker, dan
identitas `SOUL.md` + prompt ala hermes (thinking/planning/delegating).

## Konteks bug (terkonfirmasi)
- `subagent::list()` (`crates/evrax-agent/src/subagent.rs:119`) membaca SEMUA direktori
  di `%APPDATA%\evrax\subagents\` tanpa filter → worker dari proyek/sesi lain ikut terbaca.
  Bukti: pane TUI menampilkan 2 worker, tapi agent (`subagent list`) melapor 6.
- **Bukti tambahan (2026-10-28)**: `worker list` lapor **18 worker** dari **5 sesi berbeda**
  menumpuk (hanya worker 1–5 milik sesi ini; sisanya yatim, termasuk 3 berstatus `stopped`
  yang direktorinya tetap tersisa). Konfirmasi P3 (tanpa filter root) + prune yatim.
  `stop` hanya menulis flag, direktori state tidak dihapus → `subagents\` menumpuk.
- **Blocker kerja**: `read`/`shell` gagal semua — `read` balikin isi salah ("clean, nothing
  to commit" untuk file `.rs` → P8), `shell` "rejected by user" (P7). Tanpa ini agent/worker
  tak bisa membaca file untuk mulai kerja → harus diperbaiki lebih dulu.
- Worker yatim tak pernah dibersihkan: `stop` hanya menulis file flag `stop`, direktori
  state tidak dihapus → `subagents\` menumpuk selamanya.
- `subagent` vs `worker` = satu sistem, dua nama. Disatukan jadi **`worker`**.

## Keputusan desain (FINAL, dari user)
1. **Worker = pane di TUI induk** (opsi i): proses background, log-nya dirender di pane
   TUI yang sama. BUKAN window/terminal sendiri.
2. **Pane tetap setelah selesai** → status `idle` (tidak auto-tutup), supaya log bisa dilihat.
3. **Otomatis (agent-driven)**: agent membuat worker sendiri saat tugas berat & belum ada
   worker; kalau sudah ada worker → otomatis dibagi tugas via `worker send`. Bukan mesin yang
   memaksa; LLM yang memutuskan lewat tool + prompt.
4. Tool name: **`worker`** (tunggal). Scope `list`: **per-proyek** (root sama).
5. Sekali jalan (3–4 subagent). Rilis **`v0.1.8`**.

## P1 — rename `subagent` → `worker` (satu sistem)
- `crates/evrax-agent/src/subagent.rs` → `worker.rs`; `lib.rs`: `pub mod subagent;` → `pub mod worker;`.
- `crates/evrax-agent/src/tools.rs`: spec `"subagent"` → `"worker"`; arm `risk`/`is_mutating`/
  `summary`/`run` → `"worker"`; `catalog.rs` nama tool.
- TUI: `subagent_link.rs` → `worker_link.rs`; `main.rs`/`app.rs` `evrax_agent::subagent::*` → `evrax_agent::worker::*`.
- **HAPUS `action=start`** (cara lama buka window `wt.exe`). Aksi final:
  `spawn | send | status | list | stop`. Semua worker = background + pane.

## P2 — auto-orchestration (prompt, agent-driven)
- `lib.rs::system_prompt`: perkuat blok Workers — agent PROAKTIF spawn worker untuk tugas
  berat/multi-bagian saat belum ada worker; kalau sudah ada worker, bagi subtask via
  `worker send`. Batas: jangan spawn untuk tugas kecil. Pane tetap (`idle`) setelah selesai.
- Nudge agar agent memakai `worker list`/`status` untuk cek worker proyek ini dulu.

## P3 — scope + cleanup (fix "18 worker dari 5 sesi")
- `worker::list_for(root)` — filter `state.root == ctx.root` (per-proyek); jangan tampilkan
  worker sesi/proyek lain.
- `worker list` (agent) hanya proyek ini; baris `(+K hidden)` bila ada worker proyek lain.
- **Prune yatim**: hapus state `!is_alive` (heartbeat mati) + status terminal (stopped/failed)
  saat **startup** dan saat **stop** (sekarang `stop` cuma tulis flag, direktori tak dihapus).
- **Perintah user (TUI) — WAJIB ada**:
  - `/workers` — **list** worker (jumlah + status + id, per-proyek).
  - `/workers clear` — hapus **semua** worker proyek ini.
  - `/workers kill <id>` — hapus satu worker.
  - (tool: `worker action=list` sama; `worker action=stop` menerima `all` untuk bersih-bersih.)
- File: `worker.rs` (prune + list_for), `app.rs`/`main.rs` (slash `/workers`), `tools.rs` (disp).

## P4 — TUI
- `/worker N` (sudah ada) + pane grid (sudah ada). Pastikan pane tetap idle setelah selesai.
- Opsional: `/workers` untuk kelola (list/stop).

## P6 — main agent standby saat worker kerja (keluhan user)
- Saat worker menjalankan tugas, main agent **tetap standby** (TIDAK ikut mengerjakan tugas
  yang sama / tidak diam terkunci). Setelah worker selesai, main agent melanjutkan:
  baca hasil worker, validasi, gabung, lanjut langkah berikutnya.
- Artinya: turn main agent tidak "menunggu mati" — ia bisa mengirim tugas ke worker,
  mengecek `worker status`, dan tetap responsif (menerima prompt user / interupsi) selama
  worker bekerja.
- Implikasi teknis: orkestrasi **non-blocking** — `worker send` tidak memblokir turn main;
  main agent poll `worker status`/`list` saat perlu, bukan busy-wait.

## P5 — docs
- Ganti `subagent` → `worker` di `README.md`, `README-IND.md`, `AGENTS.md`, `CHANGELOG.md`, `docs/`.

## P7 — approval `shell` di worker (BUG, prioritas 1)
- **Gejala**: worker headless menolak SEMUA command `shell` — bahkan `echo hi` di-reject.
  Ini bukan policy per-command; gate-nya menolak sebelum command dievaluasi.
- **Kandidat akar**: worker dijalankan tanpa handle approval ke TUI (pane tak punya jalur
  jawab approval) → default **deny**, atau approval timeout auto-deny. Cek apakah
  worker/pane memakai jalur approval yang SAMA dengan orchestrator (`authorize_and_run`)
  atau punya gate sendiri yang men-default-deny.
- **Repro**: spawn worker → minta `echo hi` → lihat layer mana yang memotong (gate TUI?
  headless run? policy?).
- **Arah fix**: worker headless harus punya policy approval otonom (mis. auto-allow di
  dalam workspace seperti agent utama; di luar workspace → reject). Jangan default-deny.

## P8 — `read` cache collision (BUG, prioritas 2)
- **Gejala**: `read` mengembalikan isi file yang SALAH (mis. habis `git status` — "clean,
  nothing to commit" — lalu `read` file lain mengembalikan entri lama).
- **Kandidat akar**: kunci cache `read` bukan `(path, window)` melainkan sesuatu yang
  shared — slot/kapasitas-ke-N, atau hash yang di-reset per turn → tabrakan antar file.
- **Deteksi**: log key + hash payload tiap `read`, ulangi skenario, lihat tabrakan.
- **Test regresi (wajib)**: `read a.rs` → `read b.rs` → `read a.rs` lagi harus balikin isi
  `a.rs`, bukan hasil pemanggilan sebelumnya.

## P9 — injeksi bukti verifikasi (akar "gak bisa bilang 76 passed")
- **Masalah**: tiga worker menulis kode tapi tak ada satu pun yang bisa menyatakan
  "76 passed" — hasil verifikasi tak kembali ke orchestrator sebagai bukti.
- **Arah**: mekanisme worker menyetor **bukti** (command + output) yang bisa dibaca main
  agent saat sintesis (mis. `last_reply` diperkaya, atau `worker status` menampilkan
  verified-command + ringkasan output). Setelah P7 beres ini sebagian sembuh sendiri.
- **Catatan**: P7 → P8 → P9 urutan prioritas (paling murah dulu).

## P10 — approval shell: workspace auto, luar workspace minta allow (keluhan user)
- Saat ini `tools.rs::requires_approval` → `name == "shell"` SELALU minta approval
  (`lib.rs:1260` "shell always needs approval (system-wide scope)").
- **Target**: shell **di dalam workspace** jalan otomatis (seperti write/edit); approval
  HANYA kalau command menyentuh path di luar workspace.
- Deteksi "path luar": parse path absolut di command vs `ctx.root`; kalau ada yang keluar
  root → approval (`key = "shell:<dir>"`). Selain itu auto.
- File: `tools.rs` (`requires_approval`/`outside_target` untuk shell) + `lib.rs` (authorize).

## P11 — format chat kurang jelas (ala opencode)
- `ui.rs::entry_lines` user bubble cuma `▌ ` + teks; assistant = markdown polos; tool row
  minim. Tak ada label/nama + jarak antar turn.
- **Target**: header label (`▌ you` / `✳ evrax`), separator/jarak antar turn, tool row lebih
  terbaca — mengikuti gaya opencode.
- File: `ui.rs`.

## P12 — todolist susah dipahami (ala opencode)
- `tools/todo.rs::render` → `[x] #1 (pending, medium) <text>` datar, ada id noise.
- **Target**: grup by status (`◐ In progress`, `◻ Pending`, `✔ Done`), glyph + nomor tanpa
  noise; lebih mudah dibaca.
- File: `tools/todo.rs` + render TUI bila perlu.

## P13 — delegasi wajib ke worker kalau ada (keluhan user)
- Kalau worker tersedia, agent **WAJIB** mendelegasikan; kerja sendiri hanya untuk tugas
  **ringan**.
- **Target**: nudge keras di `system_prompt` (`lib.rs`) + (opsional) guard.
- File: `lib.rs`.

## P14 — `evrax-graph status` belum-index jangan langsung ✗ merah
- Saat `.evrax/graph.db` belum ada, `status` mengembalikan `ok:false` → TUI render `✗` merah
  → user/agent menyangka error padahal cuma belum di-index.
- **Target (dua tingkat)**:
  1. **Belum di-index** = **info, bukan error**: pesan jelas
     `graph belum di-index · jalankan evrax-graph action=index`, **tanpa** `✗`/merah
     (mis. `ok:true` + status netral, atau glyph info `·`/`○`).
  2. `✗` merah **hanya** untuk kegagalan nyata (DB korup, IO error, gagal baca).
- Aturan umum: tool yang "belum siap" (belum index/konfigurasi) → **beri tahu dulu**, jangan
  langsung tanda gagal.
- File: `tools/graph.rs` (status handler) + mapping status→glyph di `ui.rs` bila perlu.

## P16 — chip paste besar (ala opencode) [PRIORITAS TINGGI]
- **Gejala**: paste > 10 baris cuma memunculkan **flash** `pasted 2621 chars`; tak ada chip
  visual `[pasted N chars]`, dan tak bisa dihapus sebagai satu unit.
- **Akar**: `app.rs::paste_text_input` push ke `pasted_text` + `set_flash` saja; tak ada
  render chip ( `ui.rs::render_attachment_chips` hanya menggambar `attachments` gambar), tak
  ada handling backspace.
- **Target**:
  1. Render chip `[pasted N chars]` di atas composer (untuk tiap `app.pasted_text`).
  2. Backspace saat composer kosong → hapus **chip terakhir** (bukan per-karakter).
  3. `composer_height` menambah baris chip paste.
  4. Isi tetap dikirim ke model saat submit (`merge_pasted_text` → `[pasted N chars]\n<body>`).
- File: `ui.rs` (render), `main.rs` (backspace), `app.rs` (`composer_height`, urutan hapus).
- Test: `large_paste_shows_a_chip`, `backspace_removes_paste_chip`, `paste_chip_content_is_sent`.

## P15 (opsional) — auto-index evrax-graph
- Kalau belum ada index & user minta status/search → jalankan index otomatis. Berat untuk
  repo besar. Usul: **jangan auto**, cukup pesan jelas (bagian dari P14).

## P17 — baca `SOUL.md` + `AGENTS.md` (identitas + konteks)
- `workspace.rs::context_file` diperluas:
  - **`SOUL.md`**: identitas/persona. Cari di **`%APPDATA%\evrax\SOUL.md`** (global user)
    dulu, lalu **workspace root**. (Tanpa `%APPDATA%`, fallback home.)
  - **`AGENTS.md`**: chain dari git-root turun ke cwd (`AGENTS.override.md` menang per dir),
    bukan cuma 1 file.
- **JANGAN baca `CLAUDE.md` / `.cursorrules`** — evrax tak memakai konvensi Claude (keputusan
  user). Hanya `SOUL.md` + `AGENTS.md`.
- Cap tiap file (~8 KB) + tanda `[... truncated ...]`.
- File: `workspace.rs` + `lib.rs` (inject ke prompt).
- Test: `soul_md_loaded_from_appdata_then_workspace`, `agents_md_chain_root_to_cwd`,
  `claude_and_cursorrules_are_ignored`.

## P18 — system prompt ala hermes (3 slot)
- Susun `system_prompt` (lib.rs) jadi blok berurutan seperti hermes:
  1. **stable**: `SOUL.md` (atau default identity) → coding brief (senior-engineer) →
     platform/env hints.
  2. **context**: workspace snapshot + `AGENTS.md` chain.
  3. **volatile**: skills index (ringkas) + memory + baris tanggal.
- Tujuan: luwes/persona-driven, mudah diarahkan lewat `SOUL.md`/`AGENTS.md` (bukan dinding
  larangan). Pertahankan aturan bahasa user + UI-English + plan-mode caveat.
- File: `lib.rs` (`system_prompt`), `workspace.rs`.

## P19 — thinking + planning + delegating
- **Thinking**: pastikan `ThinkingDelta`/`ThinkingFinished` dipakai; prompt mengizinkan
  berpikir dulu sebelum bertindak (reasoning step eksplisit).
- **Planning (grok sebelum bertindak)**: guidance "pahami dulu keseluruhan sebelum mengubah"
  (gather context → rencana → eksekusi).
- **Delegating**: worker (lihat P13) — bagi tugas lewat `worker send`, standby (P6).
- File: `lib.rs` (brief).

## Kontrak beku (untuk multi-worker)
```rust
evrax_agent::worker::{run, load, list_for, spawn_worker, State, WorkerStatus, events_path}
```

## Lane (sekali jalan)
- **K1** `crates/evrax-agent/src/worker.rs` + `crates/evrax-agent/src/workspace.rs` + `lib.rs` — P1 core, P2, P3 (list_for+prune), P7 (approval worker), P10 (approval shell), P13 (delegasi), P17 (SOUL+AGENTS), P18 (prompt hermes), P19 (thinking/planning).
- **K2** `crates/evrax-agent/src/tools.rs` + `catalog.rs` + `tools/todo.rs` + `tools/graph.rs` — P1 tool rename, P8 (`read` cache), P9 (bukti), P10 (requires_approval), P12 (todo render), P14 (graph status).
- **K3** `crates/evrax-tui/src/main.rs` + `app.rs` + `worker_link.rs` + `ui.rs` — P1 TUI, P3 (`/workers` list/clear/kill), P4, P6 (standby), P11 (chat format), P16 (chip paste).
- **K4** docs — P5.

## Prioritas (paling murah → mahal)
1. **P7** approval `shell` worker (impact terbesar).
2. **P16** chip paste besar + backspace (kerja sering gagal karena prompt panjang tak terlihat).
3. **P8** `read` cache collision.
4. **P10** shell workspace auto / luar approval.
5. **P3** worker numpuk: scope + prune + `/workers` list/clear/kill.
6. **P13** delegasi wajib ke worker.
7. **P9** bukti verifikasi (sebagian sembuh setelah P7).
8. **P14** graph status netral (info dulu, merah hanya kalau gagal).
9. **P11** format chat · **P12** todolist.
10. **P17/P18/P19** SOUL.md + prompt hermes + thinking/planning/delegating.
11. P1 rename `subagent`→`worker`, P2/P4/P6, P5 docs.

## Gate (serial, orkestrator)
`cargo fmt --all` · `cargo clippy --workspace --all-targets -- -D warnings` ·
`cargo test --workspace` · `powershell scripts/check-names.ps1` — semua harus hijau.

## Rilis
Bump `Cargo.toml` → `0.1.8` (+ `cargo check --workspace` refresh lock) · commit
(feat / docs / chore) · push `another` main · tag `v0.1.8` HANYA ke `another` ·
verifikasi asset + SHA256SUMS via `github.com` web redirect (api.github.com rate-limited).

## Catatan
- Build debug pertama setelah `cargo clean` (bersih-bersih 15 GB) lambat karena rekompilasi
  dependency dari nol; selanjutnya incremental/cepat.
