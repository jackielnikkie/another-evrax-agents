# Todofix v0.1.8 — worker unification + auto-orchestration + scope fix

Batch setelah `v0.1.7`. **Belum dieksekusi** — rencana dikunci, pengerjaan mulai besok.
Tujuan: satukan `subagent` → `worker`, worker otomatis saat tugas berat, dan perbaiki
scope/cleanup worker yang bocor antar proyek/sesi.

## Konteks bug (terkonfirmasi)
- `subagent::list()` (`crates/evrax-agent/src/subagent.rs:119`) membaca SEMUA direktori
  di `%APPDATA%\evrax\subagents\` tanpa filter → worker dari proyek/sesi lain ikut terbaca.
  Bukti: pane TUI menampilkan 2 worker, tapi agent (`subagent list`) melapor 6.
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

## P3 — scope + cleanup (fix bug 2 vs 6)
- `worker::list_for(root)` — filter `state.root == ctx.root` (per-proyek).
- Tool `worker list` (agent) hanya menampilkan worker proyek ini; tambah baris
  `(+K hidden)` bila ada worker proyek lain.
- Prune yatim: hapus state yang `!is_alive` (heartbeat mati) + status terminal
  (stopped/failed) — saat **startup** dan saat **stop**.

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

## Kontrak beku (untuk multi-worker)
```rust
evrax_agent::worker::{run, load, list_for, spawn_worker, State, WorkerStatus, events_path}
```

## Lane (sekali jalan)
- **K1** `crates/evrax-agent/src/worker.rs` + `crates/evrax-agent/src/lib.rs` — P1 core, P2, P3, **P7** (approval worker).
- **K2** `crates/evrax-agent/src/tools.rs` + `crates/evrax-agent/src/catalog.rs` — P1 tool rename, **P8** (`read` cache), **P9** (bukti verifikasi).
- **K3** `crates/evrax-tui/src/main.rs` + `app.rs` + `worker_link.rs` — P1 TUI, P4, P6 (standby non-blocking).
- **K4** docs — P5.

## Prioritas (paling murah → mahal)
1. **P7** approval `shell` worker (impact terbesar).
2. **P8** `read` cache collision.
3. **P9** injeksi bukti verifikasi (sebagian sembuh setelah P7).

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
