# AGENTS.md — evrax

State/handoff dokumen untuk agent. Update manual tiap selesai batch kerja (jangan tunggu compacting).
Model disarankan: ringkas, bullet, simpan nilai teknis persis (path, hex, nomor baris).
Detail besar dipindah ke `docs/` (tidak di-inject tiap turn): lihat **Detail referensi**.

## Objective
- Bangun **"evrax"**: agent coding TUI di **Rust + ratatui** (`D:\another-evrax-agent`) — UI 1:1 ala opencode (RAM kecil, bukan Bun), otak backend ala `hermes-agent`, UI-first dengan MockBackend.
- **Pekerjaan aktif: M1 backend asli (9router SSE)** — plan disetujui user "gas semua sekaligus". `/remote` C1-C5 + polish UI + LAN fix SELESAI; hapus foto (C6) SELESAI.
- **Repo publik `evrax-ui`** (backup): `https://github.com/jackielnikkie/evrax-ui` — `main` @ `e812587`, 4 crate (core/agent/remote/tui) + docs + AGENTS.md + LICENSE MIT + README. `target/` & `agent-reference/` di-ignore; AGENTS.md **ikut** (keputusan user).

## Aturan kerja (user)
- Response ke user pakai **Bahasa Indonesia**.
- **Build mode aktif** (disetujui user); user menulis "woi"/"oi"/"hai" = lanjutkan saja, jangan berhenti/tanya.
- Toolchain **GNU** (`stable-x86_64-pc-windows-gnu`, pinned `rust-toolchain.toml`); prefix semua cargo:
  `$env:Path = [System.Environment]::GetEnvironmentVariable("Path","User") + ";" + [System.Environment]::GetEnvironmentVariable("Path","Machine");`
- **Truncation**: hasil Read tool DAN bash `Get-Content` terpotong ~2.4KB/hasil. Baca file/JSON panjang pakai window kecil: `Get-Content <path> | Select-Object -Skip N -First M` berulang; fallback `Select-String -Pattern '"md_|"syn_' -Context 0,2`.
- **Blocked**: `cargo build`/`cargo run` gagal `Access is denied. (os error 5)` selama instance evrax jalan → user Ctrl+C dulu; `cargo test` aman.
- **MOJIBAKE GUARD**: `Set-Content`/`Out-File` PS 5.1 menambah BOM + bisa double-encode UTF-8 → JANGAN pakai untuk rewrite file source. Pakai edit tool, atau `[System.IO.File]::WriteAllText($p, $t, (New-Object System.Text.UTF8Encoding($false)))`. Cek BOM: byte `0xEF 0xBB 0xBF` di awal file.
- **Catatan**: working tree bisa berisi edit setengah jalan dari instance evrax yang sedang dipakai user (kejadian nyata: `plan_mode` di evrax-remote bikin workspace gagal kompile). Cek `git status` + `cargo check --workspace --all-targets` SEBELUM mulai; `git checkout -- <file>` kalau memang WIP salah.
- `.git` sudah ada (repo `evrax-ui`, remote `origin/main`) — Serena/CBM project auto-aktif jalan.

## Keputusan (FINAL)
- **Scope alias**: hapus alias `/themes` (`app.rs:111` → `aliases: &[]`); `docs/opencode-ui-reference.md:129` sudah diamend. String "themes" di ui.rs & test `theme_modal_lists_themes` bukan alias slash — tidak diubah.
- **Gruvbox diff fg (CONFIRMED)**: `diffAdded=#98971a`, `diffRemoved=#cc241d`.
- **`/remote` (FINAL)**: server pusat auto-spawn oleh instance evrax pertama yang ON; port **444**; `tiny_http`; web UI **Vue 3 CDN single HTML** (`include_str!`, tanpa npm/Vite); login password saja; ON/OFF per-session + permit `read`/`read-write`; tunneling external + bind configurable + docs.
- **Fix OFF→ON (FINAL)**: **Opsi A** — server hidup selama app pegang handle `RemoteServer`; OFF = unregister saja; mati saat app quit / handle di-drop. Bind gagal (port 444 dipakai) → **auto-revert `enabled=false` + flash error**.
- **Approval policy ala opencode (FINAL)**: file tool (`read/list/grep/write/edit`) path **dalam workspace = otomatis** (write/edit TANPA prompt). Path **luar workspace = wajib approval**, `key = "<tool>:<dir induk kanonik>"` (mis. `read:C:/Users/nzeex/Documents`) → "Always allow" cuma buka folder itu. `shell` **selalu** approval (`key = "shell"`). Plan mode: mutating → `blocked: plan mode is read-only`, akses luar → `blocked: plan mode is workspace read-only` (tanpa prompt). Implementasi: `tools.rs` `Context::classify()` (tidak nolak escape lagi, cuma klasifikasi `Resolved{path,outside}`), `tools::outside_target()`, `tools::approval_key()`, `lib.rs::authorize_and_run()`. `AgentEvent::ApprovalRequested` punya field `key: String`; `App.auto_allow` sekarang isi `key`, bukan nama tool.
- **Plan/Build web (FINAL)**: mode = **state global per sesi, sinkron dua arah** (bukan per-prompt). Web hanya *request* (`POST /api/session/{id}/mode` → antre `pending_modes`), TUI yang apply + konfirmasi `set_session_mode()` → broadcast `RemoteEvent::Mode` ke semua tab. `App::sync_remote_mode()` dipanggil tiap tick (4 Hz, idempotent). `take_pending_prompt` TETAP `Option<String>` (jangan pernah pakai `plan_mode` per-prompt).
- **Perf TUI (FINAL)**: redraw **event-driven + dirty flag**, bukan 4 Hz buta. `ui::messages()` nyimpen `App.frame_max_scroll` (satu kali build transcript per frame; `ui::max_scroll_for` DIHAPUS). Tick cuma repaint kalau `busy`/`flash`/`approval`/kitty/resize/maximize berubah. `MouseEventKind::Moved` diabaikan. Burst `event_rx` di-drain (`try_recv`) → 1 repaint per burst. Label backend dinamis (`App.backend_label`), bukan "mock · max" hardcoded.

