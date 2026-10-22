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

### Changed

- Quiet transcript by default: thinking and successful tool calls render as one
  line; errors show up to four lines. Toggle with `ctrl+t` / `ctrl+o`.
- Diff previews are capped at 40 lines by default and 200 lines when expanded.

### Fixed

- `edit` and `write` on CRLF files: snippets are matched in LF space and written
  back with the file's original EOL. "text not found" errors now include the
  nearest line and a CRLF hint.
- Tunnel activation with a weak password: quick and custom-domain tunnels are
  blocked until a non-default password is set, and the password editor rejects
  weak values.
- Replies follow the user's language instead of drifting to another language.

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
