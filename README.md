# evrax

A coding agent that lives in your terminal. Written in Rust with
[ratatui](https://ratatui.rs): one small binary, low memory, real LLM backend,
and a web UI you can open from your phone.

Indonesian: [README-IND.md](README-IND.md)

![evrax TUI](assets/evrax1.png)

## Features

- **Agent loop with tools** — `read`, `list`, `grep`, `write`, `edit`, `shell`,
  `web_fetch`, `web_search`, `todo`, `memory`, `git`, `ast_grep`, `evrax-graph`,
  `worker`, `skills_list`, `skill_view`.
- **Multi-provider** — save many endpoints (openai-compatible or native
  Anthropic), fetch their models automatically, and switch with `/model`.
- **Plan / Build modes** — Plan mode makes every tool read-only.
- **Approvals** — mutating tools and paths outside the workspace ask first
  (Once / Always / Reject).
- **Sessions on disk** — every project gets its own store, so you can quit and
  continue later.
- **Nice terminal UI** — per-session transcripts, automatic titles (heuristic
  plus LLM), `/rename`, command palette, slash menu, image paste with Kitty
  preview (large text pastes collapse to a `[pasted N chars]` chip), seven color
  themes.
- **Web remote** — `/remote` starts a small web UI on port 444: password login,
  bearer tokens, SSE streaming, prompts and approvals from the browser, plus a
  Cloudflare tunnel.
- **Code graph** — `evrax-graph` indexes the repo into SQLite and answers
  symbol search, callers/callees, impact, dead code, and file outlines.
- **Project memory** — the `memory` tool keeps durable facts and conventions in
  `.evrax/memory.json`, upserted by key across turns and restarts.
- **Retries and guardrails** — automatic retry for 429/5xx/network errors, loop
  detection, and a clear notice when a turn runs out of steps.
- **Parallel worker agents** — `/worker 2` spawns headless worker agents in
  split panes beside the main transcript, and the main agent acts as an
  orchestrator that splits the task across them.

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
.\install.ps1 -Version v0.1.3        # pin a release
.\install.ps1 -LocalZip .\evrax.zip  # install from a local zip
.\install.ps1 -Uninstall             # remove the binary and PATH entry
```

See [`docs/install.md`](docs/install.md) for every flag (`-InstallDir`,
`-System`, `-NoPath`, `-Purge`, ...).

## First run

Start `evrax`. If no API key is configured yet, a setup wizard opens. It is a
multi-provider setup: you can save as many providers as you like and switch
between them later with `/model`.

1. Pick a saved provider, or **add provider**.
2. Choose the protocol: **openai-compatible** (`/chat/completions`) or
   **anthropic** (`/messages`).
3. Give it a name (e.g. `Nimbus Go`, `Muse Spark`), then the base URL
   (`◀/▶` fills a preset: 9router, OpenRouter, DeepSeek, Custom).
4. Paste the API key and press `Enter` - evrax checks the key by fetching the
   model list (`GET /models`). No model name needed yet.
5. Pick a model from the list (type to filter). If the gateway has no `/models`,
   just type the model name and press `Enter`.
6. Review and save - the backend switches immediately, no restart.

Settings are written to `%APPDATA%\evrax\agent.json`:

```json
{
  "active": "Nimbus Go",
  "providers": [
    {
      "name": "Nimbus Go",
      "kind": "openai",
      "base_url": "https://api.example.com/v1",
      "api_key": "your-key",
      "model": "some-model"
    },
    {
      "name": "Muse Spark",
      "kind": "anthropic",
      "base_url": "https://api.example.com",
      "api_key": "your-key",
      "model": "claude-sonnet-4-5"
    }
  ],
  "max_iterations": 500
}
```

Old single-provider files are migrated automatically on the next save. You can
also use environment variables (`EVRAX_PROVIDER`, `EVRAX_BASE_URL`,
`EVRAX_MODEL`, `EVRAX_API_KEY`, `EVRAX_MAX_ITERATIONS`) or CLI flags - they
override the active provider for that run only. Run `evrax --setup` to open the
wizard again at any time, or `/model` inside the TUI to switch provider/model.

## Usage

```
evrax                        start the TUI with a new session
evrax -c                     continue the most recent session
evrax -s <id|prefix|title>   resume a specific session
evrax -l                     list saved sessions
evrax --setup                open the setup wizard
evrax --provider <name>      start with a saved provider
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

Slash commands: `/new`, `/sessions`, `/mode`, `/copy`, `/theme`, `/model`,
`/remote`, `/rename <title>`, `/worker <n>`, `/workers`, `/update`, `/exit`.
`/model` lists
saved providers, fetches their models on demand, and switches the active
provider/model without a restart. `/worker 2` spawns two worker agents in split
panes; the main agent splits the task. `/update` checks GitHub for a newer
release (evrax also checks once at startup).

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

| Path | Crate | What it is |
| --- | --- | --- |
| `crates/evrax-core` | `evrax-core` | backend contract — `TurnRequest`, `AgentEvent`, `AgentBackend` |
| `crates/evrax-agent` | `evrax-agent` | OpenAI-compatible + Anthropic backends, agent loop, tools, catalog |
| `crates/evrax-remote` | `evrax-remote` | web remote server, SSE stream, cloudflared tunnel |
| `crates/evrax-store` | `evrax-store` | project and session storage on disk |
| `crates/evrax-tui` | `evrax-tui` | the ratatui app — binary **`evrax`** |
| `evrax-graph/` | `evrax-graph` | code-graph indexer and CLI — binary **`evrax-graph`** |
| `skill-evrax/` | — | skills loaded by the catalog |
| `docs/` | — | code map, install, remote/tunnel, themes |

Module map and call flow: [`docs/code-map.md`](docs/code-map.md).

## License

MIT — see [LICENSE](LICENSE).