## Detail referensi (tidak di-inject; baca saat perlu)
- Mapping 5 tema (hex lengkap catppuccin/dracula/gruvbox/nord/tokyonight): `docs/themes-notes.md`
- Peta path & nomor baris + keybinds + render order + recon hermes: `docs/code-map.md`
- Riwayat detail tiap batch kerja (verbatim): `docs/work-log.md`

## Work State
### Completed (detail penuh: `docs/work-log.md`)
- **M1 backend asli SELESAI & TERBUKTI LIVE** — crate baru `crates/evrax-agent`: SSE parser (`SseDecoder`, flush sisa frame saat stream close tanpa `[DONE]`), `ToolCallAccumulator` (fix CRITICAL: 9router kirim `"id":""` di frame lanjutan → dulu dianggap tool-call baru, nama & argumen pecah), agent loop max 12 iterasi, history per session (`Mutex<HashMap<session, Vec<turn>>`, trim 100k char), tools `read/list/grep/write/edit/shell` (path guard anti-escape, output cap 30KB, PowerShell + timeout + kill_on_drop, PATH di-inject `%USERPROFILE%\.cargo\bin`), diff via `similar`, approval utk mutating saja, plan mode = read-only (tools di-filter + deny tanpa prompt). Config: `%APPDATA%\evrax\agent.json` + env `EVRAX_*` + CLI `--mock --model --base-url --api-key --max-iterations --root`; default `http://127.0.0.1:20128/v1` + `bebas`; retry 3× utk 429/5xx.
- **API key 9router**: WAJIB (401 `Missing API key` kalau tanpa bearer) — value diambil dari `~/.config/opencode/opencode.json` provider `9router` (jangan ditulis ke file dalem repo) dan disimpan di `%APPDATA%\evrax\agent.json` (di luar repo, jangan pernah commit).
- **Core**: `TurnRequest { prompt, session, plan_mode }` — MockBackend abaikan field baru.
- **TUI wiring**: `main.rs` pilih backend real/mock + flash `backend · <model> · <url>`; `start_turn` kirim session+mode; transcript per session di App (`transcripts: HashMap<String, Vec<ChatEntry>>`) — ganti session replay tampilan, `/new` nama unik `session-<ms>-<counter>` (biar key history backend juga baru).
- Live smoke (ignored, jalan manual): `cargo test -p evrax-agent -- --ignored` → 2 passed (teks "OK" + read Cargo.toml beneran).
- **Checkpoint terakhir: 179 tests hijau (38 agent + 5 core + 28 remote + 108 tui), clippy 0, build OK.**
- **Batch M2 (perf + approval policy + Plan/Build web)**: lihat **Keputusan (FINAL)** 3 butir terakhir. Seed demo conversation di `App::new()` DIHAPUS (tampak mulai kosong; `demo_conversation()` & `ToolStatus::Pending` ikut dihapus — test lama pakai `ui::sample_conversation()` lokal). `App::backend_label` default `"agent"`, di-override `main.rs` dari model real / `"mock"`. Header tidak lagi nulis "U3 mock stream". Agent A & B digarap paralel subagent (crate terisolasi) — pattern yang berhasil.
- **LAN HP**: `remote.json` bind `0.0.0.0` + firewall rule `evrax remote 444` (inbound TCP 444, remoteip 192.168.1.0/24) → listener `0.0.0.0:444`, `/health` via `192.168.1.9` OK.
- **UI dialog `/remote`**: baris info (bind/port/url/session = row 3/4/5/7) read-only, cursor skip, diblok-center di bawah tunnel; baris `url` tampilin IP LAN asli (`remote_display_url()` + `local_ipv4()`).
- **Approval web**: `RemoteEvent::Approval`/`ApprovalResolved` (SSE) + `POST /api/session/{id}/approval` (once/always/reject), kartu 3 tombol di web; **bind/port hot-reload** (`/remote` OFF→ON tanpa restart app).
- **T2 custom domain**: `tunnel.rs` `start_domain()` — cloudflared login→create→route dns (idempotent) → `evrax.malvera.biz.id` verified 200 via curl.
- **T1 tunnel**: `TunnelManager` quick-tunnel + `generation` counter (fix leak 9 cloudflared orphan) + click-to-copy URL.
- **Fix dialog `/remote`**: `remote_move` wrap row_count nyata; edit password mulai buffer kosong.
- **C5 hardening**: `RemoteConfig{password,bind,port,public_url}` (`%APPDATA%\evrax\remote.json`), bearer token wajib (kecuali /health,/login,GET /), expiry 24h, rate-limit /login 5/5mnt, constant-time compare; web login splash + Android responsive.
- **Scroll dialog** (palette/slash/session/theme): `ui::dialog_window()` jendela ikut selection + PgUp/PgDn.
- **Prompt web→TUI**: antre `pending_prompts`, drain di ticker saat `!busy`; echo multi-tab; web renderer per-turn block; restyle palette EVRAX.
- **Fix SSE**: `Request::into_writer()` + preamble manual + flush per frame + heartbeat 15s (BufWriter tiny_http nahan frame 30s).
- **C3**: forwarding AgentEvent→SSE, end-to-end terbukti.
- **Fix `/remote` OFF→ON**: bind sinkron di `start()`, `AliveGuard`, bind gagal → revert `enabled=false` + flash.
- 5 tema + swatch; paste/composer wrap; image attachment + preview kitty.

