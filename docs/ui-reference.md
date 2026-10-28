# UI reference (TUI)

Catatan desain untuk implementasi TUI evrax. Referensi provenance: TUI proyek lain dipelajari untuk perilaku/layout; evrax mengimplementasikan ulang secara independen. Source: `agent-reference/opencode/packages/tui/src` (cloned repo). Purpose: avoid re-dissecting the source. All paths below are relative to `packages/tui/src` unless stated otherwise.

## 1. Layout

- Stack: SolidJS + OpenTUI (`@opentui/core`). Entry: `app.tsx` / `index.tsx`.
- Root: header line + body (session view + optional sidebar) + composer + footer hints.
- Session tabs (`component/session-tabs.tsx`, `component/session-tabs-rail.tsx`):
  - `tabs.layout`: `"horizontal"` (**default**) | `"vertical"` — config `packages/tui/src/config/index.tsx:196,335`.
  - Horizontal: tab strip (top). Vertical: right sidebar of session tabs, width **42** col.
  - Auto-compacts to a **5-col rail** (icons/spinners) when it doesn't fit; switch threshold via `sessionTabsFitVertically` in `ui/layout.ts`: fits when terminal width >= 42 + 64 (preferred content width) = **106**.
  - Rail controls row at bottom of rail: search/list button; `belowHighlighted` state.
- Right sidebar content slots (`feature-plugins/sidebar/*`, slot id `sidebar.content`):
  - `context.tsx`: "Context" — `N tokens`, `X% used`, `$Y spent` (hidden if no data/cost).
  - `mcp.tsx`: MCP servers panel.
  - `footer.tsx`: Getting-started onboarding card ("OpenCode includes free models...", connect 75+ providers) shown when height >= 22 and no provider connected; dismissible.
- Layout helpers: `ui/layout.ts`, `ui/pane-resize.ts` + `component/pane-resize-handle.tsx` (mouse-drag pane resizing), `ui/border.ts`.
- Toggle sidebar: `<leader>b` (`session.sidebar.toggle`). Focus left/right pane: `<leader>left` / `<leader>right`.
- Worker panes (evrax): `/worker N` (alias `/spawn`) men-spawn hingga tiga worker agent headless yang transcript-nya dirender sebagai pane di samping transcript utama (`main`). Layout: **1 worker = split kanan**; **2 = kiri-kanan**; **3 = grid**; **lebih dari 3 = tab**. Main agent bertindak sebagai orkestrator (di-nudge untuk memecah tugas jadi N subtask dan mengirim via tool `subagent action=send`).

## 2. Default Keybinds (config/keybind.ts)

Leader default: **ctrl+x** (`LeaderDefault`).

