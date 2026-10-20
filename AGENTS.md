# AGENTS.md — evrax

State/handoff dokumen untuk agent. Update manual tiap selesai batch kerja (jangan tunggu compacting).
Model disarankan: ringkas, bullet, simpan nilai teknis persis (path, hex, nomor baris).
Detail besar dipindah ke `docs/` (tidak di-inject tiap turn): lihat **Detail referensi**.

## Objective
- Bangun **"evrax"**: agent coding TUI di **Rust + ratatui** (`D:\another-evrax-agent`) — UI 1:1 ala opencode (RAM kecil, bukan Bun), otak backend ala `hermes-agent`, UI-first dengan MockBackend.
- **Pekerjaan aktif: M1 backend asli (9router SSE)** — plan disetujui user "gas semua sekaligus". `/remote` C1-C5 + polish UI + LAN fix SELESAI; hapus foto (C6) SELESAI.
- **Repo publik `evrax-ui`** (backup): `https://github.com/jackielnikkie/evrax-ui` — `main` @ `eeaf22e`, 25 file (3 crate + docs + AGENTS.md + LICENSE MIT + README). `target/` & `agent-reference/` di-ignore; AGENTS.md **ikut** (keputusan user).

## Aturan kerja (user)
- Response ke user pakai **Bahasa Indonesia**.
- **Build mode aktif** (disetujui user); user menulis "woi"/"oi"/"hai" = lanjutkan saja, jangan berhenti/tanya.
- Toolchain **GNU** (`stable-x86_64-pc-windows-gnu`, pinned `rust-toolchain.toml`); prefix semua cargo:
  `$env:Path = [System.Environment]::GetEnvironmentVariable("Path","User") + ";" + [System.Environment]::GetEnvironmentVariable("Path","Machine");`
- **Truncation**: hasil Read tool DAN bash `Get-Content` terpotong ~2.4KB/hasil. Baca file/JSON panjang pakai window kecil: `Get-Content <path> | Select-Object -Skip N -First M` berulang; fallback `Select-String -Pattern '"md_|"syn_' -Context 0,2`.
- **Blocked**: `cargo build`/`cargo run` gagal `Access is denied. (os error 5)` selama instance evrax jalan → user Ctrl+C dulu; `cargo test` aman.
- `.git` sudah ada (repo `evrax-ui`, remote `origin/main`) — Serena/CBM project auto-aktif jalan.

## Keputusan (FINAL)
- **Scope alias**: hapus alias `/themes` (`app.rs:111` → `aliases: &[]`); `docs/opencode-ui-reference.md:129` sudah diamend. String "themes" di ui.rs & test `theme_modal_lists_themes` bukan alias slash — tidak diubah.
- **Gruvbox diff fg (CONFIRMED)**: `diffAdded=#98971a`, `diffRemoved=#cc241d`.
- **`/remote` (FINAL)**: server pusat auto-spawn oleh instance evrax pertama yang ON; port **444**; `tiny_http`; web UI **Vue 3 CDN single HTML** (`include_str!`, tanpa npm/Vite); login password saja; ON/OFF per-session + permit `read`/`read-write`; tunneling external + bind configurable + docs.
- **Fix OFF→ON (FINAL)**: **Opsi A** — server hidup selama app pegang handle `RemoteServer`; OFF = unregister saja; mati saat app quit / handle di-drop. Bind gagal (port 444 dipakai) → **auto-revert `enabled=false` + flash error**.

## Detail referensi (tidak di-inject; baca saat perlu)
- Mapping 5 tema (hex lengkap catppuccin/dracula/gruvbox/nord/tokyonight): `docs/themes-notes.md`
- Peta path & nomor baris + keybinds + render order + recon hermes: `docs/code-map.md`
- Riwayat detail tiap batch kerja (verbatim): `docs/work-log.md`

## Work State
> **Aturan log (berlaku semua agent — opencode, Hermes, dll):** riwayat detail tiap batch kerja DIAPPEND ke `docs/work-log.md` (verbatim, satu bullet per batch). `AGENTS.md` ini cuma nyimpen ringkasan + state hidup — jangan gedein lagi (budget ~7KB; full text dipindah ke work-log 2026-10-20).
### Completed (detail penuh: `docs/work-log.md`)
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
- **Checkpoint terakhir: 133 tests hijau (5 core + 23 remote + 105 tui), clippy 0, build 2026-10-20 14:07.**

### Active
- **M1 backend asli (9router SSE)** — plan disetujui, "gas semua sekaligus". Kerja berjalan: `Cargo.toml`/`evrax-core/src/lib.rs`/`Cargo.lock` modified (belum commit).
- Verifikasi manual binary 14:07: info-row `/remote` center+muted+skip cursor & url IP LAN; approval web 3 tombol; LAN HP `http://192.168.1.9:444`; tunnel click-copy; guard password default; OFF→tunnel stop.

### Blocked
- (none — instance mati, port 444 bebas; `cargo build` aman. Kalau instance jalan lagi: build/run kena `Access is denied`, `cargo test`/`clippy` tetap aman.)

## Next Move
1. Lanjut **M1** (9router SSE) — deps/vendor: `docs/code-map.md`.
2. **T3** tailscale serve + item menu kondisional + rombak `docs/remote-tunnel.md`.

## Catatan konfigurasi (opencode global)
- MCP `~/.config/opencode/opencode.json(c)`: **OFF** = `rea`, `context7`, `memory`, `fetch`, `puppeteer`, `sequential-thinking`; **ON** = `serena` + `codebase-memory-mcp` (CBM v0.12.1, terinstal via `install.ps1` default di `%LOCALAPPDATA%\Programs\codebase-memory-mcp`). Re-enable CBM: hapus/rename entry. CBM menulis plugin `cbm-augment.ts` + skill + 3 agent + blok di `AGENTS.md` global (+~750 token/turn, tapi arahkan ke graph tools biar hemat).
- CBM project **evrax** (mode `moderate`) ter-index (re-index terakhir: **823 node, 3940 edge**); `agent-reference/`, `docs/`, `target/` ter-exclude (`.gitignore` di repo). Detail lihat: `codebase-memory-mcp.exe cli list_projects`.
- Coexist dengan serena untuk perbandingan overhead. Putuskan: CBM swap dengan serena (hemat ~30 tool) atau rollback CBM.
