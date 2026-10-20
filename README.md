# evrax

Coding agent berbasis TUI — Rust + [ratatui](https://ratatui.rs). UI ala opencode (ringan, tanpa Bun), dengan remote web server bawaan.

> Status: UI + remote sudah selesai dan solid. Backend LLM asli (SSE/OpenAI-compatible + tools) sedang dikerjakan. Saat ini agent jalan dengan `MockBackend`.

## Fitur

- TUI multi-pane: chat stream (thinking / text / tool calls), composer dengan paste gambar + thumbnail, palette, slash menu.
- 5 tema (catppuccin, dracula, gruvbox, nord, tokyonight) + modal picker.
- Mode Build/Plan, dialog approval tool (Once / Always / Reject).
- `/remote` — web server (Vue 3, single HTML, tanpa npm) di port 444:
  - login password + token bearer, rate-limit, hardening
  - streaming SSE realtime, prompt dari web → TUI
  - approve/reject tool dari browser/HP
  - tunnel: Cloudflare quick tunnel, custom domain (named tunnel), manual URL; click-to-copy
- Session management ( cosmetically ), scroll PgUp/PgDn di dialog.

## Build & Run

Toolchain di-pin ke GNU Windows (`rust-toolchain.toml`):

```sh
cargo run            # evrax TUI (MockBackend)
cargo test           # test suite
cargo clippy         # lint
```

## Struktur

```
crates/
  evrax-core/    # AgentEvent, trait AgentBackend, MockBackend
  evrax-remote/  # /remote web server + SSE + tunnel (cloudflared)
  evrax-tui/     # aplikasi TUI (ratatui)
docs/            # referensi UI, peta kode, catatan tema & tunnel
```

## Konfigurasi remote

`%APPDATA%\evrax\remote.json`:

```json
{
  "password": "...",
  "bind": "127.0.0.1",
  "port": 444,
  "public_url": "",
  "tunnel_mode": "",
  "tunnel_hostname": ""
}
```

Toggle lewat dialog `/remote` di TUI (ON/OFF, password, tunnel). Bind `0.0.0.0` untuk akses LAN; lihat `docs/remote-tunnel.md`.

## Lisensi

MIT — lihat [LICENSE](LICENSE).
