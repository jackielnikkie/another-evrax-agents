# AGENTS.md — evrax

State/handoff dokumen untuk agent. Update manual tiap selesai batch kerja (jangan tunggu compacting).
Model disarankan: ringkas, bullet, simpan nilai teknis persis (path, hex, nomor baris).
Detail besar dipindah ke `docs/` (tidak di-inject tiap turn): lihat **Detail referensi**.

## Objective
- Bangun **"evrax"**: agent coding TUI di **Rust + ratatui** (`D:\another-evrax-agent`) — UI 1:1 ala opencode (RAM kecil, bukan Bun), otak backend ala `hermes-agent`, UI-first dengan MockBackend.
- **Pekerjaan berikutnya: batch skills** (bundle 211 skill + compile-time embed `build.rs` + tools `skills_list`/`skill_view`/`skill_manage` + `/skills` picker + scrub kata "opencode"/"hermes" dari file evrax) — plan final disetujui, eksekusi setelah smoke batch ini. `/remote` C1-C5, M1 backend, M2 perf+approval+web, M3 anti-bloat+auto-judul, **iterasi hermes+retry opencode+fix 403 SELESAI**.
- **Repo publik `evrax-ui`** (backup): `https://github.com/jackielnikkie/evrax-ui` — `main` @ `954d3da` (+ M3 & batch iterasi/retry belum di-commit), 4 crate (core/agent/remote/tui) + docs + AGENTS.md + LICENSE MIT + README. `target/` & `agent-reference/` di-ignore; AGENTS.md **ikut** (keputusan user).

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
- **M3 anti-bloat (FINAL)**: render = `ui::TranscriptCache` (flattened lines per-entry; prefix dibekukan, tail rebuild HANYA kalau `App.content_rev`/`tick` berubah; invalidasi width/theme/shrink/`invalidate()` saat ganti sesi). Paragraph tanpa `.wrap()` — semua line sudah pre-wrap (Diff kini wrap eksplisit + path truncate). Retensi: `MAX_ENTRIES_PER_SESSION=400` (trim >480), `MAX_RETAINED_SESSIONS=16` LRU (`remember_transcript`/`forget_session`), agent `MAX_HISTORY_SESSIONS=24` (`Inner.session_use`+`clock`).
- **Iterasi hermes (FINAL, user "samain aja kaya hermes")**: default `max_iterations` **500** (`lib.rs:43`, test `for_test`), counter hanya naik untuk round berisi tool call, budget dicek sebelum request. Habis → `Notice` + nudge hermes + request summary **tanpa tools** + retry 1× + append/commit + `TurnFinished` (incomplete, bukan `Failed`); fallback copy `Batas langkah turn ini habis (N tool call). Kirim `continue`...` via `TextDelta`. Turn abort → `commit_partial()` simpan prefix balanced (`balanced` = akhir round lengkap) supaya `continue` punya konteks. Subagent/`max_iterations` per-turn reset tiap prompt.
- **Retry opencode (FINAL)**: `retry.rs` — retry 429/5xx/408/transport/`x-should-retry:true`; TIDAK retry 400/401/402/403/404/422. Backoff 2s×2 cap 10s/gap, max 10 retry, jitter ≤250ms, `Retry-After`/`retry-after-ms` (cap 15mnt), timeout max 3×. Event `Retrying{attempt,max,delay_ms,reason}` → flash; 401/403/402 lewat `provider_error()` (hint API Keys 9router).
- **Guardrails (FINAL)**: `guardrails.rs` — streak hasil identik ≥3 notice; siklus 2–4 langkah ×3 lap notice; gagal identik 3× → panggilan ke-4 diblok; hasil identik ≥512 char → stub. State per turn.
- **9router 403 (FIXED 2026-10-20)**: pesan `This API key is not allowed to use model` dibuat 9router (row `apiKeys.accessRestricted=1` + `accessAllow="[]"`); fix `UPDATE apiKeys SET accessRestricted=0` (backup DB dulu). `usageHistory` hanya sukses & model tercatat hasil resolve (mis. `auto`) → dashboard kosong saat request gagal. Key tetap hanya di `%APPDATA%\evrax\agent.json` + DB 9router, jangan commit.
- **Auto judul sesi (FINAL, user pilih LLM)**: `session_id` **immutable** (`session-<ms>-<n>`; key transcript/history/remote — JANGAN rename id), display judul di `App.titles: HashMap<id,String>`. Placeholder heuristik `title_from_prompt()` seketika saat prompt pertama (non-slash); LLM via `AgentBackend::suggest_title(prompt,reply)->BoxFuture<Option<String>>` (default None; ChatBackend non-streaming `max_tokens:16 temp:0.2` timeout 10s + `sanitize_title` ≤8 kata/60 char) di-spawn main.rs dari `App::take_title_job()` (TurnFinished pertama, `title_pending` sekali per sesi) → `AgentEvent::TitleSuggested{session,title}`. `/rename <judul>` manual (`manual_titles` memblokir LLM susulan). Remote: `rename_session(server,id,name)` update nama saja (mode/queue utuh; `register` lama reset mode → pakai rename untuk rename). Web otomatis ikut (poll `/api/sessions` 2s).

