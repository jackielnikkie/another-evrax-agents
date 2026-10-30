# Changelog

All notable changes to evrax are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions use
[semantic versioning](https://semver.org/).

## [Unreleased]

## [0.1.8] - 2026-10-30

### Added

- SOUL.md identity file loaded into the system prompt, chained with `AGENTS.md`
  (CLAUDE.md and .cursorrules are no longer consulted).
- Chat rendering and the today list now use a grouped, labelled layout.
- Large text pastes collapse to a `[pasted N chars]` chip.
- `/workers` lists, clears, or kills this project's workers (`/workers clear`,
  `/workers kill <id>`).

### Changed

- Worker unification: the `subagent` tool is now `worker` (spawn/send/status/
  list/stop), and headless worker panes use the same system. State lives under
  `%APPDATA%\evrax\workers\<id>\` (`EVRAX_WORKERS_DIR` overrides the base), and
  the internal flag is `--worker-id`.
- Workers are scoped per project and pruned, so stale workers from other
  projects or finished sessions no longer pile up or show up in `worker list`.
- Workers persist across sessions: resuming a session reconnects its worker
  panes instead of starting empty.
- Shell approval now matches files: a `shell` call inside the workspace runs
  without a prompt, while a call outside the workspace asks first.
- Worker (headless) sessions may run shell, instead of refusing every shell
  call.

### Fixed

- Guardrail duplicate-result stub no longer misfires on `read` calls that
  legitimately return identical content.

## [0.1.7] - 2026-10-28

### Fixed

- `HTTP 413 Payload Too Large`: a single large tool result (a big `read`,
  `shell`, `web_fetch`, or search) is now capped before it is sent back to the
  provider, and the whole request is trimmed (oldest history dropped first) to
  stay under a body limit, so a long conversation or a heavy tool call no
  longer trips the gateway's limit.
- A `413` now reports a clear message (start a new session or keep tool output
  smaller) instead of the raw status.

## [0.1.6] - 2026-10-28

### Added

- `web_search` tool: searches the web (DuckDuckGo HTML endpoint) and returns the
  top results as title, URL, and snippet. No browser or API key required.
- `skills_list` / `skill_view` tools surface the embedded skills, so the agent
  can pull in specialized playbooks (search, debugging, TDD, and more) on demand.
- The system prompt now carries an engineering brief (gather context before
  editing, never invent symbols, batch independent lookups, fix root causes,
  verify before finishing), a live workspace snapshot (branch, status, recent
  commits, manifests, verify commands), and the project's `AGENTS.md` when
  present.
- Worker panes are only spawned by `/worker N`; the main agent acts as the
  orchestrator through the `subagent` tool instead of an injected prompt.

### Changed

- The composer stays editable while a turn streams: a small spinner sits above
  the input, typed text shows live, slash commands run immediately, and plain
  text is queued.
- `AGENTS.md` is loaded into the system prompt (capped) so project conventions
  apply without repeating them each session.

### Fixed

- `/worker N` no longer starts a turn on its own or shows an internal
  orchestrator prompt as a user message.

## [0.1.5] - 2026-10-28

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
- Update checks no longer fail with `HTTP 403` when the GitHub API's anonymous
  per-IP rate limit (60/hour) is exhausted. `check_update` tries the API first
  and falls back to the `/releases/latest` web redirect, which has no rate
  limit and needs no token.

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