```
app:        exit ctrl+c, ctrl+d, <leader>q | clear ctrl+l | palette ctrl+p | help (none)
help/show:  help.show (none), docs.open (none), opencode.settings (none)
sessions:   new <leader>n | list <leader>l | open recent ctrl+o | rename ctrl+r | delete ctrl+d
            interrupt escape | compact <leader>c | export <leader>x | copy id (none)
            pin ctrl+f | queue prompt <leader>return | queued prompts <leader>q
            undo <leader>u | redo <leader>r | timeline <leader>g | fork (none)
tabs:       next ctrl+tab,alt+down | prev ctrl+shift+tab,alt+up | close <leader>w
            reopen ctrl+shift+t | select N <leader>N,ctrl+N | quick_switch N <leader>N
            history back (none) / forward ctrl+i
children:   child picker down | next child right | prev child left | parent up
models:     list <leader>m | cycle recent f2 / shift+f2 | provider list ctrl+a | favorite ctrl+f
agents:     list <leader>a | cycle shift+tab | variant.cycle ctrl+t | variant.list (none)
mcp:        list (none); plugins.list (none); plugins.toggle return
prompt:     submit return (input.submit) | newline shift+return,ctrl+return,alt+return,ctrl+j
            clear ctrl+c | paste ctrl+v | history prev/next up/down
            editor <leader>e | images <leader>i | stash (none)
            autocomplete: up/down, ctrl+p/ctrl+n, tab complete, escape hide
input nav:  left/right/up/down + ctrl+b/ctrl+f | home ctrl+a | end ctrl+e
            word fwd alt+f,alt+right,ctrl+right | word back alt+b,alt+left,ctrl+left
            delete: backspace | del ctrl+d,delete | del line ctrl+shift+d | del to end ctrl+k
            del to start ctrl+u | del word fwd alt+d | del word back ctrl+w,ctrl+backspace
            undo ctrl+- | redo ctrl+.
scroll:     pageup/pagedown, ctrl+alt+b/f | line ctrl+alt+y/e | half ctrl+alt+u/d
            first ctrl+g,home | last ctrl+alt+g,end | copy message <leader>y
            toggle thinking (none) | toggle scrollbar (none) | verbosity cycle (none)
diff:       close escape,q | next/prev hunk ] / [ | next/prev file n / p
            toggle file tree b | single patch s | source d | split/unified v
            fullscreen f | mark reviewed m | help ? | nav j/k, pgdn/pgup, gg, G
composer:   subagent up/down/return/ctrl+d | shell up/down/return/ctrl+d
            terminal up/down/return (k/j) | terminal.toggle <leader>t | close <leader>up
dialogs:    select prev/next up/down, ctrl+p/n | page pgup/pgdn | home/end
            submit return | mcp toggle space | plugins update ctrl+u, check ctrl+r
misc:       session.pair (none) | terminal.suspend ctrl+z | theme.switch (none)
            theme.switch_mode (none) | session.tab.next_unread alt+shift+down
which-key:  toggle ctrl+alt+k | layout ctrl+alt+shift+k | group prev/next ctrl+alt+left/right
            scroll up/down ctrl+alt+up/down | page ctrl+alt+pgup/pgdn
app.debug:  none | app.console none | app.toggle.animations none | app.scrap none
prompt.copy: (leader y copy message) | session.share deprecated (removed)
```

## 3. Composer / Prompt

- `component/prompt/index.tsx` + `routes/session/composer/*`:
  - Tabs: shell, subagents, terminals (`shell-tab.tsx`, `subagents-tab.tsx`, `terminals-tab.tsx`).
  - `metadata.tsx`: model/agent line under input (e.g. "Build · <model> · OpenCode · max").
  - `move.tsx`: move session (workspace/worktree). `local-attachment.ts`: image attachments. `draft-stash.ts`: stashed prompts.
- Autocomplete (`component/prompt/autocomplete.tsx`):
  - `@` prefix: files (dir drill-down inserts `@path/`), agents (`@agent.id`), skills (`@skill.id`), references.
  - `/` prefix: server commands / slash commands.
  - Selection markers/ordinals styled with `formfield` tokens.
- `feature-plugins/prompt/btw.tsx`: "btw" side-question input; `prompt/footer.tsx`: prompt footer hints.
- Queued prompts: queue with `<leader>return`; manage `<leader>q`; undo `ctrl+u`, delete `ctrl+d`.

## 4. Message Rendering (`routes/session/`)

- `index.tsx` orchestrates; parts in `message-parts.tsx`, `rows.ts`, grouping in `grouping/session.ts`, `grouping/tree.ts` (exploration/tool grouping, toggle via `session.toggle.exploration_grouping`).
- Thinking blocks: collapsible "Thought: <summary> · 1.2s" pattern; syntax in `thinking-syntax.ts`; toggle via `session.toggle.thinking`.
- User message: left accent bar; assistant: no bubble, content indent (paddingLeft 3).
- Diffs: `component/patch-diff.tsx` — split view auto when width > 120, else unified; sidebar file tree + viewer in `feature-plugins/system/diff-viewer*.tsx` (file menu, tree, image diffs).
- Navigation: `anchors.ts`, `anchor-view.tsx`, `history.ts`, `message-navigation.ts`; dialogs: `dialog-timeline.tsx`, `dialog-fork.tsx`, `dialog-message.tsx`, `dialog-execute.tsx` (run commands from message).
- Permissions: `permission.tsx` (approval prompt; `ctrl+f` fullscreen toggle).
- Utilities: `activity-summary.ts`, `form.tsx`, `mount-budget.ts`, `render-context.tsx`, `location-missing.tsx`.