## Detail referensi (tidak di-inject; baca saat perlu)
- Mapping 5 tema (hex lengkap catppuccin/dracula/gruvbox/nord/tokyonight): `docs/themes-notes.md`
- Peta path & nomor baris + keybinds + render order + recon hermes: `docs/code-map.md`
- Riwayat detail tiap batch kerja (verbatim): `docs/work-log.md`

## Work State
### Completed (detail penuh: `docs/work-log.md`)
- **M1 backend asli SELESAI & TERBUKTI LIVE** — crate baru `crates/evrax-agent`: SSE parser (`SseDecoder`, flush sisa frame saat stream close tanpa `[DONE]`), `ToolCallAccumulator` (fix CRITICAL: 9router kirim `"id":""` di frame lanjutan → dulu dianggap tool-call baru, nama & argumen pecah), agent loop max 500 iterasi (parity hermes), history per session (`Mutex<HashMap<session, Vec<turn>>`, trim 100k char), tools `read/list/grep/write/edit/shell` (path guard anti-escape, output cap 30KB, PowerShell + timeout + kill_on_drop, PATH di-inject `%USERPROFILE%\.cargo\bin`), diff via `similar`, approval utk mutating saja, plan mode = read-only (tools di-filter + deny tanpa prompt). Config: `%APPDATA%\evrax\agent.json` + env `EVRAX_*` + CLI `--mock --model --base-url --api-key --max-iterations --root`; default `http://127.0.0.1:20128/v1` + `bebas`; retry 3× utk 429/5xx.
- **API key 9router**: WAJIB (401 `Missing API key` kalau tanpa bearer) — value diambil dari `~/.config/opencode/opencode.json` provider `9router` (jangan ditulis ke file dalem repo) dan disimpan di `%APPDATA%\evrax\agent.json` (di luar repo, jangan pernah commit).
- **Core**: `TurnRequest { prompt, session, plan_mode }` — MockBackend abaikan field baru.
- **TUI wiring**: `main.rs` pilih backend real/mock + flash `backend · <model> · <url>`; `start_turn` kirim session+mode; transcript per session di App (`transcripts: HashMap<String, Vec<ChatEntry>>`) — ganti session replay tampilan, `/new` nama unik `session-<ms>-<counter>` (biar key history backend juga baru).
- Live smoke (ignored, jalan manual): `cargo test -p evrax-agent -- --ignored` → 2 passed (teks "OK" + read Cargo.toml beneran).
- **Checkpoint terakhir: 210 tests hijau (55 agent + 5 core + 29 remote + 121 tui), clippy 0.**
- **Batch M2 (perf + approval policy + Plan/Build web)**: lihat **Keputusan (FINAL)** 3 butir sebelum M3. Seed demo conversation di `App::new()` DIHAPUS (tampak mulai kosong; `demo_conversation()` & `ToolStatus::Pending` ikut dihapus — test lama pakai `ui::sample_conversation()` lokal). `App::backend_label` default `"agent"`, di-override `main.rs` dari model real / `"mock"`. Header tidak lagi nulis "U3 mock stream". Agent A & B digarap paralel subagent (crate terisolasi) — pattern yang berhasil.
- **Batch iterasi hermes + retry opencode + 403 (2026-10-20)**: detail `docs/work-log.md`. Akar stop 12 = default 12 + `Err` tanpa commit (amnesia) + round teks ikut dihitung. Kini: default 500 (agent.json user ikut 500), counter tool-round, summary toolless + fallback `continue`, `commit_partial` prefix balanced, guardrails loop, `retry.rs` (10 retry 429/5xx/transport, no-retry 4xx), `Notice/Retrying/Iteration` + footer `iterasi n/max`. 403 9router = `apiKeys.accessRestricted=1` (bukan missing key) → DB fix + verifikasi live 2 ignored tests.
- **Batch M3 (anti-bloat + auto judul)**: detail `docs/work-log.md`. Inti: `TranscriptCache` (tail-only rebuild via `content_rev`/`tick`, render slice viewport, tanpa `line_count()`), retensi 400/16/24, judul = placeholder heuristik instan + LLM susulan (`suggest_title`) + `/rename` manual sticky, `session_name`→`session_id`, `run_slash_line` (slash dieksekusi langsung dari composer, `/rename` ber-arg), remote `rename_session`. Seed `sessions` demo di `App::new()` ikut dihapus.
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
- **Smoke batch iterasi/retry (butuh `cargo run`)**: user Ctrl+C instance PID 15044 dulu; kirim prompt → footer `iterasi n/500`; `--max-iterations 3` → setelah 3 tool round keluar `Notice` + ringkasan + `TurnFinished` (bukan error), lalu `continue` → konteks lanjut; matikan 9router saat turn → flash `retry 1/10`, bukan langsung gagal.
- **M3 smoke** (masih relevan): judul heuristik instan → LLM ≤10 detik; `/rename`; chat panjang + banyak sesi → RAM stabil; remote ON → web judul + rename tidak reset mode.
- Belum diuji: edit file beneran + `cargo test` via shell tool (self-fix loop).
- **Belum diukur lagi**: idle CPU setelah perf fix (sebelum: 1 thread ~45-60% core, RAM 25 MB). `$p=Get-Process evrax; $a=$p.CPU; Start-Sleep 5; $b=(Get-Process evrax).CPU; ($b-$a)/5`; target <5%.
- Belum diuji live: toggle Plan/Build dari web; approval akses luar workspace (key per folder).

