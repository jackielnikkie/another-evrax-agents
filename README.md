# evrax

Coding agent berbasis TUI — Rust + [ratatui](https://ratatui.rs). Ringan (RAM belasan MB, satu binary), backend LLM asli via endpoint OpenAI-compatible, plus web remote yang bisa diakses dari HP.

## Fitur

- **Agent loop** dengan tool call: `read` `list` `grep` `write` `edit` `shell` `web_fetch` `todo` `git` `ast_grep` `evrax-graph` `subagent`.
- **Mode Plan/Build** — Plan memaksa semua tool jadi read-only.
- **Approval** — tool mutating & akses di luar workspace minta konfirmasi (Once / Always / Reject).
- **Transcript per sesi**, judul otomatis (heuristik + LLM), `/rename`, palette, slash menu, paste gambar + preview Kitty.
- **`/remote`** — server web (Vue 3, satu file HTML, tanpa npm) di port 444: login password, token bearer, streaming SSE, prompt/approval/ganti mode dari browser, tunnel Cloudflare (quick / custom domain / manual).
- **`evrax-graph`** — index kode lokal (SQLite di `.evrax/graph.db`): search simbol, callers/callees, impact, dead code, outline file.
- **7 tema**: `evrax`, `spectrum`, catppuccin, dracula, gruvbox, nord, tokyonight.
- **Retry + guardrail** — retry otomatis untuk 429/5xx/masalah jaringan, deteksi loop tool-call, notifikasi saat batas langkah turn habis.

## Install (Windows)

```powershell
irm https://raw.githubusercontent.com/jackielnikkie/another-evrax-agents/main/install.ps1 | iex
```

Binary dipasang ke `%USERPROFILE%\.evrax\bin` dan ditambahkan ke PATH user (tanpa admin). Detail flag (`-Version`, `-InstallDir`, `-System`, `-LocalZip`, `-NoPath`, `-Uninstall`, `-Purge`) ada di [`docs/install.md`](docs/install.md).

### Konfigurasi

Buat `%APPDATA%\evrax\agent.json`:

```json
{
  "base_url": "http://127.0.0.1:20128/v1",
  "model": "nama-model",
  "api_key": "key-kamu",
  "max_iterations": 500
}
```

Bisa juga lewat env (`EVRAX_BASE_URL`, `EVRAX_MODEL`, `EVRAX_API_KEY`, `EVRAX_MAX_ITERATIONS`) atau flag CLI. Key tidak pernah dibaca dari variabel ke repo.

## Pakai

```
evrax                       buka TUI, sesi baru
evrax --mock                backend mock (tanpa API)
evrax --model <id>          override model
evrax --base-url <url>      override endpoint
evrax --max-iterations <n>  batas tool round per turn
evrax --root <path>         buka workspace lain
evrax --help                semua flag
```

Di dalam TUI: `Ctrl+P` palette, `Ctrl+N` sesi baru, `Ctrl+O` daftar sesi, `Ctrl+Y` copy transcript, `Tab` ganti Plan/Build, `Ctrl+C` keluar.

## Build dari source

Toolchain di-pin ke GNU Windows (`rust-toolchain.toml`):

```sh
cargo build --release -p evrax-tui   # hasil: target\release\evrax.exe
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

## Struktur

```
crates/evrax-core     kontrak backend (TurnRequest, AgentEvent, AgentBackend)
crates/evrax-agent    backend SSE + agent loop + tools + catalog
crates/evrax-remote   server web remote + SSE + tunnel
crates/evrax-tui      aplikasi TUI (biner: evrax)
evrax-graph/          crates indexer graph kode + CLI
skill-evrax/          skill yang dimuat catalog
docs/                 peta kode, install, remote/tunnel, tema
```

Peta modul & alur kode: [`docs/code-map.md`](docs/code-map.md).

## Lisensi

MIT — lihat [LICENSE](LICENSE).