## 5. Dialogs (component/)

session-list, session-rename, model, agent, config, mcp, integration, skill, stash, status, open, workspaces, worktree-name, workspace-file-changes, theme-list, variant, image-preview, shell-output, debug, experiments, error-details, highlights, update, pair.
UI primitives (`ui/`): dialog, dialog-select, dialog-confirm, dialog-alert, dialog-prompt, dialog-help, dialog-export-options/result, toast, file-path, link, spinner, select-controller, working-directory-actions.

## 6. Visual Primitives (component/)

logo, masked-text, fade-in-text, shimmer-text, title-shimmer, spinner + one-cell-spinner + spinner-frames, tab-pulse, reconnecting, retry-provider, startup-loading, migration-overlay, devtools-bar, error-component, terminal-pane, panel-host, plugin-route-missing, theme-error-toast.

## 7. Features Inventory

- Sessions: tabs (horizontal/vertical w/ 5-col rail), quick-switch slots, pin, unread markers, timeline, fork, child/subagent navigation, queued prompts, undo/redo, rename/delete, export transcript, message copy, compaction (`<leader>c`).
- Agents: multiple agents (build/plan/...), cycling, model variants; skills selector (`@skill`, `prompt.skills`).
- Models: provider integrations (75+), model dialog w/ favorites + recent cycling (f2), provider connect dialog.
- MCP: server list dialog, on/off toggles, sidebar panel.
- Permissions: approval prompts for tool execution with fullscreen variant.
- Diffs: file-tree viewer, split/unified, hunk nav, reviewed marks, image diffs, fullscreen.
- Terminal: docked terminal pane, shells/subagents/terminals composer tabs.
- Context: tokens, % used, cost — sidebar panel + status dialog.
- Themes: theme list dialog, light/dark mode switch + lock; semantic tokens (`text.*`, `background.*`, `action`, `formfield`, `feedback`) — theme assets JSON v2 (opencode dark palette in `evrax-tui/src/theme.rs`).
- Workspaces/worktrees: move session, manage worktrees, workspace file changes dialog.
- Meta: which-key panel (`ctrl+alt+k`), command palette (`ctrl+p`, fuzzy), plugins manager, experiments, debug/console panels, stats screen, storybook (dev-only), pair device, service restart, docs open, update dialog.

## 8. Theming Tokens (rules)

- Pick tokens by semantic role; never raw hue. `text.feedback`/`background.feedback` only for outcomes (errors/warnings/success); `formfield` for control text/ordinals/selection markers; `action` for actions.
- opencode dark palette: bg `#0a0a0a`, raised `#141414`, raised.high ~`#1e1e1e`, text `#eeeeee`, muted `#808080`, border `#484848`, accent `#9d7cd8`, interactive/orange `#fab283`, error `#e06c75`, success `#7fd88f`, diff add `#4fd6be` / remove `#c53b53`.

## 9. Evrax Status vs This Reference