### Blocked
- Instance evrax user jalan (PID 15044) → `cargo build`/`cargo run` debug `Access is denied`; `cargo test`/`clippy`/`check` aman. User Ctrl+C dulu untuk build/smoke.

## Next Move
1. Smoke batch ini (lihat Active) + ukur CPU idle; perbaiki quirk stream/retry/judul yang muncul.
2. **Batch skills** (plan FINAL di objective): bundle 211 + rename 10 nama → scrub "opencode/hermes" (guard `git grep -i -E "opencode|hermes" -- . ':!skills'` = 0) → `TurnRequest.skills` → agent `build.rs` embed + `skills_list`/`skill_view`/`skill_manage` → TUI `/skills` + `/skill` + `/<nama>` → `docs/skills.md` + gate.
3. **T3** tailscale serve + item menu kondisional + rombak `docs/remote-tunnel.md`.
4. Ide M1.5: persist transcript+history ke disk antar-restart, token usage di status, `/model` picker.

## Catatan konfigurasi (opencode global)
- MCP `~/.config/opencode/opencode.json(c)`: **OFF** = `rea`, `context7`, `memory`, `fetch`, `puppeteer`, `sequential-thinking`; **ON** = `serena` + `codebase-memory-mcp` (CBM v0.12.1, terinstal via `install.ps1` default di `%LOCALAPPDATA%\Programs\codebase-memory-mcp`). Re-enable CBM: hapus/rename entry. CBM menulis plugin `cbm-augment.ts` + skill + 3 agent + blok di `AGENTS.md` global (+~750 token/turn, tapi arahkan ke graph tools biar hemat).
- CBM project **evrax** (mode `moderate`) ter-index (re-index terakhir: **823 node, 3940 edge**); `agent-reference/`, `docs/`, `target/` ter-exclude (`.gitignore` di repo). Detail lihat: `codebase-memory-mcp.exe cli list_projects`.
- Coexist dengan serena untuk perbandingan overhead. Putuskan: CBM swap dengan serena (hemat ~30 tool) atau rollback CBM.
