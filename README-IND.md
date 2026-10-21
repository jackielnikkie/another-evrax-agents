# evrax

Coding agent yang hidup di terminal. Ditulis dengan Rust + [ratatui](https://ratatui.rs):
satu binary kecil, hemat memori, backend LLM asli, plus web UI yang bisa dibuka dari HP.

![evrax TUI](assets/evrax1.png)

## Fitur

- **Agent loop dengan tools** — `read`, `list`, `grep`, `write`, `edit`, `shell`,
  `web_fetch`, `todo`, `git`, `ast_grep`, `evrax-graph`, `subagent`.
- **Mode Plan / Build** — Plan membuat semua tool jadi read-only.
- **Approval** — tool yang mengubah file dan path di luar workspace minta izin
  dulu (Once / Always / Reject).
- **Sesi tersimpan di disk** — tiap project punya store sendiri, jadi bisa
  keluar lalu lanjut lagi kapan saja.
- **UI terminal nyaman** — transcript per sesi, judul otomatis (heuristik + LLM),
  `/rename`, command palette, slash menu, paste gambar dengan preview Kitty,
  tujuh tema warna.
- **Web remote** — `/remote` menyalakan web UI kecil di port 444: login password,
  token bearer, streaming SSE, prompt dan approval dari browser, plus tunnel
  Cloudflare.
- **Code graph** — `evrax-graph` meng-index repo ke SQLite dan menjawab pencarian
  simbol, callers/callees, impact, dead code, dan outline file.
- **Retry dan guardrail** — retry otomatis untuk 429/5xx/error jaringan, deteksi
  loop, dan notifikasi jelas saat langkah satu turn habis.

![Pemilih tema](assets/evrax2.png)

*Pemilih tema: tujuh tema bawaan, `Enter` untuk simpan, `Esc` untuk batal.*

## Install (Windows)

```powershell
irm https://raw.githubusercontent.com/jackielnikkie/another-evrax-agents/main/install.ps1 | iex
```

Perintah ini menaruh `evrax.exe` di `%USERPROFILE%\.evrax\bin` dan menambahkannya
ke PATH user. Tidak perlu admin.

Opsi lain:

```powershell
.\install.ps1 -Version v0.1.2        # pakai rilis tertentu
.\install.ps1 -LocalZip .\evrax.zip  # install dari zip lokal
.\install.ps1 -Uninstall             # hapus binary dan entri PATH
```

Semua flag (`-InstallDir`, `-System`, `-NoPath`, `-Purge`, ...) ada di
[`docs/install.md`](docs/install.md).

## Pertama kali jalan

Jalankan `evrax`. Kalau API key belum ada, wizard setup akan terbuka:

1. Pilih preset (9router, OpenRouter, DeepSeek, atau Custom).
2. Isi base URL, model, dan API key.
3. Tekan **Test connection** untuk mengecek koneksi.
4. Tekan **Simpan** — backend baru langsung dipakai, tanpa restart.

Konfigurasi ditulis ke `%APPDATA%\evrax\agent.json`:

```json
{
  "base_url": "http://127.0.0.1:20128/v1",
  "model": "model-kamu",
  "api_key": "key-kamu",
  "max_iterations": 500
}
```

Bisa juga lewat environment variable (`EVRAX_BASE_URL`, `EVRAX_MODEL`,
`EVRAX_API_KEY`, `EVRAX_MAX_ITERATIONS`) atau flag CLI. Jalankan `evrax --setup`
kalau mau membuka wizard lagi.

## Pemakaian

```
evrax                        buka TUI dengan sesi baru
evrax -c                     lanjutkan sesi terakhir
evrax -s <id|prefix|judul>   lanjutkan sesi tertentu
evrax -l                     daftar sesi tersimpan
evrax --setup                buka wizard setup
evrax --mock                 backend mock bawaan (tanpa API)
evrax --model <id>           override model
evrax --base-url <url>       override endpoint
evrax --max-iterations <n>   tool round per turn (default 500)
evrax --root <path>          pakai workspace lain
evrax --no-save              jangan simpan sesi ke disk
evrax --help                 tampilkan semua flag
```

Keyboard:

| Tombol | Fungsi |
| --- | --- |
| `Enter` | kirim prompt |
| `Tab` | ganti Plan / Build |
| `Ctrl+P` | command palette |
| `Ctrl+N` | sesi baru |
| `Ctrl+O` | daftar sesi |
| `Ctrl+Y` | copy balasan terakhir |
| `Ctrl+C` | keluar |

Slash command: `/new`, `/sessions`, `/mode`, `/copy`, `/theme`, `/remote`,
`/rename <judul>`, `/exit`.

## Sesi

Sesi disimpan per project di `%LOCALAPPDATA%\evrax\projects\<project>-<hash>\`.
Transcript dan history agent disimpan setelah setiap turn, jadi `evrax -c`
melanjutkan tepat dari posisi terakhir. Pakai `evrax -l` untuk melihat id,
waktu, dan judul.

## Web remote

Ketik `/remote` di TUI untuk menyalakan web UI untuk sesi ini. Default-nya
listen di port 444 dan dilindungi password yang kamu set di dialog. Dialog yang
sama menampilkan URL LAN dan bisa menyalakan tunnel Cloudflare. Bind address,
port, dan public URL disimpan di `%APPDATA%\evrax\remote.json`.

## Build dari source

Toolchain di-pin ke GNU Windows di `rust-toolchain.toml`:

```sh
cargo build --release -p evrax-tui   # -> target\release\evrax.exe
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

## Struktur project

```
crates/evrax-core     kontrak backend (TurnRequest, AgentEvent, AgentBackend)
crates/evrax-agent    backend SSE, agent loop, tools, catalog
crates/evrax-remote   server web remote, SSE, tunnel
crates/evrax-store    penyimpanan project dan sesi
crates/evrax-tui      aplikasi TUI (binary: evrax)
evrax-graph/          indexer code graph dan CLI
skill-evrax/          skill yang dimuat catalog
docs/                 peta kode, install, remote/tunnel, tema
```

Peta modul dan alur kode: [`docs/code-map.md`](docs/code-map.md).

## Lisensi

MIT — lihat [LICENSE](LICENSE).
