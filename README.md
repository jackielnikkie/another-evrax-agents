# evrax

A coding agent that lives in your terminal. Written in Rust with
[ratatui](https://ratatui.rs): one small binary, low memory, real LLM backend,
and a web UI you can open from your phone.

Indonesian: [README-IND.md](README-IND.md)

![evrax TUI](assets/evrax1.png)

## Features

- **Agent loop with tools** — `read`, `list`, `grep`, `write`, `edit`, `shell`,
  `web_fetch`, `todo`, `git`, `ast_grep`, `evrax-graph`, `subagent`.
- **Plan / Build modes** — Plan mode makes every tool read-only.
- **Approvals** — mutating tools and paths outside the workspace ask first
  (Once / Always / Reject).
- **Sessions on disk** — every project gets its own store, so you can quit and
  continue later.
- **Nice terminal UI** — per-session transcripts, automatic titles (heuristic
  plus LLM), `/rename`, command palette, slash menu, image paste with Kitty
  preview, seven color themes.
- **Web remote** — `/remote` starts a small web UI on port 444: password login,
  bearer tokens, SSE streaming, prompts and approvals from the browser, plus a
  Cloudflare tunnel.
- **Code graph** — `evrax-graph` indexes the repo into SQLite and answers
  symbol search, callers/callees, impact, dead code, and file outlines.
- **Retries and guardrails** — automatic retry for 429/5xx/network errors, loop
  detection, and a clear notice when a turn runs out of steps.

![Theme picker](assets/evrax2.png)

*Theme picker: seven built-in themes, `Enter` to save, `Esc` to revert.*

## Install (Windows)

```powershell
irm https://raw.githubusercontent.com/jackielnikkie/another-evrax-agents/main/install.ps1 | iex
```

This puts `evrax.exe` in `%USERPROFILE%\.evrax\bin` and adds it to your user
PATH. No admin rights needed.

Other options:

```powershell
.\install.ps1 -Version v0.1.2        # pin a release
.\install.ps1 -LocalZip .\evrax.zip  # install from a local zip
.\install.ps1 -Uninstall             # remove the binary and PATH entry
```

See [`docs/install.md`](docs/install.md) for every flag (`-InstallDir`,
`-System`, `-NoPath`, `-Purge`, ...).

## First run

Start `evrax`. If no API key is configured yet, a setup wizard opens:

1. Pick a preset (9router, OpenRouter, DeepSeek, or Custom).
2. Fill in base URL, model, and API key.
3. Press **Test connection** to check it.
4. Press **Save** — the new backend is used immediately, no restart.

Settings are written to `%APPDATA%\evrax\agent.json`:

```json
{
  "base_url": "http://127.0.0.1:20128/v1",
  "model": "your-model",
  "api_key": "your-key",
  "max_iterations": 500
}
```

You can also use environment variables (`EVRAX_BASE_URL`, `EVRAX_MODEL`,
`EVRAX_API_KEY`, `EVRAX_MAX_ITERATIONS`) or CLI flags. Run `evrax --setup` to
open the wizard again at any time.

## Usage

```
evrax                        start the TUI with a new session
evrax -c                     continue the most recent session
evrax -s <id|prefix|title>   resume a specific session
evrax -l                     list saved sessions
evrax --setup                open the setup wizard
evrax --mock                 built-in mock backend (no API needed)
evrax --model <id>           override the model
evrax --base-url <url>       override the endpoint
evrax --max-iterations <n>   tool rounds per turn (default 500)
evrax --root <path>          use another workspace
evrax --no-save              do not write sessions to disk
evrax --help                 show all flags
```

Keyboard:

| Key | Action |
| --- | --- |
| `Enter` | send prompt |
| `Tab` | switch Plan / Build |
| `Ctrl+P` | command palette |
| `Ctrl+N` | new session |
| `Ctrl+O` | session list |
| `Ctrl+Y` | copy last reply |
| `Ctrl+C` | quit |

Slash commands: `/new`, `/sessions`, `/mode`, `/copy`, `/theme`, `/remote`,
`/rename <title>`, `/exit`.

## Sessions

Sessions are stored per project under
`%LOCALAPPDATA%\evrax\projects\<project>-<hash>\`. Transcript and agent history
are saved after every turn, so `evrax -c` picks up exactly where you left off.
Use `evrax -l` to see ids, timestamps, and titles.

## Web remote

Type `/remote` inside the TUI to turn on the web UI for the current session.
It listens on port 444 by default and is protected by a password you set in the
dialog. The same dialog shows the LAN URL and can start a Cloudflare tunnel.
Bind address, port, and public URL live in `%APPDATA%\evrax\remote.json`.

## Build from source

The toolchain is pinned to GNU Windows in `rust-toolchain.toml`:

```sh
cargo build --release -p evrax-tui   # -> target\release\evrax.exe
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

## Project layout

```
crates/evrax-core     backend contract (TurnRequest, AgentEvent, AgentBackend)
crates/evrax-agent    SSE backend, agent loop, tools, catalog
crates/evrax-remote   web remote server, SSE, tunnel
crates/evrax-store    project and session storage
crates/evrax-tui      the TUI app (binary: evrax)
evrax-graph/          code graph indexer and CLI
skill-evrax/          skills loaded by the catalog
docs/                 code map, install, remote/tunnel, themes
```

Module map and call flow: [`docs/code-map.md`](docs/code-map.md).

## License

MIT — see [LICENSE](LICENSE).