- Done (U0-U3): layout skeleton (header, chat, right sidebar 42col, composer 3 lines, footer hints + cwd), opencode palette, chat entry model (user/assistant/thinking/tool/diff) + renderer, AgentEvent streaming via MockBackend, LineEditor, scroll (mouse + keys + clamp), resize handling, sidebar rule (>=106 col && maximized; compact rail not yet).
- Done (opencode-style selection): in-app drag selection over the render buffer clamped to the chat messages area only (sidebar/input/footer excluded; highlight + auto-copy on release to clipboard), ctrl+y copies last assistant reply, footer flash "copied N chars"; bracketed paste with whitespace collapse; composer horizontal scroll for long input. Drag-select repaints are throttled to ~60 fps (`App::paste_text_input` decision aside, the mouse `Drag(Left)` event updates the selection on every event but coalesces the repaint), so dragging stays smooth on a long transcript.
- Done (paste chip): a large multi-line paste collapses to a `[pasted N chars]` chip above the composer instead of dumping thousands of characters into the input buffer; short single-line text still inserts inline. `App::paste_text_input(text)` owns the decision (image paste is handled first by `App::ingest_paste`), and both the bracketed-paste event and the no-bracketed-paste key burst route through it.
- Sidebar detection: `evrax-tui/src/window.rs` `window_state()` — foreground terminal window class (`CASCADIA_HOSTING_WINDOW_CLASS` / `ConsoleWindowClass`) rect vs monitor work area, then font-pixel coverage heuristic, then classic conhost `IsZoomed`.
- Done (chat polish + modes): unicode-width word wrap with hanging indents for user/assistant/thinking/tool-detail (no more ragged col-0 continuation lines), tool names bold, diff context lines dimmed; Build/Plan mode toggle (opencode binds agent cycle to **shift+tab** — tab is autocomplete there; evrax accepts tab + shift+tab), composer metadata line under input ("Build · mock · max", mode label colored), plan mode tints the composer border accent purple.
- Done (diff backgrounds): diff rows use opencode theme v2 token values (`assets/v2/opencode.json` → `diff.background.*`): add content bg `#20303b`, rem `#37222c`, gutter (line-number) bg `#1b2b34`/`#2d1f26`, context gutter `#141414` (neutral.700), context text `#828bb8`; bands pad to full chat width (GitHub-style rows). OpenCode computes system-theme variants as `tint(bg, ansi green/red, 0.22)` in `theme/system.ts`.
- Done (footer + palette): footer left shows a single `/ commands` hint (enter/shift+tab/drag/ctrl+y/ctrl+p hints removed; busy state keeps `esc cancel · ctrl+c quit`; flash still appends), right side shows the workspace path like opencode. `ctrl+p` still opens the command palette overlay (rounded `commands` dialog, query line with cursor, filtered list, wrap-around selection, Enter runs / Esc closes): Switch to plan/build mode, Session list, Copy last reply, New session, Change theme, Quit evrax. Filter is nucleo-matcher fuzzy (subsequence scoring, sorted best-first) with a "no matching commands" empty state.
- Done (Batch A + modal fixes): palette/session dialogs go through a shared `modal()` helper that renders ratatui `Clear` first — fixes ghost text bleeding through popups (root cause: ratatui Block/Paragraph only set cell styles, they don't blank glyphs; `ratatui-widgets 0.3.2` block.rs `set_style` + paragraph.rs render only own graphemes). Permission prompt is NOT a popup — it docks inline between the message list and the composer like opencode (`routes/session/permission.tsx`): full chat-column width, accent `▌` left border, raised bg, `△ Permission required` header + tool name/summary, option buttons in one row (`Allow once` / `Always allow` / `Reject`) with left/right (or h/l) selection, `⇆ select · enter confirm` hints, Esc rejects; "Always allow" auto-approves the same tool for the rest of the session. Session dialog (`ctrl+o`, rows: new session / current / saved mock sessions, Enter switches name + clears chat). `ctrl+n` new session. Esc twice within 5s cancels a running turn (`PRESS ESC AGAIN TO CANCEL` flash on first press). Chat viewport scrollbar (VerticalRight, track border gray, thumb muted, only when content overflows). Sidebar now shows the current session name + shortcuts (incl. ctrl+o).
- Done (Batch B): markdown rendering for assistant replies via in-house `crates/evrax-tui/src/markdown.rs` (no new deps). Structure follows opencode's opentui renderer (conceal mode, block spacing, no code band), but letter colors use the evrax purple family (accent `#9d7cd8` / purple.100 `#b38ff4` / purple.300 `#8869bd`) instead of the opencode markdown rainbow: conceal mode (markers `**`, `*`, backticks, `#` hidden), heading accent bold (h1 underlined), strong purple.100 bold, emphasis accent italic, blockquote purple.300 italic, inline code purple.100 (no band), list markers `-`/`n.` accent with hanging indent, links label accent + `(url)` purple.300 underlined, blockquote rail `│` dim, `---` renders as a full-width rule line, blank line between separated blocks (heading/list/code/quote/rule) like opentui marginTop. Fenced code blocks sit on the base background (no raised band) with a lightweight per-language syntax highlighter in the same purple scale (comment/operator mute — comment italic, string accent, number purple.100, keyword purple.100 italic, function accent, type purple.300, variable plain text) for rust/js/ts/go/py/sh/json/toml/yaml/ruby/php/c-family; unknown/empty language stays plain. Span-aware word wrap keeps styles across wrapped rows; unclosed markers stay literal (streaming-safe). Multi-line composer: enter submits; shift+enter / ctrl+enter / alt+enter / ctrl+j insert a newline (opencode keybinds `input.newline`; kitty keyboard enhancement pushed best-effort so shift+enter is distinguishable); up/down move between lines (column-preserving, clamped), home/end are line-wise; the composer box grows with content up to six rows; bracketed paste keeps newlines and collapses runs of spaces per line. The demo conversation and MockBackend scripts now stream markdown so every change is visible on `cargo run`.
- Done (Batch B+): slash commands in the composer, mirroring opencode's autocomplete popup: typing `/` as the first character of a message opens a "commands" popup anchored right above the composer (bordered rounded box, raised bg, same Clear-based modal hygiene as the palette). Live fuzzy filter (nucleo; name matches rank above description matches, aliases surface as their own rows like opencode), up/down move the selection, Enter/Tab runs the command, Esc dismisses until the text changes, space or a second `/` closes the popup. Registry: `/new` (new session), `/sessions` (+resume, continue), `/mode` (toggle build/plan), `/copy` (+yank), `/exit` (+quit, q); commands reuse the same `execute_command` path as the ctrl+p palette and clear the composer when run. Enter with an empty match list dismisses and submits the text as a normal prompt. `/worker <n>` (+`/spawn`) spawns up to three headless worker agents rendered as panes; the main agent orchestrates the split.
- Done (`/update`): `/update` (check for a newer evrax release) fetches `api.github.com/repos/jackielnikkie/another-evrax-agents/releases/latest` and always reports the outcome: `update available: vX (you: <cur>) · <url>`, `up to date: <cur>`, or `update check failed: <err>`. Startup runs the same check once in the background and shows a passive flash `update available: vX — /update for details` only when a newer release exists (silent when up to date or on failure).
- Next (U5): footer token usage + real model/variant in the metadata line, sidebar Context panel (tokens/%/cost), getting-started onboarding card.
- Done (Batch B+ themes): `/theme` (plus "Change theme" in the palette) opens a centered "themes" dialog with live preview: up/down repaints instantly, enter saves the choice to `%APPDATA%\evrax\theme` and flashes `theme · <name>`, esc reverts to the theme active when the dialog opened; the saved key is loaded on startup. Colors now flow through `theme::current() -> &'static Theme` (thread-local index over the `THEMES` presets in theme.rs) instead of compile-time constants. Presets: "evrax" (default; purple markdown/syntax scale, accent `#9d7cd8` / purple.100 `#b38ff4` / purple.300 `#8869bd`) and "opencode" (original v2 rainbow letters per `assets/v2/opencode.json`: strong `#f5a742`, emphasis/quote `#e5c07b`, inline code green `#7fd88f`, list/link orange `#fab283`, link text cyan `#56b6c2`, rule `#808080`; syntax comment muted, keyword `#9d7cd8` italic, function orange, variable red `#e06c75`, string green, number `#f5a742`, type `#e5c07b`, operator cyan `#56b6c2`), plus five community presets per their official theme assets (`packages/tui/src/theme/assets/`): catppuccin, dracula, gruvbox, nord, tokyonight — each row in the dialog shows a two-dot swatch (accent + interactive). Switching themes recolors the whole chrome, not just letters: the opencode preset drives `accent` with interactive.200 orange (`#fab283`) and `selection`/`action_dim` with the orange.800 tint (`#3d1d06`), so brand, markers, mode labels, palette cursor, selected modal rows and the approval buttons all repaint with the theme.
- Milestones after UI: M1 provider SSE (9router local), M2 agent loop + tools + approval, M3 SQLite sessions + resume + palette integration, M4 markdown/usage/AGENTS.md chain, M5 MCP/skills/compaction/subagents.