### Active
- Verifikasi manual binary baru (`cargo run`): flash `backend · bebas · http://127.0.0.1:20128/v1`; minta "baca Cargo.toml" → tool `read` beneran; `/new` & ganti session → transcript replay + history backend ikut; approval write/edit/shell (TUI + web 3 tombol); Plan mode nolak mutating.
- Belum diuji: edit file beneran + `cargo test` via shell tool (self-fix loop) — coba di sesi live berikutnya.
- **Belum diukur lagi**: idle CPU setelah perf fix (sebelum fix: 1 thread ~45-60% core, RAM 25 MB). Cara ukur: `$p=Get-Process evrax; $a=$p.CPU; Start-Sleep 5; $b=(Get-Process evrax).CPU; ($b-$a)/5`. Target <5%.
- Belum diuji live: toggle Plan/Build dari web → TUI ikut berubah & semua tab dapat SSE `mode`; approval akses luar workspace (key per folder).

### Blocked
- (none — instance mati, port 444 bebas; `cargo build` aman. Kalau instance jalan lagi: build/run kena `Access is denied`, `cargo test`/`clippy` tetap aman.)

## Next Move
1. Smoke live M1 di TUI (lihat Active) — perbaiki quirk stream/model yang muncul.
2. **T3** tailscale serve + item menu kondisional + rombak `docs/remote-tunnel.md`.
3. Ide M1.5: persist transcript+history ke disk antar-restart, token usage di status, `/model` picker.

## Catatan konfigurasi (opencode global)
- MCP `~/.config/opencode/opencode.json(c)`: **OFF** = `rea`, `context7`, `memory`, `fetch`, `puppeteer`, `sequential-thinking`; **ON** = `serena` + `codebase-memory-mcp` (CBM v0.12.1, terinstal via `install.ps1` default di `%LOCALAPPDATA%\Programs\codebase-memory-mcp`). Re-enable CBM: hapus/rename entry. CBM menulis plugin `cbm-augment.ts` + skill + 3 agent + blok di `AGENTS.md` global (+~750 token/turn, tapi arahkan ke graph tools biar hemat).
- CBM project **evrax** (mode `moderate`) ter-index (re-index terakhir: **823 node, 3940 edge**); `agent-reference/`, `docs/`, `target/` ter-exclude (`.gitignore` di repo). Detail lihat: `codebase-memory-mcp.exe cli list_projects`.
- Coexist dengan serena untuk perbandingan overhead. Putuskan: CBM swap dengan serena (hemat ~30 tool) atau rollback CBM.
