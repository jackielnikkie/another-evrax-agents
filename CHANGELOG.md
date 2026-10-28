# Changelog

All notable changes to evrax are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions use
[semantic versioning](https://semver.org/).

## [Unreleased]

### Added

- `ToolPreparing` progress event with throttled updates (4 KB / 200 ms) while
  tool arguments stream, plus elapsed time, iteration budget, and queued prompt
  count in the busy footer.
- Prompt queue while a turn is streaming (`VecDeque`, max 8). TUI and web
  prompts share the queue; read-only slash commands still run immediately.
- 42 skills embedded in the binary, so fresh installs have skills without a
  repository checkout.
- Web remote replays the session transcript on connect (capped at 512 KiB /
  2000 events), so a fresh tab or a reload rebuilds the conversation instead of
  showing an empty chat.
- Web remote reports queue depth (`queued N`) when a prompt is accepted while
  the TUI is busy, and refuses new prompts at the cap with `429`.
- Independent login tokens: each device logs in separately and `/logout` only
  invalidates the presented token.
- `/update` slash command that checks GitHub for a newer evrax release.
- Startup update check (notification only: a passive flash when a newer
  release exists, silent otherwise).
- `/worker N` (alias `/spawn`) spawns up to three headless worker agents
  rendered as panes beside the main transcript.
- Pane grid rendering for one to three workers, with the main agent acting as
  an orchestrator that splits work across them.
- `subagent` tool gains an `action=spawn` with a `count` argument so the agent
  can spawn workers when a task is heavy.
- `memory` tool: persistent per-project memory in `.evrax/memory.json`, upserted
  by key, so notes survive across turns and restarts.

### Changed

- Quiet transcript by default: thinking and successful tool calls render as one
  line; errors show up to four lines. Toggle with `ctrl+t` / `ctrl+o`.
- Diff previews are capped at 40 lines by default and 200 lines when expanded.
- Web remote keys tool rows by a per-call id, so repeated invocations of the
  same tool stay separate rows and finishing events update the right one.
- Web remote keeps the scroll position while the user reads history, and the
  chat view no longer overflows the viewport on mobile.
- Panic details (time, location, message, optional backtrace) are written to
  `%APPDATA%\evrax\last-panic.log`; the terminal is still restored first.

### Fixed

- `edit` and `write` on CRLF files: snippets are matched in LF space and written
  back with the file's original EOL. "text not found" errors now include the
  nearest line and a CRLF hint.
- Tunnel activation with a weak password: quick and custom-domain tunnels are
  blocked until a non-default password is set, and the password editor rejects
  weak values.
- Replies follow the user's language instead of drifting to another language.
- Web remote no longer renders a duplicate user bubble for prompts sent from
  the browser (the server echoes once, the TUI no longer echoes again).
- Approval decisions carry a nonce: a click that arrives after its dialog
  closed can no longer settle the next approval, and stale decisions are
  dropped when a dialog is settled from the terminal.
- Browser connection drops no longer appear as assistant "stream error"
  bubbles (native `EventSource` errors are told apart from server `error`
  frames).
- Session list order is stable across polls; the web UI is served with
  `Cache-Control: no-store` so an upgrade is never masked by a cached page.
- Request bodies are capped at 256 KiB.
- Large multi-line pastes become a `[pasted N chars]` chip instead of a heavy
  composer buffer.
- Drag-select repaints are throttled to ~60 fps while the highlight still
  updates on every event; worker panes poll their event log incrementally
  instead of re-reading the whole file each frame.

### Security

- `is_weak_password` (empty, whitespace, fewer than 8 characters, or
  `12345678`) is enforced by the server gate, `/login`, and the TUI guards.

### Removed

- No default MCP configuration is seeded or bundled. evrax starts with embedded
  skills and tools only; `%APPDATA%\evrax\mcp.json` is read only when the user
  provides it.

## [0.1.3] - 2026-10-22

### Added

- Multi-provider configuration (`providers[]`, active provider) with
  OpenAI-compatible and Anthropic backends.
- Provider wizard with key check, on-demand model list, and `/model` picker;
  manual model entry when fetching fails.
- Anthropic native client: messages, SSE tool use, and history conversion.
- Release assets for Windows: MSVC static build, ZIP, and `SHA256SUMS`;
  installer verified end to end.

### Changed

- English-only UI strings.
- Content-sized wizard/model dialogs with step paging.

### Fixed

- Wizard saves switch the active backend immediately, without a restart.

## [0.1.2] - 2026-10-21

### Fixed

- Saving the setup wizard now switches the active backend without restarting
  the app.

## [0.1.1] - 2026-10-21

### Added

- Project-scoped session persistence.
- CLI flags `-c`, `-s`, and `-l`.
- First-run setup wizard.

## [0.1.0] - 2026-10-21

### Added

- Initial release: Rust + ratatui TUI, streaming chat, tool calling, and the
  web remote.
