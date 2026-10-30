use std::collections::{HashSet, VecDeque};
use std::time::Instant;

use evrax_agent::catalog::Catalog;
use evrax_core::{AgentEvent, DiffLine};
use evrax_store::{ProjectStore, SessionMeta};
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use ratatui::layout::Rect;

use crate::theme;

pub const ESC_CANCEL_TICKS: u64 = 20;
pub const APPROVAL_OPTIONS: [&str; 3] = ["Allow once", "Always allow", "Reject"];

/// A background `GET /models` job (wizard or `/model` dialog).
pub struct FetchRequest {
    pub request_id: u64,
    pub kind: evrax_agent::ProviderKind,
    pub base_url: String,
    pub api_key: Option<String>,
}

/// Reply channel payload for a finished fetch: `(request_id, models)`.
pub type FetchResult = (u64, Result<Vec<String>, String>);

pub struct App {
    pub should_quit: bool,
    pub tick: u64,
    pub cwd: String,
    /// Project root workers are scoped to (set from the loaded config).
    pub root: std::path::PathBuf,
    pub messages: Vec<ChatEntry>,
    pub input: LineEditor,
    pub busy: bool,
    /// Current tool-calling iteration inside the running turn (`n/max`).
    pub iteration: Option<(usize, usize)>,
    /// Wall-clock start of the running turn, for the footer elapsed timer.
    pub turn_started: Option<Instant>,
    /// Prompts typed while a turn was streaming, run in order once it ends.
    pub queued: VecDeque<String>,
    /// Transcript detail level: false = quiet (tool bodies collapsed),
    /// true = everything expanded (`ctrl+t` / `ctrl+o`).
    pub expanded: bool,
    pub mode: AgentMode,
    pub scroll_from_bottom: u16,
    pub viewport_height: u16,
    /// Scroll bound measured while rendering the last frame. `ui::messages` fills it;
    /// the event loop reads it instead of rebuilding the whole transcript again.
    pub frame_max_scroll: u16,
    /// Which backend is really wired up ("bebas", "mock", ...) — shown in the
    /// status line and the sidebar.
    pub backend_label: String,
    pub catalog: Catalog,
    pub maximized: Option<bool>,
    pub selectable_area: Rect,
    pub selection: Selection,
    pub copied_text: Option<String>,
    pub flash: Option<(String, u64)>,
    pub palette: Option<Palette>,
    pub slash: Option<SlashMenu>,
    slash_dismissed: Option<String>,
    pub approval: Option<Approval>,
    /// Per-session sequence stamped on remote `tool` events so the web can keep
    /// repeated same-name tool calls as distinct rows.
    remote_tool_seq: u64,
    /// Per-session sequence stamped on remote `approval` events; the web echoes
    /// it back so a stale decision can never settle a later approval.
    remote_approval_seq: u64,
    /// Test-only: every remote event this app tried to emit, in order.
    #[cfg(test)]
    pub(crate) remote_log: Vec<evrax_remote::RemoteEvent>,
    pub auto_allow: HashSet<String>,
    pub session_dialog: Option<SessionDialog>,
    pub theme_dialog: Option<ThemeDialog>,
    pub remote_dialog: Option<RemoteDialog>,
    pub tunnel_dialog: Option<TunnelDialog>,
    /// Screen rect of the tunnel dialog's status/url line (click copies it).
    pub tunnel_url_rect: Option<Rect>,
    pub remote: Option<RemoteState>,
    remote_server: Option<evrax_remote::RemoteServer>,
    last_tunnel_key: Option<String>,
    pub attachments: Vec<Attachment>,
    /// Large multi-line pastes parked as chips; merged into the prompt at submit.
    pub pasted_text: Vec<PastedText>,
    pub preview: Option<usize>,
    pub pending_kitty: Vec<String>,
    pub chip_rects: Vec<Rect>,
    pub chip_x_rects: Vec<Rect>,
    pub wizard: Option<Wizard>,
    /// Config saved by the setup wizard; the run loop swaps the backend once.
    pub reload_config: Option<evrax_agent::AgentConfig>,
    /// Saved providers + the active one (loaded from `agent.json`).
    pub providers: Vec<evrax_agent::Provider>,
    pub active_provider: Option<String>,
    pub max_iterations: usize,
    pub config_mock: bool,
    /// Runtime provider/model picker (`/model`).
    pub model_dialog: Option<ModelDialog>,
    fetch_seq: u64,
    pub session_id: String,
    pub sessions: Vec<String>,
    /// Project store for on-disk persistence (None with `--no-save`/`--mock`).
    store: Option<ProjectStore>,
    /// Message history kept per named session (replayed when switching back).
    pub transcripts: std::collections::HashMap<String, Vec<ChatEntry>>,
    /// Display titles per session id; `session_id` itself never changes so the
    /// backend history and remote registry stay intact across renames.
    pub titles: std::collections::HashMap<String, String>,
    /// Sessions whose title was set manually; LLM suggestions must not
    /// overwrite them.
    pub manual_titles: std::collections::HashSet<String>,
    /// Sessions for which an LLM title request has already been fired.
    title_pending: std::collections::HashSet<String>,
    /// Queued LLM title job picked up by the event loop (session, prompt, reply).
    title_job: Option<(String, String, String)>,
    /// Worker panes rendered alongside the main transcript (`/worker N`).
    pub workers: Vec<WorkerPane>,
    /// Highest worker ordinal handed out (for "worker N" naming).
    worker_seq: usize,
    /// Pending headless-spawn count consumed by the event loop (`/worker N`).
    worker_spawn_request: Option<usize>,
    /// True when the last requested update check should always report its result.
    update_explicit: bool,
    /// True while an update check is in flight (blocks a second spawn).
    update_inflight: bool,
    /// Flattened transcript lines, rebuilt per entry only when it changes.
    pub transcript: crate::ui::TranscriptCache,
    /// Bumped whenever transcript content changes; lets the render cache skip
    /// tail rebuilds on pure keystroke repaints.
    pub content_rev: u64,
    pub last_esc: Option<u64>,
    asst_open: bool,
}

pub struct Approval {
    pub name: String,
    pub summary: String,
    /// Identity for "always allow": tool name, or `tool:dir` for outside-workspace
    /// access, so granting one folder does not open the whole filesystem.
    pub key: String,
    pub selected: usize,
    /// Nonce broadcast to the web; a decision may only settle this approval.
    pub id: u64,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ApprovalDecision {
    Once,
    Always,
    Reject,
}

pub const WIZARD_PRESETS: [(&str, &str, &str); 4] = [
    ("9router", "http://127.0.0.1:20128/v1", "bebas"),
    (
        "OpenRouter",
        "https://openrouter.ai/api/v1",
        "deepseek/deepseek-chat",
    ),
    ("DeepSeek", "https://api.deepseek.com/v1", "deepseek-chat"),
    ("Custom", "", ""),
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WizardStep {
    Providers,
    Kind,
    Name,
    BaseUrl,
    ApiKey,
    Checking,
    Models,
    Review,
}

pub struct Wizard {
    pub step: WizardStep,
    /// Provider chooser row: `0..providers.len()` = existing, last = new.
    pub cursor: usize,
    pub editing: bool,
    pub kind: evrax_agent::ProviderKind,
    pub name: String,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub models: Vec<String>,
    pub filtered: Vec<usize>,
    pub model_cursor: usize,
    pub filter: String,
    pub existing: bool,
    pub preset: usize,
    pub request_id: u64,
    pub status: Option<String>,
}

impl Wizard {
    pub(crate) fn refilter(&mut self) {
        let needle = self.filter.trim().to_lowercase();
        self.filtered = self
            .models
            .iter()
            .enumerate()
            .filter(|(_, model)| needle.is_empty() || model.to_lowercase().contains(&needle))
            .map(|(index, _)| index)
            .collect();
        if self.model_cursor >= self.filtered.len() {
            self.model_cursor = 0;
        }
    }
}

pub enum WizardAction {
    None,
    FetchModels(FetchRequest),
    Save(evrax_agent::ConfigSettings),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ModelLevel {
    Providers,
    Models,
}

pub struct ModelDialog {
    pub level: ModelLevel,
    pub provider_cursor: usize,
    pub model_cursor: usize,
    pub models: Vec<String>,
    pub filtered: Vec<usize>,
    pub filter: String,
    pub request_id: u64,
    pub loading: bool,
    pub status: Option<String>,
}

impl ModelDialog {
    pub(crate) fn refilter(&mut self) {
        let needle = self.filter.trim().to_lowercase();
        self.filtered = self
            .models
            .iter()
            .enumerate()
            .filter(|(_, model)| needle.is_empty() || model.to_lowercase().contains(&needle))
            .map(|(index, _)| index)
            .collect();
        if self.model_cursor >= self.filtered.len() {
            self.model_cursor = 0;
        }
    }
}

pub enum ModelAction {
    None,
    FetchModels(FetchRequest),
    Apply(evrax_agent::ConfigSettings),
}

pub struct SessionDialog {
    pub selected: usize,
}

pub struct ThemeDialog {
    pub selected: usize,
    pub original: usize,
}

/// Remote web state for this session (ON/OFF toggle lives in the dialog).
pub struct RemoteState {
    pub enabled: bool,
    pub permit: evrax_remote::Permit,
    pub session_id: String,
    /// Live config: bind/port from `remote.json`; password editable in the dialog.
    pub config: evrax_remote::RemoteConfig,
    /// External tunnel process (cloudflared) managed for this session.
    pub tunnel: evrax_remote::tunnel::TunnelManager,
}

pub struct RemoteDialog {
    pub selected: usize,
    /// Some(buffer) while the password row is being edited inline.
    pub editing: Option<String>,
}

pub struct TunnelDialog {
    pub selected: usize,
    /// Some(buffer) while an input row is being edited inline.
    pub editing: Option<String>,
    /// Which editor the buffer belongs to (manual URL or custom hostname).
    pub kind: TunnelEditKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunnelEditKind {
    ManualUrl,
    Hostname,
}

/// Follow-up the caller must run after a tunnel dialog action.
pub enum TunnelEffect {
    None,
    Copy(String),
    Start,
}

/// Outcome of finishing a tunnel inline edit (caller persists + reacts).
#[derive(Debug, PartialEq, Eq)]
pub enum TunnelEditResult {
    UrlSaved,
    UrlCleared,
    /// Custom hostname stored: the caller should start the domain tunnel.
    HostReady,
}

fn tunnel_status_key(status: &evrax_remote::tunnel::TunnelStatus) -> String {
    use evrax_remote::tunnel::TunnelStatus;
    match status {
        TunnelStatus::Off => "off".to_string(),
        TunnelStatus::Downloading => "downloading".to_string(),
        TunnelStatus::Starting => "starting".to_string(),
        TunnelStatus::Login { url } => format!("login:{url}"),
        TunnelStatus::Running { url } => format!("running:{url}"),
        TunnelStatus::Error { message } => format!("error:{message}"),
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Command {
    ToggleMode,
    Sessions,
    CopyReply,
    NewSession,
    Theme,
    Model,
    Remote,
    Rename,
    Update,
    Worker,
    Workers,
    Quit,
}

pub struct SlashMenu {
    pub selected: usize,
}

pub struct SlashCommand {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub description: &'static str,
    pub command: Command,
}

pub const SLASH_COMMANDS: &[SlashCommand] = &[
    SlashCommand {
        name: "new",
        aliases: &[],
        description: "start a new session",
        command: Command::NewSession,
    },
    SlashCommand {
        name: "sessions",
        aliases: &["resume", "continue"],
        description: "switch to another session",
        command: Command::Sessions,
    },
    SlashCommand {
        name: "mode",
        aliases: &[],
        description: "toggle build/plan mode",
        command: Command::ToggleMode,
    },
    SlashCommand {
        name: "copy",
        aliases: &["yank"],
        description: "copy the last assistant reply",
        command: Command::CopyReply,
    },
    SlashCommand {
        name: "theme",
        aliases: &[],
        description: "change the color theme",
        command: Command::Theme,
    },
    SlashCommand {
        name: "model",
        aliases: &["provider"],
        description: "switch provider or model",
        command: Command::Model,
    },
    SlashCommand {
        name: "remote",
        aliases: &[],
        description: "remote web access for this session",
        command: Command::Remote,
    },
    SlashCommand {
        name: "rename",
        aliases: &[],
        description: "set this session's title",
        command: Command::Rename,
    },
    SlashCommand {
        name: "update",
        aliases: &[],
        description: "check for a newer evrax release",
        command: Command::Update,
    },
    SlashCommand {
        name: "worker",
        aliases: &["spawn"],
        description: "spawn worker agents for parallel work",
        command: Command::Worker,
    },
    SlashCommand {
        name: "workers",
        aliases: &[],
        description: "list/clear/kill this project's workers",
        command: Command::Workers,
    },
    SlashCommand {
        name: "exit",
        aliases: &["quit", "q"],
        description: "quit evrax",
        command: Command::Quit,
    },
];

pub type SlashOption = (&'static str, &'static str, Command);

pub fn slash_options() -> Vec<SlashOption> {
    SLASH_COMMANDS
        .iter()
        .flat_map(|entry| {
            std::iter::once((entry.name, entry.description, entry.command)).chain(
                entry
                    .aliases
                    .iter()
                    .map(move |alias| (*alias, entry.description, entry.command)),
            )
        })
        .collect()
}

/// Rows shown per page in scrollable dialogs (palette, slash menu, lists).
pub const DIALOG_PAGE: usize = 8;

/// Transcript entries kept per session in memory (oldest are dropped first).
pub const MAX_ENTRIES_PER_SESSION: usize = 400;

/// Background sessions whose transcripts and titles are retained (LRU).
pub const MAX_RETAINED_SESSIONS: usize = 16;

/// Minimum remote password length enforced by the in-TUI editor.
pub const REMOTE_MIN_PASSWORD: usize = 8;

/// Prompts allowed to wait while a turn streams (newest beyond this are dropped).
pub const MAX_QUEUED_PROMPTS: usize = 8;

/// Cap for simultaneous worker panes; beyond this they open as tabs.
pub const MAX_WORKERS: usize = 3;

/// Diff lines rendered before the "+N more" trailer while the view is quiet.
pub const DIFF_PREVIEW_LINES: usize = 40;

/// Body lines kept for an errored tool while quiet (successes show none).
pub const QUIET_ERROR_LINES: usize = 4;

pub struct Palette {
    pub query: String,
    pub selected: usize,
}

#[derive(Default)]
pub struct Selection {
    anchor: Option<(u16, u16)>,
    head: Option<(u16, u16)>,
    dragging: bool,
    copy_requested: bool,
}

impl Selection {
    pub fn begin(&mut self, pos: (u16, u16)) {
        self.anchor = Some(pos);
        self.head = Some(pos);
        self.dragging = true;
        self.copy_requested = false;
    }

    pub fn update(&mut self, pos: (u16, u16)) {
        if self.dragging {
            self.head = Some(pos);
        }
    }

    pub fn finish(&mut self) {
        if self.dragging && self.anchor != self.head {
            self.copy_requested = true;
        }
        self.dragging = false;
    }

    pub fn clear(&mut self) {
        self.anchor = None;
        self.head = None;
        self.dragging = false;
        self.copy_requested = false;
    }

    pub fn normalized(&self) -> Option<(u16, u16, u16, u16)> {
        let (ax, ay) = self.anchor?;
        let (hx, hy) = self.head?;
        let (start, end) = if (ay, ax) <= (hy, hx) {
            ((ax, ay), (hx, hy))
        } else {
            ((hx, hy), (ax, ay))
        };
        Some((start.0, start.1, end.0, end.1))
    }

    pub fn is_dragging(&self) -> bool {
        self.dragging
    }

    pub fn head(&self) -> Option<(u16, u16)> {
        self.head
    }

    pub fn shift_anchor(&mut self, up: bool, delta: u16) {
        if let Some((x, y)) = self.anchor {
            let y = if up {
                y.saturating_add(delta)
            } else {
                y.saturating_sub(delta)
            };
            self.anchor = Some((x, y));
        }
    }

    pub fn take_copy_request(&mut self) -> Option<(u16, u16, u16, u16)> {
        if std::mem::take(&mut self.copy_requested) {
            self.normalized()
        } else {
            None
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
pub enum ChatEntry {
    User(String),
    Assistant(String),
    Thinking { summary: String, seconds: f32 },
    Tool(ToolCall),
    Diff { path: String, lines: Vec<DiffLine> },
}

/// One headless worker agent rendered in a pane beside the main transcript.
/// The process itself is spawned by the event loop (`worker_spawn_request`);
/// this struct is only the view state, refreshed from the worker's state file
/// and `events.jsonl` on every tick.
pub struct WorkerPane {
    pub id: String,
    pub name: String,
    pub status: String,
    pub pid: u32,
    pub transcript: Vec<ChatEntry>,
    pub last_reply: String,
    pub cache: crate::ui::TranscriptCache,
    /// Number of `events.jsonl` lines already folded into `transcript`.
    pub events_seen: usize,
}

pub struct Attachment {
    pub id: u32,
    pub path: String,
    pub name: String,
    pub width: u32,
    pub height: u32,
}

/// A large text paste parked as a chip instead of living in the line editor.
/// Keeping multi-line pastes out of `LineEditor.chars` avoids re-wrapping the
/// whole buffer on every repaint; the body is attached to the prompt at submit.
pub struct PastedText {
    pub chars: usize,
    pub body: String,
}

/// Stable per-session id counter for kitty graphics placement.
static NEXT_ATTACHMENT_ID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);

/// Millisecond epoch as a compact string, for unique session ids.
fn s_now() -> u128 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// The machine's LAN IPv4. A UDP connect only selects a route (no packets are
/// sent), which is enough to learn the address of the default interface.
fn local_ipv4() -> Option<String> {
    let socket = std::net::UdpSocket::bind(("0.0.0.0", 0)).ok()?;
    socket.connect(("8.8.8.8", 80)).ok()?;
    match socket.local_addr().ok()?.ip() {
        std::net::IpAddr::V4(ip) if !ip.is_loopback() => Some(ip.to_string()),
        _ => None,
    }
}

/// Unique session name: millisecond epoch + a process-local counter.
fn unique_session_id() -> String {
    static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("session-{}-{n}", s_now())
}

/// Placeholder title from a prompt: first prose line, markdown stripped,
/// cut to a short word boundary. Used until the model proposes a real title.
pub fn title_from_prompt(prompt: &str) -> Option<String> {
    let line = prompt
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with("```"))?;
    if line.starts_with('/') {
        return None;
    }
    let mut words: Vec<String> = Vec::new();
    for w in line.split_whitespace() {
        let clean = w.trim_matches(|c: char| {
            matches!(c, '#' | '*' | '`' | '>' | '_' | '~' | '[' | ']' | '(' | ')')
        });
        if clean.is_empty() {
            continue;
        }
        words.push(clean.to_string());
        if words.len() == 6 {
            break;
        }
    }
    if words.is_empty() {
        return None;
    }
    let mut title = words.join(" ");
    title = title
        .trim_matches(|c: char| {
            c.is_whitespace()
                || matches!(
                    c,
                    '"' | '\'' | ':' | ';' | '.' | ',' | '!' | '?' | '-' | '#'
                )
        })
        .to_string();
    if title.chars().count() > 48 {
        let mut cut: String = title.chars().take(48).collect();
        if let Some(pos) = cut.rfind(' ') {
            cut.truncate(pos);
        }
        title = cut
            .trim_end_matches(|c: char| {
                matches!(c, '"' | '\'' | ':' | ';' | '.' | ',' | '!' | '?' | '-')
            })
            .to_string();
    }
    if title.chars().count() < 2 {
        return None;
    }
    Some(title)
}

/// Copies a staged clipboard image to a friendlier `screenshot-HHMMSS.png`
/// name so the composer chip stays readable.
fn stage_named_copy(staged: &str) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let name = format!(
        "screenshot-{:02}{:02}{:02}.png",
        secs / 3600 % 24,
        secs / 60 % 60,
        secs % 60
    );
    let dest = std::env::temp_dir().join(&name);
    if std::fs::copy(staged, &dest).is_ok() {
        dest.display().to_string()
    } else {
        staged.to_string()
    }
}

impl Attachment {
    pub fn from_path(path: &str) -> Option<Self> {
        if !crate::image::is_image_path(path) {
            return None;
        }
        let data = std::fs::read(path).ok()?;
        let (width, height) = crate::image::png_dimensions(&data).unwrap_or((0, 0));
        let name = std::path::Path::new(path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("image.png")
            .to_string();
        Some(Self {
            id: NEXT_ATTACHMENT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            path: path.trim().to_string(),
            name,
            width,
            height,
        })
    }

    pub fn label(&self) -> String {
        let name = if self.name.is_empty() {
            self.path.as_str()
        } else {
            self.name.as_str()
        };
        if self.width > 0 {
            format!("ðŸ–¼ {} ({}Ã—{})", name, self.width, self.height)
        } else {
            format!("ðŸ–¼ {name}")
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct ToolCall {
    pub name: String,
    pub summary: String,
    pub status: ToolStatus,
    pub detail: String,
    /// Remote call id assigned when this row starts; the finishing event
    /// reuses it so the web keeps one row per call.
    #[serde(default)]
    pub call: u64,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub enum ToolStatus {
    /// Arguments are still streaming in (or the call waits for approval).
    Preparing,
    Running,
    Done,
    Error,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AgentMode {
    Build,
    Plan,
}

impl AgentMode {
    pub fn label(self) -> &'static str {
        match self {
            AgentMode::Build => "Build",
            AgentMode::Plan => "Plan",
        }
    }

    fn toggled(self) -> Self {
        match self {
            AgentMode::Build => AgentMode::Plan,
            AgentMode::Plan => AgentMode::Build,
        }
    }
}

pub struct LineEditor {
    chars: Vec<char>,
    cursor: usize,
    /// Bumped on every mutation so `visual_rows` can skip rebuilding.
    rev: u64,
    rows_cache: Vec<String>,
    rows_rev: u64,
    rows_width: usize,
}

impl LineEditor {
    pub fn new() -> Self {
        Self {
            chars: Vec::new(),
            cursor: 0,
            rev: 0,
            rows_cache: Vec::new(),
            rows_rev: u64::MAX,
            rows_width: 0,
        }
    }

    pub fn text(&self) -> String {
        self.chars.iter().collect()
    }

    pub fn is_empty(&self) -> bool {
        self.chars.is_empty()
    }

    #[cfg(test)]
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn insert_char(&mut self, c: char) {
        self.chars.insert(self.cursor, c);
        self.cursor += 1;
        self.rev = self.rev.wrapping_add(1);
    }

    pub fn insert_str(&mut self, text: &str) {
        let chars: Vec<char> = text.chars().collect();
        self.chars.splice(self.cursor..self.cursor, chars);
        self.cursor += text.chars().count();
        self.rev = self.rev.wrapping_add(1);
    }

    pub fn insert_newline(&mut self) {
        self.insert_char('\n');
    }

    /// Rebuilds the wrapped rows only when the buffer or the width changed.
    fn ensure_rows(&mut self, width: usize) {
        let width = width.max(1);
        if self.rows_rev == self.rev && self.rows_width == width {
            return;
        }
        let text = self.text();
        let mut rows: Vec<String> = Vec::new();
        for line in text.split('\n') {
            let chars: Vec<char> = line.chars().collect();
            if chars.is_empty() {
                rows.push(String::new());
                continue;
            }
            let mut i = 0;
            while i < chars.len() {
                let end = (i + width).min(chars.len());
                rows.push(chars[i..end].iter().collect());
                i = end;
            }
        }
        if rows.is_empty() {
            rows.push(String::new());
        }
        self.rows_cache = rows;
        self.rows_rev = self.rev;
        self.rows_width = width;
    }

    /// Wrapped rows, cached across repaints (only rebuilt when `rev`/width change).
    pub fn visual_rows(&mut self, width: usize) -> &[String] {
        self.ensure_rows(width);
        &self.rows_cache
    }

    pub fn cursor_visual(&self, width: usize) -> (usize, usize) {
        let width = width.max(1);
        let (row, col) = self.cursor_line_col();
        let text = self.text();
        let mut rows_before = 0;
        for line in text.split('\n').take(row) {
            let len = line.chars().count();
            rows_before += if len == 0 { 1 } else { len.div_ceil(width) };
        }
        (rows_before + col / width, col % width)
    }

    pub fn cursor_line_col(&self) -> (usize, usize) {
        let mut row = 0;
        let mut col = 0;
        for (i, c) in self.chars.iter().enumerate() {
            if i == self.cursor {
                break;
            }
            if *c == '\n' {
                row += 1;
                col = 0;
            } else {
                col += 1;
            }
        }
        (row, col)
    }

    fn line_bounds(&self) -> (usize, usize) {
        let start = self.chars[..self.cursor]
            .iter()
            .rposition(|c| *c == '\n')
            .map(|i| i + 1)
            .unwrap_or(0);
        let end = self.chars[self.cursor..]
            .iter()
            .position(|c| *c == '\n')
            .map(|i| self.cursor + i)
            .unwrap_or(self.chars.len());
        (start, end)
    }

    pub fn backspace(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.chars.remove(self.cursor);
            self.rev = self.rev.wrapping_add(1);
        }
    }

    pub fn delete(&mut self) {
        if self.cursor < self.chars.len() {
            self.chars.remove(self.cursor);
            self.rev = self.rev.wrapping_add(1);
        }
    }

    pub fn move_left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn move_right(&mut self) {
        if self.cursor < self.chars.len() {
            self.cursor += 1;
        }
    }

    pub fn home(&mut self) {
        self.cursor = self.line_bounds().0;
    }

    pub fn end(&mut self) {
        self.cursor = self.line_bounds().1;
    }

    pub fn move_up(&mut self) {
        let (start, _) = self.line_bounds();
        if start == 0 {
            return;
        }
        let col = self.cursor - start;
        let prev_end = start - 1;
        let prev_start = self.chars[..prev_end]
            .iter()
            .rposition(|c| *c == '\n')
            .map(|i| i + 1)
            .unwrap_or(0);
        let prev_len = prev_end - prev_start;
        self.cursor = prev_start + col.min(prev_len);
    }

    pub fn move_down(&mut self) {
        let (start, end) = self.line_bounds();
        if end >= self.chars.len() {
            return;
        }
        let col = self.cursor - start;
        let next_start = end + 1;
        let next_end = self.chars[next_start..]
            .iter()
            .position(|c| *c == '\n')
            .map(|i| next_start + i)
            .unwrap_or(self.chars.len());
        let next_len = next_end - next_start;
        self.cursor = next_start + col.min(next_len);
    }

    pub fn take(&mut self) -> String {
        let text = self.text();
        self.chars.clear();
        self.cursor = 0;
        self.rev = self.rev.wrapping_add(1);
        text
    }
}

impl Default for LineEditor {
    fn default() -> Self {
        Self::new()
    }
}

fn mode_from(mode: &str) -> AgentMode {
    if mode.eq_ignore_ascii_case("plan") {
        AgentMode::Plan
    } else {
        AgentMode::Build
    }
}

impl App {
    pub fn new() -> Self {
        Self {
            should_quit: false,
            tick: 0,
            cwd: std::env::current_dir()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| "~".to_string()),
            root: std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
            messages: Vec::new(),
            input: LineEditor::new(),
            busy: false,
            iteration: None,
            turn_started: None,
            queued: VecDeque::new(),
            expanded: false,
            mode: AgentMode::Build,
            scroll_from_bottom: 0,
            viewport_height: 10,
            frame_max_scroll: 0,
            backend_label: "agent".to_string(),
            catalog: Catalog::default(),
            maximized: None,
            selectable_area: Rect::default(),
            selection: Selection::default(),
            copied_text: None,
            flash: None,
            palette: None,
            slash: None,
            slash_dismissed: None,
            approval: None,
            remote_tool_seq: 0,
            remote_approval_seq: 0,
            #[cfg(test)]
            remote_log: Vec::new(),
            auto_allow: HashSet::new(),
            session_dialog: None,
            theme_dialog: None,
            remote_dialog: None,
            tunnel_dialog: None,
            tunnel_url_rect: None,
            remote: None,
            remote_server: None,
            last_tunnel_key: None,
            attachments: Vec::new(),
            pasted_text: Vec::new(),
            preview: None,
            pending_kitty: Vec::new(),
            chip_rects: Vec::new(),
            chip_x_rects: Vec::new(),
            wizard: None,
            reload_config: None,
            providers: Vec::new(),
            active_provider: None,
            max_iterations: evrax_agent::DEFAULT_MAX_ITERATIONS,
            config_mock: false,
            model_dialog: None,
            fetch_seq: 0,
            session_id: unique_session_id(),
            sessions: Vec::new(),
            store: None,
            transcripts: std::collections::HashMap::new(),
            titles: std::collections::HashMap::new(),
            manual_titles: std::collections::HashSet::new(),
            title_pending: std::collections::HashSet::new(),
            title_job: None,
            workers: Vec::new(),
            worker_seq: 0,
            worker_spawn_request: None,
            update_explicit: false,
            update_inflight: false,
            transcript: crate::ui::TranscriptCache::default(),
            content_rev: 0,
            last_esc: None,
            asst_open: false,
        }
    }

    /// Loads saved providers + active selection into the app.
    pub fn attach_config(&mut self, loaded: &evrax_agent::LoadedConfig) {
        self.providers = loaded.providers.clone();
        self.active_provider = loaded.active.clone();
        self.max_iterations = loaded.max_iterations;
        self.config_mock = loaded.mock;
        self.root = loaded.config.root.clone();
    }

    fn next_fetch_id(&mut self) -> u64 {
        self.fetch_seq += 1;
        self.fetch_seq
    }

    pub fn open_wizard(&mut self) {
        let existing = !self.providers.is_empty();
        let cursor = self
            .active_provider
            .as_deref()
            .and_then(|name| evrax_agent::provider_index(&self.providers, name))
            .unwrap_or(0);
        self.wizard = Some(Wizard {
            step: if existing {
                WizardStep::Providers
            } else {
                WizardStep::Kind
            },
            cursor,
            editing: false,
            kind: evrax_agent::ProviderKind::Openai,
            name: String::new(),
            base_url: String::new(),
            api_key: String::new(),
            model: String::new(),
            models: Vec::new(),
            filtered: Vec::new(),
            model_cursor: 0,
            filter: String::new(),
            existing: false,
            preset: 0,
            request_id: 0,
            status: None,
        });
    }

    pub fn wizard_step(&self) -> Option<WizardStep> {
        self.wizard.as_ref().map(|wizard| wizard.step)
    }

    pub fn wizard_move(&mut self, delta: i32) {
        let Some(step) = self.wizard.as_ref().map(|wizard| wizard.step) else {
            return;
        };
        if step == WizardStep::Kind {
            self.wizard_kind(delta);
            return;
        }
        let Some(wizard) = self.wizard.as_mut() else {
            return;
        };
        match step {
            WizardStep::Providers => {
                let rows = self.providers.len() + 1;
                wizard.cursor = (wizard.cursor as i32 + delta).rem_euclid(rows as i32) as usize;
            }
            WizardStep::Models => {
                if wizard.filtered.is_empty() {
                    wizard.model_cursor = 0;
                } else {
                    let count = wizard.filtered.len() as i32;
                    wizard.model_cursor =
                        (wizard.model_cursor as i32 + delta).rem_euclid(count) as usize;
                }
            }
            _ => {}
        }
    }

    /// Jumps a page in the wizard list steps (clamped, no wrap).
    pub fn wizard_page(&mut self, delta: i32) {
        let Some(wizard) = self.wizard.as_mut() else {
            return;
        };
        let page = DIALOG_PAGE as i32 * delta;
        match wizard.step {
            WizardStep::Kind => {
                let count = evrax_agent::ProviderKind::ALL.len() as i32;
                let index = (wizard.kind as i32 + page.signum()).rem_euclid(count) as usize;
                wizard.kind = evrax_agent::ProviderKind::ALL[index];
            }
            WizardStep::Providers => {
                let rows = (self.providers.len() + 1) as i32;
                wizard.cursor = (wizard.cursor as i32 + page).clamp(0, rows - 1) as usize;
            }
            WizardStep::Models if !wizard.filtered.is_empty() => {
                let rows = wizard.filtered.len() as i32;
                wizard.model_cursor =
                    (wizard.model_cursor as i32 + page).clamp(0, rows - 1) as usize;
            }
            _ => {}
        }
    }

    /// Jumps a page in the provider/model picker (clamped, no wrap).
    pub fn model_page(&mut self, delta: i32) {
        let Some(dialog) = self.model_dialog.as_mut() else {
            return;
        };
        let page = DIALOG_PAGE as i32 * delta;
        match dialog.level {
            ModelLevel::Providers if !self.providers.is_empty() => {
                let rows = self.providers.len() as i32;
                dialog.provider_cursor =
                    (dialog.provider_cursor as i32 + page).clamp(0, rows - 1) as usize;
            }
            ModelLevel::Models if !dialog.filtered.is_empty() => {
                let rows = dialog.filtered.len() as i32;
                dialog.model_cursor =
                    (dialog.model_cursor as i32 + page).clamp(0, rows - 1) as usize;
            }
            _ => {}
        }
    }

    /// Switches the protocol (kind step).
    pub fn wizard_kind(&mut self, delta: i32) {
        if let Some(wizard) = self.wizard.as_mut()
            && wizard.step == WizardStep::Kind
        {
            let count = evrax_agent::ProviderKind::ALL.len() as i32;
            let index = (wizard.kind as i32 + delta).rem_euclid(count) as usize;
            wizard.kind = evrax_agent::ProviderKind::ALL[index];
        }
    }

    /// Fills base URL / name from the built-in presets (base URL step).
    pub fn wizard_preset(&mut self, delta: i32) {
        let Some(wizard) = self.wizard.as_mut() else {
            return;
        };
        if wizard.step != WizardStep::BaseUrl {
            return;
        }
        let count = WIZARD_PRESETS.len() as i32;
        wizard.preset = (wizard.preset as i32 + delta).rem_euclid(count) as usize;
        let (name, url, _) = WIZARD_PRESETS[wizard.preset];
        wizard.base_url = url.to_string();
        if wizard.name.trim().is_empty() {
            wizard.name = name.to_string();
        }
    }

    fn wizard_load_provider(&mut self, index: usize) {
        let Some(provider) = self.providers.get(index).cloned() else {
            return;
        };
        if let Some(wizard) = self.wizard.as_mut() {
            wizard.kind = provider.kind;
            wizard.name = provider.name;
            wizard.base_url = provider.base_url;
            wizard.api_key = provider.api_key.unwrap_or_default();
            wizard.model = provider.model;
            wizard.models.clear();
            wizard.filtered.clear();
            wizard.filter.clear();
            wizard.existing = true;
            wizard.step = WizardStep::Kind;
            wizard.status = None;
        }
    }

    fn wizard_new_provider(&mut self) {
        if let Some(wizard) = self.wizard.as_mut() {
            wizard.kind = evrax_agent::ProviderKind::Openai;
            wizard.name.clear();
            wizard.base_url.clear();
            wizard.api_key.clear();
            wizard.model.clear();
            wizard.models.clear();
            wizard.filtered.clear();
            wizard.filter.clear();
            wizard.existing = false;
            wizard.step = WizardStep::Kind;
            wizard.status = None;
        }
    }

    pub fn wizard_enter(&mut self) -> WizardAction {
        let Some(step) = self.wizard.as_ref().map(|wizard| wizard.step) else {
            return WizardAction::None;
        };
        if self.wizard.as_ref().is_some_and(|wizard| wizard.editing) {
            if let Some(wizard) = self.wizard.as_mut() {
                wizard.editing = false;
            }
            return WizardAction::None;
        }
        match step {
            WizardStep::Providers => {
                let cursor = self.wizard.as_ref().map(|w| w.cursor).unwrap_or(0);
                if cursor < self.providers.len() {
                    self.wizard_load_provider(cursor);
                } else {
                    self.wizard_new_provider();
                }
                WizardAction::None
            }
            WizardStep::Kind => {
                if let Some(wizard) = self.wizard.as_mut() {
                    wizard.step = WizardStep::Name;
                    wizard.editing = true;
                    wizard.status = None;
                }
                WizardAction::None
            }
            WizardStep::Name => {
                if self
                    .wizard
                    .as_ref()
                    .is_some_and(|wizard| wizard.name.trim().is_empty())
                {
                    self.wizard_status("provider name is required", false);
                    return WizardAction::None;
                }
                if let Some(wizard) = self.wizard.as_mut() {
                    wizard.step = WizardStep::BaseUrl;
                    wizard.editing = true;
                }
                WizardAction::None
            }
            WizardStep::BaseUrl => {
                let valid = self.wizard.as_ref().is_some_and(|wizard| {
                    let url = wizard.base_url.trim().to_lowercase();
                    url.starts_with("http://") || url.starts_with("https://")
                });
                if !valid {
                    self.wizard_status("base URL must start with http:// or https://", false);
                    return WizardAction::None;
                }
                if let Some(wizard) = self.wizard.as_mut() {
                    wizard.step = WizardStep::ApiKey;
                    wizard.editing = true;
                }
                WizardAction::None
            }
            WizardStep::ApiKey => {
                let request_id = self.next_fetch_id();
                let Some(wizard) = self.wizard.as_mut() else {
                    return WizardAction::None;
                };
                wizard.request_id = request_id;
                wizard.step = WizardStep::Checking;
                wizard.editing = false;
                wizard.models.clear();
                wizard.filtered.clear();
                wizard.filter.clear();
                wizard.status = Some("cek API key & ambil model...".to_string());
                WizardAction::FetchModels(FetchRequest {
                    request_id,
                    kind: wizard.kind,
                    base_url: wizard.base_url.trim().to_string(),
                    api_key: Some(wizard.api_key.trim().to_string()).filter(|key| !key.is_empty()),
                })
            }
            WizardStep::Checking => WizardAction::None,
            WizardStep::Models => {
                let Some(wizard) = self.wizard.as_mut() else {
                    return WizardAction::None;
                };
                if let Some(index) = wizard.filtered.get(wizard.model_cursor) {
                    wizard.model = wizard.models[*index].clone();
                    wizard.step = WizardStep::Review;
                } else if !wizard.filter.trim().is_empty() {
                    wizard.model = wizard.filter.trim().to_string();
                    wizard.step = WizardStep::Review;
                } else {
                    wizard.status = Some("type a model name first".to_string());
                }
                WizardAction::None
            }
            WizardStep::Review => match self.wizard_settings() {
                Some(settings) => WizardAction::Save(settings),
                None => {
                    self.wizard_status("fill in the name, base URL, and model", false);
                    WizardAction::None
                }
            },
        }
    }

    /// Settings snapshot for the current wizard form (None when incomplete).
    pub fn wizard_settings(&self) -> Option<evrax_agent::ConfigSettings> {
        let wizard = self.wizard.as_ref()?;
        let name = wizard.name.trim();
        let base_url = wizard.base_url.trim();
        let model = wizard.model.trim();
        if name.is_empty() || base_url.is_empty() || model.is_empty() {
            return None;
        }
        let provider = evrax_agent::Provider {
            name: name.to_string(),
            kind: wizard.kind,
            base_url: base_url.to_string(),
            api_key: Some(wizard.api_key.trim().to_string()).filter(|key| !key.is_empty()),
            model: model.to_string(),
        };
        let mut providers = self.providers.clone();
        match evrax_agent::provider_index(&providers, name) {
            Some(index) => providers[index] = provider,
            None => providers.push(provider),
        }
        Some(evrax_agent::ConfigSettings {
            providers,
            active: name.to_string(),
            max_iterations: self.max_iterations,
            mock: self.config_mock,
        })
    }

    /// Applies a saved settings snapshot and queues a backend swap.
    pub fn apply_settings(&mut self, settings: evrax_agent::ConfigSettings) {
        self.active_provider = Some(settings.active.clone());
        self.providers = settings.providers.clone();
        self.max_iterations = settings.max_iterations;
        self.queue_backend_reload(settings.resolved());
    }

    pub fn wizard_models_ready(&mut self, request_id: u64, result: Result<Vec<String>, String>) {
        let Some(wizard) = self.wizard.as_mut() else {
            return;
        };
        if wizard.request_id != request_id {
            return;
        }
        match result {
            Ok(models) => {
                wizard.models = models;
                wizard.filter.clear();
                wizard.model_cursor = 0;
                wizard.refilter();
                wizard.step = WizardStep::Models;
                wizard.status = if wizard.models.is_empty() {
                    Some("no models; type a model name and press Enter".to_string())
                } else {
                    None
                };
            }
            Err(error) => {
                wizard.models.clear();
                wizard.filtered.clear();
                wizard.filter.clear();
                wizard.model_cursor = 0;
                wizard.step = WizardStep::Models;
                wizard.status = Some(format!("{error} - type the model manually and press Enter"));
            }
        }
    }

    pub fn queue_backend_reload(&mut self, cfg: evrax_agent::AgentConfig) {
        self.reload_config = Some(cfg);
    }

    pub fn take_reload_config(&mut self) -> Option<evrax_agent::AgentConfig> {
        self.reload_config.take()
    }

    /// Esc inside the wizard: closes it (true) or moves one step back (false).
    pub fn wizard_escape(&mut self) -> bool {
        let Some(wizard) = self.wizard.as_mut() else {
            return false;
        };
        if wizard.editing {
            wizard.editing = false;
            return false;
        }
        match wizard.step {
            WizardStep::Providers => true,
            WizardStep::Kind => {
                wizard.step = WizardStep::Providers;
                false
            }
            WizardStep::Name => {
                wizard.step = WizardStep::Kind;
                false
            }
            WizardStep::BaseUrl => {
                wizard.step = WizardStep::Name;
                false
            }
            WizardStep::ApiKey => {
                wizard.step = WizardStep::BaseUrl;
                false
            }
            WizardStep::Models => {
                wizard.step = WizardStep::ApiKey;
                false
            }
            WizardStep::Review => {
                wizard.step = WizardStep::Models;
                false
            }
            WizardStep::Checking => false,
        }
    }

    pub fn wizard_type(&mut self, c: char) {
        let Some(wizard) = self.wizard.as_mut() else {
            return;
        };
        match wizard.step {
            WizardStep::Name => wizard.name.push(c),
            WizardStep::BaseUrl => wizard.base_url.push(c),
            WizardStep::ApiKey => wizard.api_key.push(c),
            WizardStep::Models => {
                wizard.filter.push(c);
                wizard.refilter();
            }
            _ => {}
        }
    }

    pub fn wizard_backspace(&mut self) {
        let Some(wizard) = self.wizard.as_mut() else {
            return;
        };
        match wizard.step {
            WizardStep::Name => {
                wizard.name.pop();
            }
            WizardStep::BaseUrl => {
                wizard.base_url.pop();
            }
            WizardStep::ApiKey => {
                wizard.api_key.pop();
            }
            WizardStep::Models => {
                wizard.filter.pop();
                wizard.refilter();
            }
            _ => {}
        }
    }

    pub fn wizard_status(&mut self, status: impl Into<String>, _busy: bool) {
        if let Some(wizard) = self.wizard.as_mut() {
            wizard.status = Some(status.into());
        }
    }

    pub fn wizard_close(&mut self) {
        self.wizard = None;
    }

    pub fn open_model_dialog(&mut self) {
        if self.providers.is_empty() {
            self.set_flash("no providers yet - run evrax --setup".to_string());
            return;
        }
        let cursor = self
            .active_provider
            .as_deref()
            .and_then(|name| evrax_agent::provider_index(&self.providers, name))
            .unwrap_or(0);
        self.model_dialog = Some(ModelDialog {
            level: ModelLevel::Providers,
            provider_cursor: cursor,
            model_cursor: 0,
            models: Vec::new(),
            filtered: Vec::new(),
            filter: String::new(),
            request_id: 0,
            loading: false,
            status: None,
        });
    }

    pub fn model_dialog_open(&self) -> bool {
        self.model_dialog.is_some()
    }

    pub fn model_move(&mut self, delta: i32) {
        let Some(dialog) = self.model_dialog.as_mut() else {
            return;
        };
        match dialog.level {
            ModelLevel::Providers => {
                let count = self.providers.len() as i32;
                dialog.provider_cursor =
                    (dialog.provider_cursor as i32 + delta).rem_euclid(count) as usize;
            }
            ModelLevel::Models => {
                if !dialog.filtered.is_empty() {
                    let count = dialog.filtered.len() as i32;
                    dialog.model_cursor =
                        (dialog.model_cursor as i32 + delta).rem_euclid(count) as usize;
                }
            }
        }
    }

    pub fn model_type(&mut self, c: char) {
        if let Some(dialog) = self.model_dialog.as_mut()
            && dialog.level == ModelLevel::Models
        {
            dialog.filter.push(c);
            dialog.refilter();
        }
    }

    pub fn model_backspace(&mut self) {
        if let Some(dialog) = self.model_dialog.as_mut()
            && dialog.level == ModelLevel::Models
        {
            dialog.filter.pop();
            dialog.refilter();
        }
    }

    pub fn model_enter(&mut self) -> ModelAction {
        let Some(dialog) = self.model_dialog.as_ref() else {
            return ModelAction::None;
        };
        match dialog.level {
            ModelLevel::Providers => {
                let Some(provider) = self.providers.get(dialog.provider_cursor).cloned() else {
                    return ModelAction::None;
                };
                let request_id = self.next_fetch_id();
                if let Some(dialog) = self.model_dialog.as_mut() {
                    dialog.level = ModelLevel::Models;
                    dialog.loading = true;
                    dialog.request_id = request_id;
                    dialog.models.clear();
                    dialog.filtered.clear();
                    dialog.filter.clear();
                    dialog.model_cursor = 0;
                    dialog.status = Some("loading models...".to_string());
                }
                ModelAction::FetchModels(FetchRequest {
                    request_id,
                    kind: provider.kind,
                    base_url: provider.base_url.clone(),
                    api_key: provider.clean_key().map(str::to_string),
                })
            }
            ModelLevel::Models => {
                if dialog.loading {
                    return ModelAction::None;
                }
                let chosen = dialog
                    .filtered
                    .get(dialog.model_cursor)
                    .and_then(|index| dialog.models.get(*index))
                    .cloned()
                    .or_else(|| {
                        (!dialog.filter.trim().is_empty()).then(|| dialog.filter.trim().to_string())
                    });
                let index = dialog.provider_cursor;
                let Some(model) = chosen else {
                    if let Some(dialog) = self.model_dialog.as_mut() {
                        dialog.status =
                            Some("type a model name or pick one from the list".to_string());
                    }
                    return ModelAction::None;
                };
                let mut providers = self.providers.clone();
                let Some(provider) = providers.get_mut(index) else {
                    return ModelAction::None;
                };
                provider.model = model;
                let active = provider.name.clone();
                let settings = evrax_agent::ConfigSettings {
                    providers,
                    active,
                    max_iterations: self.max_iterations,
                    mock: self.config_mock,
                };
                ModelAction::Apply(settings)
            }
        }
    }

    pub fn model_ready(&mut self, request_id: u64, result: Result<Vec<String>, String>) {
        let Some(dialog) = self.model_dialog.as_mut() else {
            return;
        };
        if dialog.request_id != request_id {
            return;
        }
        dialog.loading = false;
        match result {
            Ok(models) => {
                dialog.models = models;
                dialog.filter.clear();
                dialog.model_cursor = 0;
                dialog.refilter();
                dialog.status = if dialog.models.is_empty() {
                    Some("no models; type a model name and press Enter".to_string())
                } else {
                    None
                };
            }
            Err(error) => {
                dialog.models.clear();
                dialog.filtered.clear();
                dialog.filter.clear();
                dialog.status = Some(format!("{error} - type the model manually and press Enter"));
            }
        }
    }

    pub fn model_escape(&mut self) -> bool {
        let Some(dialog) = self.model_dialog.as_mut() else {
            return true;
        };
        match dialog.level {
            ModelLevel::Providers => true,
            ModelLevel::Models => {
                dialog.level = ModelLevel::Providers;
                dialog.models.clear();
                dialog.filtered.clear();
                dialog.filter.clear();
                dialog.status = None;
                dialog.loading = false;
                false
            }
        }
    }

    pub fn model_close(&mut self) {
        self.model_dialog = None;
    }

    /// Wires a project store: loads the session list + titles from disk.
    pub fn attach_project(&mut self, store: ProjectStore) {
        if let Ok(metas) = store.list() {
            self.sessions = metas.iter().map(|m| m.id.clone()).collect();
            for meta in &metas {
                if !meta.title.is_empty() {
                    self.titles.insert(meta.id.clone(), meta.title.clone());
                }
                if meta.title_pending {
                    self.title_pending.insert(meta.id.clone());
                }
            }
        }
        self.store = Some(store);
    }

    /// Loads a persisted session (metadata + transcript). Returns false when unknown.
    pub fn resume(&mut self, id: &str) -> bool {
        let Some(store) = self.store.clone() else {
            return false;
        };
        let Ok(Some(meta)) = store.load_meta(id) else {
            return false;
        };
        if self.session_id != id && !self.messages.is_empty() {
            self.save_current();
        }
        self.session_id = id.to_string();
        self.messages = store
            .read_json(id, "transcript.json")
            .ok()
            .flatten()
            .unwrap_or_default();
        self.mode = mode_from(&meta.mode);
        if !meta.title.is_empty() {
            self.titles.insert(id.to_string(), meta.title.clone());
            self.title_pending.insert(id.to_string());
        }
        if !self.sessions.contains(&id.to_string()) {
            self.sessions.insert(0, id.to_string());
        }
        self.input.take();
        self.attachments.clear();
        self.pasted_text.clear();
        self.asst_open = false;
        self.busy = false;
        self.turn_started = None;
        self.queued.clear();
        self.scroll_from_bottom = 0;
        self.transcript.invalidate();
        self.restore_worker_panes(id);
        true
    }

    /// Flushes the active session (transcript + metadata) to disk, if any.
    pub fn save_current(&mut self) {
        if self.messages.is_empty() {
            return;
        }
        let Some(store) = self.store.clone() else {
            return;
        };
        let id = self.session_id.clone();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let created = store
            .load_meta(&id)
            .ok()
            .flatten()
            .map(|m| m.created_at)
            .filter(|c| *c > 0)
            .unwrap_or(now);
        let meta = SessionMeta {
            id: id.clone(),
            title: self.titles.get(&id).cloned().unwrap_or_default(),
            mode: self.mode.label().to_lowercase(),
            title_pending: self.title_pending.contains(&id) || self.titles.contains_key(&id),
            created_at: created,
            updated_at: now,
        };
        let _ = store.write_json(&id, "transcript.json", &self.messages);
        let _ = store.save_meta(&meta);
        if !self.sessions.contains(&id) {
            self.sessions.insert(0, id);
        }
    }

    fn load_from_disk(&self, id: &str) -> Option<Vec<ChatEntry>> {
        self.store
            .as_ref()?
            .read_json(id, "transcript.json")
            .ok()
            .flatten()
    }

    fn restore_mode(&mut self, id: &str) {
        if let Some(store) = self.store.as_ref()
            && let Ok(Some(meta)) = store.load_meta(id)
        {
            self.mode = mode_from(&meta.mode);
        }
    }

    pub fn tick(&mut self) {
        self.tick = self.tick.wrapping_add(1);
        if let Some((_, until)) = &self.flash
            && self.tick >= *until
        {
            self.flash = None;
        }
        // Surface tunnel lifecycle changes (URL ready / failure) as a flash.
        let status = self.remote.as_ref().map(|s| s.tunnel.status());
        let key = status.as_ref().map(tunnel_status_key);
        if key.is_some() && key != self.last_tunnel_key {
            match status {
                Some(evrax_remote::tunnel::TunnelStatus::Running { url }) => {
                    self.set_flash(format!("tunnel live · {url}"));
                }
                Some(evrax_remote::tunnel::TunnelStatus::Error { message }) => {
                    self.set_flash(format!("tunnel error · {message}"));
                }
                _ => {}
            }
        }
        if key.is_some() {
            self.last_tunnel_key = key;
        }
    }

    pub fn set_flash(&mut self, text: String) {
        self.flash = Some((text, self.tick + 10));
    }

    /// Name shown in the sidebar, session dialog and web: the title if one
    /// exists, else a generic label (raw ids are noise).
    pub fn display_name(&self) -> String {
        self.titles
            .get(&self.session_id)
            .cloned()
            .unwrap_or_else(|| "new session".to_string())
    }

    /// Title for any known session id (falls back to the id itself).
    pub fn title_of(&self, id: &str) -> String {
        self.titles
            .get(id)
            .cloned()
            .unwrap_or_else(|| id.to_string())
    }

    /// Sets a session title and mirrors it to the remote registry, if on.
    fn set_title(&mut self, id: &str, title: String, manual: bool) {
        if title.is_empty() {
            return;
        }
        self.titles.insert(id.to_string(), title.clone());
        if manual {
            self.manual_titles.insert(id.to_string());
        }
        if id == self.session_id
            && self.remote.as_ref().is_some_and(|r| r.enabled)
            && let Some(server) = &self.remote_server
        {
            let turn_id = self
                .remote
                .as_ref()
                .map(|r| r.session_id.clone())
                .unwrap_or_default();
            evrax_remote::rename_session(server, &turn_id, title);
        }
        self.save_current();
    }

    /// `/rename <title>` — a manual title survives later model suggestions.
    pub fn rename_session_title(&mut self, arg: &str) {
        let title: String = arg.split_whitespace().collect::<Vec<_>>().join(" ");
        let title: String = title.chars().take(60).collect();
        if title.is_empty() {
            self.set_flash("usage: /rename <title>".to_string());
            return;
        }
        let id = self.session_id.clone();
        self.set_title(&id, title, true);
        self.set_flash("session renamed".to_string());
    }

    /// LLM title job queued for the event loop (it owns tokio spawning).
    pub fn take_title_job(&mut self) -> Option<(String, String, String)> {
        self.title_job.take()
    }

    /// Explicit `/update` request: reports its outcome either way.
    pub fn request_update_check(&mut self) {
        self.update_explicit = true;
        if !self.update_inflight {
            self.set_flash("checking for updates\u{2026}".to_string());
        }
    }

    /// Startup probe: silent unless a newer release exists.
    pub fn request_startup_update_check(&mut self) {
        self.update_explicit = false;
    }

    /// Hands the event loop one update job; `None` while a check is in flight.
    pub fn update_job(&mut self) -> Option<bool> {
        if self.update_inflight {
            return None;
        }
        self.update_inflight = true;
        Some(self.update_explicit)
    }

    /// Applies a finished check. Explicit requests always flash; the startup
    /// probe stays quiet unless an update is available.
    pub fn apply_update_check(
        &mut self,
        explicit: bool,
        result: Result<Option<evrax_agent::update::UpdateCheck>, String>,
    ) {
        self.update_inflight = false;
        let current = env!("CARGO_PKG_VERSION");
        let message = match result {
            Ok(Some(check)) => {
                if explicit {
                    format!(
                        "update available: {} (you: {}) \u{00b7} {}",
                        check.latest, current, check.url
                    )
                } else {
                    format!(
                        "update available: {} \u{2014} /update for details",
                        check.latest
                    )
                }
            }
            Ok(None) => {
                if explicit {
                    format!("up to date: {current}")
                } else {
                    return;
                }
            }
            Err(err) => {
                if explicit {
                    format!("update check failed: {err}")
                } else {
                    return;
                }
            }
        };
        self.set_flash(message);
    }

    fn plan_title_job(&mut self) {
        let id = self.session_id.clone();
        if self.manual_titles.contains(&id) || self.title_pending.contains(&id) {
            return;
        }
        let Some(prompt) = self.messages.iter().find_map(|entry| match entry {
            ChatEntry::User(text) if !text.trim().is_empty() => Some(text.clone()),
            _ => None,
        }) else {
            return;
        };
        let Some(reply) = self.last_assistant_text() else {
            return;
        };
        self.title_pending.insert(id.clone());
        self.title_job = Some((id, prompt, reply));
    }

    fn ensure_placeholder_title(&mut self, prompt: &str) {
        if self.titles.contains_key(&self.session_id) {
            return;
        }
        let Some(title) = title_from_prompt(prompt) else {
            return;
        };
        let id = self.session_id.clone();
        self.set_title(&id, title, false);
    }

    /// Push one transcript entry, capping how much a single session keeps in
    /// memory. Dropped entries only leave the display; the backend history is
    /// trimmed independently by character budget.
    fn push_entry(&mut self, entry: ChatEntry) {
        self.messages.push(entry);
        if self.messages.len() > MAX_ENTRIES_PER_SESSION + 80 {
            let drop = self.messages.len() - MAX_ENTRIES_PER_SESSION;
            self.messages.drain(..drop);
        }
        self.content_rev = self.content_rev.wrapping_add(1);
    }

    /// Store a session's transcript, evicting the least recently used ones
    /// beyond the retention cap.
    fn remember_transcript(&mut self, id: String, messages: Vec<ChatEntry>) {
        self.transcripts.insert(id, messages);
        while self.transcripts.len() > MAX_RETAINED_SESSIONS {
            let victim = self
                .sessions
                .iter()
                .rev()
                .find(|name| **name != self.session_id && self.transcripts.contains_key(*name))
                .cloned();
            match victim {
                Some(victim) => {
                    if self.store.is_some() {
                        self.transcripts.remove(&victim);
                    } else {
                        self.forget_session(&victim);
                    }
                }
                None => break,
            }
        }
    }

    fn forget_session(&mut self, id: &str) {
        self.transcripts.remove(id);
        self.titles.remove(id);
        self.manual_titles.remove(id);
        self.title_pending.remove(id);
        self.sessions.retain(|name| name != id);
    }

    pub fn quit(&mut self) {
        self.clear_workers();
        self.save_current();
        self.should_quit = true;
    }

    pub fn cycle_mode(&mut self) {
        self.mode = self.mode.toggled();
        self.set_flash(format!("mode · {}", self.mode.label().to_lowercase()));
    }

    pub fn toggle_palette(&mut self) {
        self.session_dialog = None;
        self.theme_dialog = None;
        self.palette = match self.palette {
            Some(_) => None,
            None => Some(Palette {
                query: String::new(),
                selected: 0,
            }),
        };
    }

    pub fn palette_close(&mut self) {
        self.palette = None;
    }

    pub fn palette_type(&mut self, c: char) {
        if let Some(p) = &mut self.palette {
            p.query.push(c);
            p.selected = 0;
        }
    }

    pub fn palette_backspace(&mut self) {
        if let Some(p) = &mut self.palette {
            p.query.pop();
            p.selected = 0;
        }
    }

    pub fn palette_move(&mut self, delta: i16) {
        let len = self.filtered_commands().len();
        if let Some(p) = &mut self.palette
            && len > 0
        {
            p.selected = (p.selected as i16 + delta).rem_euclid(len as i16) as usize;
        }
    }

    pub fn palette_page(&mut self, delta: i32, visible: usize) {
        let len = self.filtered_commands().len();
        if len == 0 || visible == 0 {
            return;
        }
        if let Some(p) = &mut self.palette {
            let v = visible.min(len) as i32;
            let step = delta.signum() * v;
            let next = (p.selected as i32 + step).clamp(0, len as i32 - 1);
            p.selected = next as usize;
        }
    }

    pub fn filtered_commands(&self) -> Vec<(String, Command)> {
        let entries = self.command_entries();
        let Some(p) = &self.palette else {
            return entries;
        };
        if p.query.is_empty() {
            return entries;
        }
        let mut matcher = Matcher::new(Config::DEFAULT);
        let pattern = Pattern::parse(&p.query, CaseMatching::Smart, Normalization::Smart);
        let mut buf = Vec::new();
        let mut scored: Vec<(u32, (String, Command))> = entries
            .into_iter()
            .filter_map(|entry| {
                let score = pattern.score(Utf32Str::new(&entry.0, &mut buf), &mut matcher)?;
                Some((score, entry))
            })
            .collect();
        scored.sort_by_key(|entry| std::cmp::Reverse(entry.0));
        scored.into_iter().map(|(_, entry)| entry).collect()
    }

    fn command_entries(&self) -> Vec<(String, Command)> {
        vec![
            (
                format!(
                    "Switch to {} mode",
                    self.mode.toggled().label().to_lowercase()
                ),
                Command::ToggleMode,
            ),
            ("Session list".to_string(), Command::Sessions),
            ("Copy last reply".to_string(), Command::CopyReply),
            ("New session".to_string(), Command::NewSession),
            ("Change theme".to_string(), Command::Theme),
            ("Provider and model".to_string(), Command::Model),
            ("Remote web".to_string(), Command::Remote),
            ("Check for updates".to_string(), Command::Update),
            ("Spawn workers".to_string(), Command::Worker),
            ("Workers (list/clear/kill)".to_string(), Command::Workers),
            ("Quit evrax".to_string(), Command::Quit),
        ]
    }

    pub fn palette_confirm(&mut self) {
        let Some(selected) = self.palette.as_ref().map(|p| p.selected) else {
            return;
        };
        let Some((_, command)) = self.filtered_commands().into_iter().nth(selected) else {
            self.palette = None;
            return;
        };
        self.palette = None;
        self.execute_command(command);
    }

    fn execute_command(&mut self, command: Command) {
        // Read-only commands keep working while a turn streams; the ones that
        // swap the session out from under the running turn do not.
        if self.busy && matches!(command, Command::NewSession | Command::Rename) {
            self.set_flash("not available while streaming".to_string());
            return;
        }
        match command {
            Command::ToggleMode => self.cycle_mode(),
            Command::Sessions => self.open_sessions(),
            Command::CopyReply => match self.last_assistant_text() {
                Some(text) => {
                    let chars = text.chars().count();
                    self.copied_text = Some(text);
                    self.set_flash(format!("copied reply · {chars} chars"));
                }
                None => self.set_flash("nothing to copy".to_string()),
            },
            Command::NewSession => self.new_session(),
            Command::Theme => self.open_theme_dialog(),
            Command::Model => self.open_model_dialog(),
            Command::Remote => self.open_remote_dialog(),
            Command::Rename => self.rename_session_title(""),
            Command::Update => self.request_update_check(),
            Command::Worker => self.spawn_workers(1),
            Command::Workers => self.workers_command(""),
            Command::Quit => self.quit(),
        }
    }

    pub fn slash_query(&self) -> Option<String> {
        let text = self.input.text();
        let rest = text.strip_prefix('/')?;
        if rest.chars().any(|c| c.is_whitespace() || c == '/') {
            return None;
        }
        Some(rest.to_string())
    }

    pub fn filtered_slash(&self) -> Vec<SlashOption> {
        let options = slash_options();
        let Some(query) = self.slash_query() else {
            return Vec::new();
        };
        if query.is_empty() {
            return options;
        }
        let mut matcher = Matcher::new(Config::DEFAULT);
        let pattern = Pattern::parse(&query, CaseMatching::Smart, Normalization::Smart);
        let mut buf = Vec::new();
        let mut scored: Vec<((bool, u32), SlashOption)> = options
            .into_iter()
            .filter_map(|option| {
                let name = pattern.score(Utf32Str::new(option.0, &mut buf), &mut matcher);
                let description = pattern.score(Utf32Str::new(option.1, &mut buf), &mut matcher);
                let (rank, score) = match (name, description) {
                    (Some(score), extra) => (true, score.max(extra.unwrap_or(0))),
                    (None, Some(score)) => (false, score),
                    (None, None) => return None,
                };
                Some(((rank, score), option))
            })
            .collect();
        scored.sort_by_key(|entry| std::cmp::Reverse(entry.0));
        scored.into_iter().map(|(_, option)| option).collect()
    }

    pub fn slash_sync(&mut self) {
        let query = self.slash_query();
        let current = self.input.text();
        let dismissed = self.slash_dismissed.as_deref() == Some(current.as_str());
        // Slash stays open while streaming: the read-only commands keep
        // working, and mutating ones are refused with a flash (see
        // `execute_command`).
        let open = query.is_some() && !dismissed;
        if !open {
            if query.is_none() {
                self.slash_dismissed = None;
            }
            self.slash = None;
            return;
        }
        let len = self.filtered_slash().len();
        let selected = self
            .slash
            .as_ref()
            .map(|menu| menu.selected.min(len.saturating_sub(1)))
            .unwrap_or(0);
        self.slash = Some(SlashMenu { selected });
    }

    pub fn slash_move(&mut self, delta: i16) {
        let len = self.filtered_slash().len();
        if let Some(menu) = &mut self.slash
            && len > 0
        {
            menu.selected = (menu.selected as i16 + delta).rem_euclid(len as i16) as usize;
        }
    }

    pub fn slash_page(&mut self, delta: i32, visible: usize) {
        let len = self.filtered_slash().len();
        if len == 0 || visible == 0 {
            return;
        }
        if let Some(menu) = &mut self.slash {
            let v = visible.min(len) as i32;
            let step = delta.signum() * v;
            let next = (menu.selected as i32 + step).clamp(0, len as i32 - 1);
            menu.selected = next as usize;
        }
    }

    pub fn slash_dismiss(&mut self) {
        self.slash_dismissed = Some(self.input.text());
        self.slash = None;
    }

    pub fn slash_confirm(&mut self) -> bool {
        let Some(selected) = self.slash.as_ref().map(|menu| menu.selected) else {
            return false;
        };
        let Some((_, _, command)) = self.filtered_slash().get(selected).copied() else {
            return false;
        };
        self.slash = None;
        self.slash_dismissed = None;
        self.input.take();
        self.execute_command(command);
        true
    }

    pub fn new_session(&mut self) {
        self.save_current();
        self.palette = None;
        self.slash = None;
        self.slash_dismissed = None;
        self.session_dialog = None;
        self.theme_dialog = None;
        let old = std::mem::replace(&mut self.session_id, unique_session_id());
        let archived = std::mem::take(&mut self.messages);
        self.remember_transcript(old.clone(), archived);
        if !self.sessions.contains(&old) {
            self.sessions.insert(0, old);
        }
        self.input.take();
        self.attachments.clear();
        self.pasted_text.clear();
        self.asst_open = false;
        self.busy = false;
        self.turn_started = None;
        self.queued.clear();
        self.scroll_from_bottom = 0;
        self.selection.clear();
        self.transcript.invalidate();
        self.clear_workers();
        self.set_flash("new session".to_string());
    }

    pub fn open_sessions(&mut self) {
        self.palette = None;
        self.slash = None;
        self.slash_dismissed = None;
        self.theme_dialog = None;
        self.session_dialog = Some(SessionDialog { selected: 0 });
    }

    pub fn sessions_close(&mut self) {
        self.session_dialog = None;
    }

    pub fn session_rows(&self) -> Vec<String> {
        let mut rows = vec!["ï¼‹ new session".to_string()];
        rows.push(format!("{} (current)", self.display_name()));
        rows.extend(
            self.sessions
                .iter()
                .map(|id| self.title_of(id))
                .collect::<Vec<String>>(),
        );
        rows
    }

    pub fn sessions_move(&mut self, delta: i16) {
        let len = self.session_rows().len();
        if let Some(dialog) = &mut self.session_dialog
            && len > 0
        {
            dialog.selected = (dialog.selected as i16 + delta).rem_euclid(len as i16) as usize;
        }
    }

    pub fn sessions_page(&mut self, delta: i32, visible: usize) {
        let len = self.session_rows().len();
        if len == 0 || visible == 0 {
            return;
        }
        if let Some(dialog) = &mut self.session_dialog {
            let v = visible.min(len) as i32;
            let step = delta.signum() * v;
            let next = (dialog.selected as i32 + step).clamp(0, len as i32 - 1);
            dialog.selected = next as usize;
        }
    }

    pub fn sessions_confirm(&mut self) {
        let Some(dialog) = self.session_dialog.take() else {
            return;
        };
        match dialog.selected {
            0 => self.new_session(),
            1 => self.set_flash("already on this session".to_string()),
            idx => {
                let target = idx - 2;
                if target < self.sessions.len() {
                    let target_id = self.sessions.remove(target);
                    self.save_current();
                    let old = std::mem::replace(&mut self.session_id, target_id.clone());
                    self.sessions.insert(0, old.clone());
                    let archived = std::mem::take(&mut self.messages);
                    self.remember_transcript(old, archived);
                    self.messages = self
                        .transcripts
                        .remove(&target_id)
                        .or_else(|| self.load_from_disk(&target_id))
                        .unwrap_or_default();
                    self.restore_mode(&target_id);
                    self.input.take();
                    self.asst_open = false;
                    self.scroll_from_bottom = 0;
                    self.selection.clear();
                    self.transcript.invalidate();
                    self.set_flash(format!("session · {}", self.display_name()));
                }
            }
        }
    }

    pub fn open_theme_dialog(&mut self) {
        self.palette = None;
        self.slash = None;
        self.slash_dismissed = None;
        self.theme_dialog = Some(ThemeDialog {
            selected: theme::index(),
            original: theme::index(),
        });
    }

    pub fn theme_move(&mut self, delta: i16) {
        let len = theme::count() as i16;
        if let Some(dialog) = &mut self.theme_dialog {
            dialog.selected = (dialog.selected as i16 + delta).rem_euclid(len) as usize;
            theme::set_index(dialog.selected);
        }
    }

    pub fn theme_page(&mut self, delta: i32, visible: usize) {
        let total = theme::count();
        if total == 0 || visible == 0 {
            return;
        }
        if let Some(dialog) = &mut self.theme_dialog {
            let v = visible.min(total) as i32;
            let step = delta.signum() * v;
            let next = (dialog.selected as i32 + step).clamp(0, total as i32 - 1);
            dialog.selected = next as usize;
            theme::set_index(dialog.selected);
        }
    }

    pub fn theme_confirm(&mut self) {
        let Some(dialog) = self.theme_dialog.take() else {
            return;
        };
        theme::set_index(dialog.selected);
        theme::save_current();
        self.set_flash(format!("theme · {}", theme::label()));
    }

    pub fn theme_close(&mut self) {
        let Some(dialog) = self.theme_dialog.take() else {
            return;
        };
        theme::set_index(dialog.original);
    }

    fn ensure_remote(&mut self) {
        if self.remote.is_none() {
            let pid = std::process::id();
            self.remote = Some(RemoteState {
                enabled: false,
                permit: evrax_remote::Permit::Read,
                session_id: format!("session_evrax_{pid}_{}", s_now()),
                config: evrax_remote::load_remote_config(),
                tunnel: evrax_remote::tunnel::TunnelManager::new(),
            });
        }
    }

    pub fn open_remote_dialog(&mut self) {
        self.palette = None;
        self.slash = None;
        self.slash_dismissed = None;
        self.ensure_remote();
        self.remote_dialog = Some(RemoteDialog {
            selected: 0,
            editing: None,
        });
    }

    /// Number of rows the remote dialog shows: enabled, permit, password, bind,
    /// port, url, tunnel, and session.
    pub fn remote_row_count(&self) -> usize {
        8
    }

    /// Rows that only display state; they are edited in
    /// `%APPDATA%\evrax\remote.json` instead of in the dialog.
    pub fn remote_row_is_info(&self, row: usize) -> bool {
        matches!(row, 3 | 4 | 5 | 7)
    }

    /// URL shown in the remote dialog. Wildcard binds (`0.0.0.0`) are replaced
    /// by this machine's LAN IPv4 so the row is directly usable from a phone.
    pub fn remote_display_url(&self) -> String {
        let Some(state) = &self.remote else {
            return String::new();
        };
        let bind = state.config.bind.trim();
        let host = if matches!(bind, "" | "0.0.0.0" | "::" | "[::]") {
            local_ipv4().unwrap_or_else(|| "127.0.0.1".to_string())
        } else {
            bind.to_string()
        };
        format!("http://{host}:{}", state.config.port)
    }

    pub fn remote_move(&mut self, delta: i16) {
        let Some(selected) = self.remote_dialog.as_ref().map(|d| d.selected) else {
            return;
        };
        let rows = self.remote_row_count().max(1);
        let step = if delta < 0 { rows - 1 } else { 1 };
        let mut idx = selected;
        // Info rows are display-only: the cursor jumps over them.
        for _ in 0..rows {
            idx = (idx + step) % rows;
            if !self.remote_row_is_info(idx) {
                break;
            }
        }
        if let Some(dialog) = &mut self.remote_dialog {
            dialog.selected = idx;
        }
    }

    pub fn remote_toggle(&mut self) {
        let Some(selected) = self.remote_dialog.as_ref().map(|d| d.selected) else {
            return;
        };
        match selected {
            // Enable/disable remote for this session.
            0 => {
                let enabling = self.remote.as_ref().is_some_and(|s| !s.enabled);
                let weak = self
                    .remote
                    .as_ref()
                    .map(|s| evrax_remote::is_weak_password(&s.config.password))
                    .unwrap_or(true);
                if enabling && weak {
                    self.set_flash(
                        "set a password first (min 8) before enabling remote".to_string(),
                    );
                    return;
                }
                if let Some(state) = self.remote.as_mut() {
                    state.enabled = !state.enabled;
                }
                self.apply_remote_registration();
            }
            // Cycle permit read -> write.
            1 => {
                let mut enabled = false;
                if let Some(state) = self.remote.as_mut() {
                    state.permit = match state.permit {
                        evrax_remote::Permit::Read => evrax_remote::Permit::Write,
                        evrax_remote::Permit::Write => evrax_remote::Permit::Read,
                    };
                    enabled = state.enabled;
                }
                if enabled {
                    self.apply_remote_registration();
                }
            }
            // Password row: start inline editing.
            2 => self.remote_edit_start(),
            // Tunnel row: provider menu.
            6 => self.open_tunnel_dialog(),
            // Everything else is informational; point at the config file.
            _ => self.set_flash("read-only · edit %APPDATA%\\evrax\\remote.json".to_string()),
        }
    }

    /// Begin editing the password inline. Starts from an empty buffer so typing
    /// always replaces the password instead of appending to the old value.
    pub fn remote_edit_start(&mut self) {
        if let Some(dialog) = self.remote_dialog.as_mut() {
            dialog.editing = Some(String::new());
        }
    }

    pub fn remote_edit_input(&mut self, c: char) {
        if let Some(dialog) = self.remote_dialog.as_mut()
            && let Some(buf) = dialog.editing.as_mut()
        {
            buf.push(c);
        }
    }

    pub fn remote_edit_backspace(&mut self) {
        if let Some(dialog) = self.remote_dialog.as_mut()
            && let Some(buf) = dialog.editing.as_mut()
        {
            buf.pop();
        }
    }

    pub fn remote_edit_cancel(&mut self) {
        if let Some(dialog) = self.remote_dialog.as_mut() {
            dialog.editing = None;
        }
    }

    /// Finish editing: validate, store into state, and return the new config so
    /// the caller can persist + push it to a running server. On error the edit
    /// buffer is kept so the user can fix it.
    pub fn remote_edit_commit(&mut self) -> Result<evrax_remote::RemoteConfig, String> {
        let buf = match self.remote_dialog.as_ref().and_then(|d| d.editing.clone()) {
            Some(b) => b,
            None => return Err("not editing".into()),
        };
        if buf.is_empty() {
            // Enter on an untouched field: treated as "no change" and stops editing.
            if let Some(dialog) = self.remote_dialog.as_mut() {
                dialog.editing = None;
            }
            return Err("password unchanged".into());
        }
        if buf.trim().is_empty() || buf == "12345678" {
            return Err("choose a different password (the default is not allowed)".into());
        }
        if buf.chars().count() < REMOTE_MIN_PASSWORD {
            return Err(format!(
                "password must be at least {REMOTE_MIN_PASSWORD} characters"
            ));
        }
        let Some(state) = self.remote.as_mut() else {
            return Err("remote unavailable".into());
        };
        state.config.password = buf;
        let cfg = state.config.clone();
        if let Some(dialog) = self.remote_dialog.as_mut() {
            dialog.editing = None;
        }
        Ok(cfg)
    }

    pub fn remote_close(&mut self) {
        if self.remote_dialog.take().is_some() {
            self.apply_remote_registration();
        }
    }

    // --- Tunnel: provider menu, quick/domain process, manual URL ----------

    /// Menu rows for the tunnel dialog.
    pub fn tunnel_rows(&self) -> Vec<String> {
        let weak = self
            .remote
            .as_ref()
            .map(|s| evrax_remote::is_weak_password(&s.config.password))
            .unwrap_or(true);
        let note = if weak { " · password required" } else { "" };
        vec![
            "stop tunnel".to_string(),
            format!("quick tunnel (trycloudflare.com){note}"),
            format!("custom domain…{note}"),
            "manual url…".to_string(),
            "copy url".to_string(),
        ]
    }

    pub fn open_tunnel_dialog(&mut self) {
        self.tunnel_dialog = Some(TunnelDialog {
            selected: 0,
            editing: None,
            kind: TunnelEditKind::ManualUrl,
        });
        self.tunnel_url_rect = None;
    }

    pub fn tunnel_close(&mut self) {
        self.tunnel_dialog = None;
        self.tunnel_url_rect = None;
    }

    pub fn tunnel_move(&mut self, delta: i16) {
        let len = self.tunnel_rows().len().max(1) as i16;
        if let Some(dialog) = &mut self.tunnel_dialog {
            dialog.selected = (dialog.selected as i16 + delta).rem_euclid(len) as usize;
        }
    }

    /// One-line tunnel status for the dialogs: provider, progress and URL.
    pub fn tunnel_label(&self) -> String {
        use evrax_remote::tunnel::TunnelStatus;
        let Some(state) = &self.remote else {
            return "off".to_string();
        };
        let short = match state.config.tunnel_mode.as_str() {
            "quick" => "quick",
            "domain" => "domain",
            "tailscale" => "tailscale",
            "manual" => "manual",
            _ => "tunnel",
        };
        let label = match state.tunnel.status() {
            TunnelStatus::Off => {
                if state.config.tunnel_mode == "manual" && !state.config.public_url.is_empty() {
                    format!("manual · {}", state.config.public_url)
                } else {
                    "off".to_string()
                }
            }
            TunnelStatus::Downloading => "downloading cloudflared…".to_string(),
            TunnelStatus::Starting => format!("{short} · starting…"),
            TunnelStatus::Login { url } => format!("login cloudflare · {url}"),
            TunnelStatus::Running { url } => format!("{short} · {url}"),
            TunnelStatus::Error { message } => format!("error · {message}"),
        };
        label.chars().take(52).collect()
    }

    /// Current tunnel URL: the live process URL or the recorded manual one.
    pub fn tunnel_current_url(&self) -> Option<String> {
        let state = self.remote.as_ref()?;
        if let Some(url) = state.tunnel.url() {
            return Some(url);
        }
        if !state.config.public_url.is_empty() {
            return Some(state.config.public_url.clone());
        }
        None
    }

    /// Run the selected tunnel-dialog action; the caller executes the effect.
    pub fn tunnel_confirm(&mut self) -> TunnelEffect {
        let Some(selected) = self.tunnel_dialog.as_ref().map(|d| d.selected) else {
            return TunnelEffect::None;
        };
        match selected {
            // Stop the process (manual mode also clears the recorded URL).
            0 => {
                self.tunnel_stop();
                TunnelEffect::None
            }
            1 => self.tunnel_select_quick(),
            // Custom domain: hostname editor, then login/create/route/run.
            2 => {
                self.tunnel_host_edit_start();
                TunnelEffect::None
            }
            // Manual URL: inline editor.
            3 => {
                self.tunnel_edit_start();
                TunnelEffect::None
            }
            // Copy the current URL (executed by the caller).
            _ => match self.tunnel_current_url() {
                Some(url) => TunnelEffect::Copy(url),
                None => {
                    self.set_flash("tunnel: no url yet".to_string());
                    TunnelEffect::None
                }
            },
        }
    }

    fn tunnel_stop(&mut self) {
        let Some(state) = &mut self.remote else {
            return;
        };
        state.tunnel.stop();
        if state.config.tunnel_mode == "manual" {
            state.config.tunnel_mode.clear();
            state.config.public_url.clear();
        }
        self.set_flash("tunnel stopped".to_string());
    }

    fn tunnel_select_quick(&mut self) -> TunnelEffect {
        let blocked = self
            .remote
            .as_ref()
            .map(|s| evrax_remote::is_weak_password(&s.config.password))
            .unwrap_or(true);
        if blocked {
            self.set_flash("set a password first (min 8) before using the tunnel".to_string());
            return TunnelEffect::None;
        }
        if let Some(state) = &mut self.remote {
            state.config.tunnel_mode = "quick".to_string();
            // The tunnel follows the remote switch: picking a provider turns
            // the web server on so the tunnel has something to expose.
            state.enabled = true;
        }
        TunnelEffect::Start
    }

    /// Begin editing the manual tunnel URL inline, prefilled with the current
    /// value (empty + enter clears it).
    pub fn tunnel_edit_start(&mut self) {
        let current = self
            .remote
            .as_ref()
            .map(|s| s.config.public_url.clone())
            .unwrap_or_default();
        if let Some(dialog) = self.tunnel_dialog.as_mut() {
            dialog.editing = Some(current);
            dialog.kind = TunnelEditKind::ManualUrl;
        }
    }

    /// Begin editing the custom-domain hostname inline (prefilled when set).
    pub fn tunnel_host_edit_start(&mut self) {
        let current = self
            .remote
            .as_ref()
            .map(|s| s.config.tunnel_hostname.clone())
            .unwrap_or_default();
        if let Some(dialog) = self.tunnel_dialog.as_mut() {
            dialog.editing = Some(current);
            dialog.kind = TunnelEditKind::Hostname;
        }
    }

    pub fn tunnel_edit_input(&mut self, c: char) {
        if let Some(dialog) = self.tunnel_dialog.as_mut()
            && let Some(buf) = dialog.editing.as_mut()
        {
            buf.push(c);
        }
    }

    pub fn tunnel_edit_backspace(&mut self) {
        if let Some(dialog) = self.tunnel_dialog.as_mut()
            && let Some(buf) = dialog.editing.as_mut()
        {
            buf.pop();
        }
    }

    pub fn tunnel_edit_cancel(&mut self) {
        if let Some(dialog) = self.tunnel_dialog.as_mut() {
            dialog.editing = None;
        }
    }

    /// Finish an inline edit: store it and report what the caller should do.
    /// `Err` rejected the input and keeps the editor open.
    pub fn tunnel_edit_commit(&mut self) -> Result<TunnelEditResult, String> {
        let (buf, kind) = match self.tunnel_dialog.as_ref().and_then(|d| d.editing.clone()) {
            Some(b) => (b, self.tunnel_dialog.as_ref().unwrap().kind),
            None => return Err("not editing".into()),
        };
        let trimmed = buf.trim().to_string();
        match kind {
            TunnelEditKind::ManualUrl => {
                if !trimmed.is_empty() {
                    let lower = trimmed.to_lowercase();
                    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
                        return Err("url must start with http:// or https://".into());
                    }
                }
                if let Some(dialog) = self.tunnel_dialog.as_mut() {
                    dialog.editing = None;
                }
                let Some(state) = self.remote.as_mut() else {
                    return Err("remote unavailable".into());
                };
                if trimmed.is_empty() {
                    state.config.public_url.clear();
                    if state.config.tunnel_mode == "manual" {
                        state.config.tunnel_mode.clear();
                    }
                    return Ok(TunnelEditResult::UrlCleared);
                }
                state.config.public_url = trimmed;
                state.config.tunnel_mode = "manual".to_string();
                Ok(TunnelEditResult::UrlSaved)
            }
            TunnelEditKind::Hostname => {
                // Accept pasted URLs too ("https://evrax.example.com/").
                let hostname = trimmed
                    .trim_start_matches("https://")
                    .trim_start_matches("http://")
                    .trim_end_matches('/')
                    .to_lowercase();
                if hostname.is_empty()
                    || !hostname.contains('.')
                    || hostname.contains('/')
                    || hostname.chars().any(|c| c.is_whitespace())
                {
                    return Err("invalid hostname (example: evrax.example.com)".into());
                }
                if self
                    .remote
                    .as_ref()
                    .map(|s| evrax_remote::is_weak_password(&s.config.password))
                    .unwrap_or(true)
                {
                    return Err("set a password first (min 8) before using the tunnel".into());
                }
                if let Some(dialog) = self.tunnel_dialog.as_mut() {
                    dialog.editing = None;
                }
                let Some(state) = self.remote.as_mut() else {
                    return Err("remote unavailable".into());
                };
                state.config.tunnel_hostname = hostname;
                state.config.tunnel_mode = "domain".to_string();
                // The tunnel follows the remote switch: picking a provider turns
                // the web server on so the tunnel has something to expose.
                state.enabled = true;
                Ok(TunnelEditResult::HostReady)
            }
        }
    }

    /// Start the configured tunnel when the remote switch is ON and the
    /// password is safe; stop it when the switch goes OFF.
    pub fn tunnel_follow_remote(&mut self) -> TunnelEffect {
        let Some(state) = self.remote.as_ref() else {
            return TunnelEffect::None;
        };
        let (enabled, mode, hostname, active, password_default) = (
            state.enabled,
            state.config.tunnel_mode.clone(),
            state.config.tunnel_hostname.clone(),
            state.tunnel.is_active(),
            evrax_remote::is_weak_password(&state.config.password),
        );
        let process_mode = mode == "quick" || mode == "domain";
        if active
            && (!enabled || !process_mode || password_default)
            && let Some(state) = &self.remote
        {
            state.tunnel.stop();
        }
        if !enabled || !process_mode {
            return TunnelEffect::None;
        }
        if password_default {
            self.set_flash("tunnel waiting · set a password first (min 8)".to_string());
            return TunnelEffect::None;
        }
        if active {
            return TunnelEffect::None;
        }
        match mode.as_str() {
            "quick" => TunnelEffect::Start,
            "domain" if !hostname.is_empty() => TunnelEffect::Start,
            "domain" => {
                self.set_flash("custom domain: set the hostname first (tunnel menu)".to_string());
                TunnelEffect::None
            }
            _ => TunnelEffect::None,
        }
    }

    /// Spawn the process for the configured tunnel mode (effect executor for
    /// `TunnelEffect::Start`).
    pub fn tunnel_run(&mut self) {
        let weak = self
            .remote
            .as_ref()
            .map(|s| evrax_remote::is_weak_password(&s.config.password))
            .unwrap_or(true);
        if weak {
            self.set_flash("set a password first (min 8) before using the tunnel".to_string());
            return;
        }
        let Some(state) = self.remote.as_ref() else {
            return;
        };
        let port = state.config.port;
        match state.config.tunnel_mode.as_str() {
            "domain" => {
                let hostname = state.config.tunnel_hostname.clone();
                if hostname.is_empty() {
                    self.set_flash("custom domain: hostname not set".to_string());
                    return;
                }
                state.tunnel.start_domain(port, hostname.clone());
                self.set_flash(format!("custom domain starting… ({hostname})"));
            }
            _ => {
                state.tunnel.start_quick(port);
                self.set_flash("quick tunnel starting…".to_string());
            }
        }
    }

    /// Register with the shared server if the switch is ON (idempotent).
    pub fn ensure_remote_registered(&mut self) {
        if self.remote.as_ref().map(|s| s.enabled).unwrap_or(false) {
            self.apply_remote_registration();
        }
    }

    /// Record a tunnel URL on the running server (no-op without a server).
    pub fn push_public_url(&mut self, url: &str) {
        if let Some(server) = self.remote_server() {
            evrax_remote::set_public_url(server, url.to_string());
        }
    }

    /// Registers/deregisters the session with the shared remote server, lazily
    /// starting the local server on first use.
    fn apply_remote_registration(&mut self) {
        let Some(remote) = &self.remote else {
            return;
        };
        let session_id = remote.session_id.clone();
        let permit = remote.permit;
        let enabled = remote.enabled;
        let name = self.display_name();

        if !enabled {
            if let Some(server) = &self.remote_server {
                evrax_remote::deregister(server, &session_id);
            }
            self.set_flash("remote off".to_string());
            return;
        }

        // Pick up manual bind/port edits from remote.json (the dialog keeps
        // those rows read-only). Password/tunnel fields stay authoritative in
        // memory; they are persisted by the caller on every change.
        let disk = evrax_remote::load_remote_config();
        if let Some(remote) = self.remote.as_mut() {
            remote.config.bind = disk.bind.clone();
            remote.config.port = disk.port;
        }
        // Restart the server when its socket is gone (e.g. a busy port earlier)
        // or when bind/port no longer match the loaded config. Dropping the
        // handle releases the stale listener; the code below re-registers.
        let stale = match (self.remote_server.as_ref(), self.remote.as_ref()) {
            (Some(server), Some(remote)) if server.is_running() => {
                let live = evrax_remote::config_snapshot(server);
                live.bind != remote.config.bind || live.port != remote.config.port
            }
            _ => false,
        };
        if stale {
            self.remote_server = None;
        }
        let alive = self.remote_server.as_ref().is_some_and(|s| s.is_running());
        if !alive {
            let cfg = self
                .remote
                .as_ref()
                .map(|s| s.config.clone())
                .unwrap_or_default();
            self.remote_server = Some(evrax_remote::start(cfg));
        }

        let running = self.remote_server.as_ref().is_some_and(|s| s.is_running());
        if !running {
            if let Some(remote) = self.remote.as_mut() {
                remote.enabled = false;
            }
            self.set_flash("remote: server unavailable (port 444 busy?)".to_string());
            return;
        }

        if let Some(server) = &self.remote_server {
            evrax_remote::register(server, session_id, name, permit);
        }
        let (bind, port) = self
            .remote
            .as_ref()
            .map(|s| (s.config.bind.clone(), s.config.port))
            .unwrap_or_default();
        self.set_flash(format!(
            "remote on · {bind}:{port} ({})",
            match permit {
                evrax_remote::Permit::Read => "read",
                evrax_remote::Permit::Write => "read/write",
            }
        ));
    }

    pub fn approval_move(&mut self, delta: i16) {
        if let Some(approval) = &mut self.approval {
            approval.selected = (approval.selected as i16 + delta)
                .rem_euclid(APPROVAL_OPTIONS.len() as i16) as usize;
        }
    }

    pub fn settle_approval(&mut self, decision: ApprovalDecision) -> Option<bool> {
        let approval = self.approval.take()?;
        let allow = match decision {
            ApprovalDecision::Once => true,
            ApprovalDecision::Always => {
                self.auto_allow.insert(approval.key);
                true
            }
            ApprovalDecision::Reject => false,
        };
        let label = match decision {
            ApprovalDecision::Once => "once",
            ApprovalDecision::Always => "always",
            ApprovalDecision::Reject => "reject",
        };
        // Any decision still queued for the approval that just closed targets a
        // dialog the operator can no longer see; drop it instead of letting it
        // settle the next one.
        self.discard_remote_approvals();
        self.emit_approval_resolved(label);
        Some(allow)
    }

    /// Clear every undrained web decision for the current remote session.
    fn discard_remote_approvals(&self) {
        let Some(session_id) = self.remote.as_ref().map(|s| s.session_id.clone()) else {
            return;
        };
        let Some(server) = self.remote_server() else {
            return;
        };
        evrax_remote::clear_pending_approvals(server, &session_id);
    }

    /// Settle the open approval from a web decision string ("once" / "always"
    /// / "reject"), but only when `id` matches the approval currently open.
    /// Unknown strings, a closed dialog, or a stale id are all ignored — a
    /// decision that arrives after its dialog closed must never settle the
    /// next one.
    pub fn settle_remote_approval(&mut self, decision: &str, id: u64) -> Option<bool> {
        if self.approval.as_ref()?.id != id {
            return None;
        }
        let decision = match decision {
            "once" => ApprovalDecision::Once,
            "always" => ApprovalDecision::Always,
            "reject" => ApprovalDecision::Reject,
            _ => return None,
        };
        self.settle_approval(decision)
    }

    pub fn take_auto_approval(&mut self) -> bool {
        if let Some(approval) = &self.approval
            && self.auto_allow.contains(&approval.key)
        {
            self.approval = None;
            self.discard_remote_approvals();
            self.emit_approval_resolved("auto");
            return true;
        }
        false
    }

    /// Mirror an approval outcome to the web subscribers (no-op when off).
    fn emit_approval_resolved(&mut self, decision: &str) {
        let Some(turn) = self.remote.as_ref().map(|s| s.session_id.clone()) else {
            return;
        };
        self.emit_remote(evrax_remote::RemoteEvent::ApprovalResolved {
            turn,
            decision: decision.to_string(),
        });
    }

    pub fn request_cancel(&mut self) -> bool {
        let now = self.tick;
        if let Some(prev) = self.last_esc
            && now.saturating_sub(prev) <= ESC_CANCEL_TICKS
        {
            self.last_esc = None;
            return true;
        }
        self.last_esc = Some(now);
        self.set_flash("press esc again to cancel".to_string());
        false
    }

    /// True when the composer may accept typing while a turn streams: busy,
    /// with no overlay/dialog stealing the keyboard.
    pub fn stream_input_open(&self) -> bool {
        self.busy
            && self.approval.is_none()
            && self.palette.is_none()
            && self.session_dialog.is_none()
            && self.theme_dialog.is_none()
            && self.remote_dialog.is_none()
            && self.preview.is_none()
            && self.wizard.is_none()
            && !self.model_dialog_open()
    }

    pub fn submit_prompt(&mut self) -> Option<String> {
        let prompt = self.input.text().trim().to_string();
        if prompt.is_empty() && self.attachments.is_empty() && self.pasted_text.is_empty() {
            return None;
        }
        if prompt.starts_with('/') && self.attachments.is_empty() {
            self.input.take();
            self.slash = None;
            self.slash_dismissed = None;
            self.run_slash_line(&prompt);
            return None;
        }
        if self.busy {
            // Attachments have nowhere to live between turns: keep them (and
            // the draft) in the composer instead of dropping them in a queue.
            if !self.attachments.is_empty() {
                self.set_flash("not available while streaming (attachments)".to_string());
                return None;
            }
            self.input.take();
            self.queue_prompt(prompt);
            return None;
        }
        self.input.take();
        self.attachments.clear();
        self.begin_turn(prompt, true)
    }

    /// Books a prompt that cannot start yet (a turn is streaming).
    fn queue_prompt(&mut self, prompt: String) {
        if self.queued.len() >= MAX_QUEUED_PROMPTS {
            self.set_flash(format!(
                "queue full ({MAX_QUEUED_PROMPTS}) · prompt dropped"
            ));
            return;
        }
        self.queued.push_back(prompt);
        self.set_flash(format!("queued ({})", self.queued.len()));
    }

    /// Pops the oldest queued prompt and starts it — only while idle.
    pub fn take_queued_prompt(&mut self) -> Option<String> {
        if self.busy {
            return None;
        }
        let prompt = self.queued.pop_front()?;
        self.begin_turn(prompt, true)
    }

    /// Runs a prompt as a new turn: transcript, title placeholder, busy flag
    /// and the remote echo are handled here for every entry path.
    fn begin_turn(&mut self, prompt: String, echo_user: bool) -> Option<String> {
        let prompt = self.merge_pasted_text(prompt);
        self.push_entry(ChatEntry::User(prompt.clone()));
        self.save_current();
        self.ensure_placeholder_title(&prompt);
        self.busy = true;
        self.iteration = None;
        self.turn_started = Some(Instant::now());
        self.asst_open = false;
        self.scroll_from_bottom = 0;
        if let Some(turn) = self.remote.as_ref().map(|s| s.session_id.clone()) {
            // Web-originated prompts (`echo_user == false`) already have their
            // bubble broadcast by the remote server; echoing again here would
            // render the same prompt twice in every open tab.
            if echo_user {
                self.emit_remote(evrax_remote::RemoteEvent::User {
                    turn: turn.clone(),
                    text: prompt.clone(),
                });
            }
            self.emit_remote(evrax_remote::RemoteEvent::TurnStart { turn });
        }
        Some(prompt)
    }

    /// Appends each parked paste to the prompt (with a `[pasted N chars]`
    /// marker so the transcript shows where the block came from) and clears
    /// the chips. Returns the prompt unchanged when nothing was pasted.
    fn merge_pasted_text(&mut self, prompt: String) -> String {
        if self.pasted_text.is_empty() {
            return prompt;
        }
        let mut merged = prompt;
        for pasted in self.pasted_text.drain(..) {
            if !merged.is_empty() {
                merged.push_str("\n\n");
            }
            merged.push_str(&format!("[pasted {} chars]\n{}", pasted.chars, pasted.body));
        }
        merged
    }

    /// Runs a `/name [arg]` line typed into the composer (the slash menu only
    /// matches argument-less names, so this is the entry point for `/rename`).
    fn run_slash_line(&mut self, line: &str) {
        let rest = &line[1..];
        let (name, arg) = match rest.split_once(char::is_whitespace) {
            Some((n, a)) => (n, a.trim()),
            None => (rest, ""),
        };
        let Some((_, _, command)) = slash_options()
            .into_iter()
            .find(|(entry, _, _)| *entry == name)
        else {
            self.set_flash(format!("unknown command: /{name}"));
            return;
        };
        if command == Command::Rename {
            self.rename_session_title(arg);
            return;
        }
        if command == Command::Worker {
            let n = if arg.is_empty() {
                1
            } else {
                match arg.trim().parse::<usize>() {
                    Ok(n) => n,
                    Err(_) => {
                        self.set_flash("usage: /worker <1-3>".to_string());
                        return;
                    }
                }
            };
            self.spawn_workers(n);
            return;
        }
        if command == Command::Workers {
            self.workers_command(arg);
            return;
        }
        if !arg.is_empty() {
            self.set_flash(format!("/{name} takes no arguments"));
            return;
        }
        self.execute_command(command);
    }

    /// Submit a prompt that originated from an external inbox (worker).
    /// Mirrors `submit_prompt` but uses the given text instead of the
    /// composer, so external prompts run under the current TUI mode and surface
    /// tool approvals in the terminal as usual.
    pub fn submit_external_prompt(&mut self, text: String) -> Option<String> {
        self.submit_with_echo(text, true)
    }

    /// Submit a prompt queued by the remote server (web UI). Its user bubble
    /// was already broadcast to every open tab at POST time, so the TUI must
    /// not echo it a second time when the turn actually starts.
    pub fn submit_web_prompt(&mut self, text: String) -> Option<String> {
        self.submit_with_echo(text, false)
    }

    fn submit_with_echo(&mut self, text: String, echo_user: bool) -> Option<String> {
        let prompt = text.trim().to_string();
        if prompt.is_empty() {
            return None;
        }
        if self.busy {
            self.queue_prompt(prompt);
            return None;
        }
        self.begin_turn(prompt, echo_user)
    }

    /// Routes a paste into the composer. Pastes longer than 10 lines become a
    /// chip (`pasted_text`) so the line editor never re-wraps a huge buffer on
    /// every repaint; returns true when handled as a chip. Short pastes go
    /// straight into the editor.
    pub fn paste_text_input(&mut self, text: &str) -> bool {
        if text.lines().count() > 10 {
            let chars = text.chars().count();
            self.pasted_text.push(PastedText {
                chars,
                body: text.to_string(),
            });
            self.set_flash(format!("pasted {chars} chars · chip added"));
            true
        } else {
            self.input.insert_str(text);
            false
        }
    }

    /// Backspace in the composer: edits a character, or — when the editor is
    /// already empty — drops the most recent paste chip as one unit (P16).
    pub fn backspace_input(&mut self) {
        if self.input.is_empty() && !self.pasted_text.is_empty() {
            self.pasted_text.pop();
            return;
        }
        self.input.backspace();
    }

    pub fn open_preview(&mut self, index: usize) {
        if index < self.attachments.len() {
            self.preview = Some(index);
        }
    }

    pub fn close_preview(&mut self) {
        self.preview = None;
    }

    pub fn remove_attachment(&mut self, index: usize) {
        if index >= self.attachments.len() {
            return;
        }
        self.attachments.remove(index);
        match self.preview {
            Some(i) if i == index => self.preview = None,
            Some(i) if i > index => self.preview = Some(i - 1),
            _ => {}
        }
    }

    /// Handles a paste-like burst: pull real image pixels from
    /// the clipboard first (screenshot), then try a pasted file path, and only
    /// fall back to literal text. Returns true when handled as an attachment.
    pub fn ingest_paste(&mut self, text: &str) -> bool {
        if let Some(staged) = crate::image::clipboard_image_png() {
            let staged = stage_named_copy(&staged);
            if let Some(attachment) = Attachment::from_path(&staged) {
                self.attachments.push(attachment);
                self.set_flash("image attached from clipboard".to_string());
                return true;
            }
        }
        if let Some(attachment) = Attachment::from_path(text) {
            self.attachments.push(attachment);
            self.set_flash("image attached".to_string());
            return true;
        }
        false
    }

    pub fn composer_height(&mut self, width: usize) -> u16 {
        let chips = self.attachments.len().min(7) as u16;
        let paste_chips = self.pasted_text.len().min(4) as u16;
        let text_rows = self.input.visual_rows(width).len().clamp(1, 7) as u16;
        (chips + paste_chips + text_rows).clamp(1, 11) + 2
    }

    pub fn last_assistant_text(&self) -> Option<String> {
        self.messages.iter().rev().find_map(|entry| match entry {
            ChatEntry::Assistant(text) if !text.is_empty() => Some(text.clone()),
            _ => None,
        })
    }

    pub fn cancel_turn(&mut self) {
        self.busy = false;
        self.turn_started = None;
        self.asst_open = false;
        let cleared = self.queued.len();
        self.queued.clear();
        for entry in self.messages.iter_mut().rev() {
            if let ChatEntry::Tool(tool) = entry
                && matches!(tool.status, ToolStatus::Running | ToolStatus::Preparing)
            {
                tool.status = ToolStatus::Error;
                tool.detail = "cancelled".to_string();
            }
        }
        self.content_rev = self.content_rev.wrapping_add(1);
        if cleared > 0 {
            self.set_flash(format!("cancelled · {cleared} queued prompt(s) cleared"));
        }
    }

    /// Flips the transcript between quiet and fully-expanded (`ctrl+t`/`ctrl+o`).
    pub fn toggle_expanded(&mut self) {
        self.expanded = !self.expanded;
        self.content_rev = self.content_rev.wrapping_add(1);
        self.transcript.invalidate();
        self.set_flash(if self.expanded {
            "transcript: expanded".to_string()
        } else {
            "transcript: quiet".to_string()
        });
    }

    pub fn apply_event(&mut self, event: AgentEvent) {
        let remote_turn = self.remote.as_ref().map(|s| s.session_id.clone());
        match event {
            AgentEvent::ThinkingDelta { text } => {
                self.asst_open = false;
                match self.messages.last_mut() {
                    Some(ChatEntry::Thinking { summary, .. }) => {
                        summary.push_str(&text);
                        self.content_rev = self.content_rev.wrapping_add(1);
                    }
                    _ => self.push_entry(ChatEntry::Thinking {
                        summary: text.clone(),
                        seconds: 0.0,
                    }),
                }
                if let Some(turn) = &remote_turn {
                    self.emit_remote(evrax_remote::RemoteEvent::Thinking {
                        turn: turn.clone(),
                        text,
                    });
                }
            }
            AgentEvent::ThinkingFinished { seconds } => {
                if let Some(ChatEntry::Thinking { seconds: s, .. }) = self.messages.last_mut() {
                    *s = seconds;
                    self.content_rev = self.content_rev.wrapping_add(1);
                }
            }
            AgentEvent::TextDelta { text } => {
                if self.asst_open {
                    if let Some(ChatEntry::Assistant(body)) = self.messages.last_mut() {
                        body.push_str(&text);
                        self.content_rev = self.content_rev.wrapping_add(1);
                    }
                } else {
                    self.push_entry(ChatEntry::Assistant(text.clone()));
                    self.asst_open = true;
                }
                if let Some(turn) = &remote_turn {
                    self.emit_remote(evrax_remote::RemoteEvent::Delta {
                        turn: turn.clone(),
                        text,
                    });
                }
            }
            AgentEvent::ToolPreparing { name, bytes } => {
                self.asst_open = false;
                let detail = format!("preparing… {bytes} B");
                // Repeated progress frames for the same call keep its row (and
                // id); anything else starts a new call row with a fresh id.
                let continues = matches!(
                    self.messages.last(),
                    Some(ChatEntry::Tool(t)) if t.name == name
                        && matches!(t.status, ToolStatus::Preparing)
                );
                let call = if continues {
                    match self.messages.last() {
                        Some(ChatEntry::Tool(t)) => t.call,
                        _ => 0,
                    }
                } else {
                    self.remote_tool_seq = self.remote_tool_seq.wrapping_add(1);
                    self.remote_tool_seq
                };
                if continues {
                    if let Some(ChatEntry::Tool(tool)) = self.messages.last_mut() {
                        tool.detail = detail.clone();
                        self.content_rev = self.content_rev.wrapping_add(1);
                    }
                } else {
                    self.push_entry(ChatEntry::Tool(ToolCall {
                        name: name.clone(),
                        summary: "preparing".to_string(),
                        status: ToolStatus::Preparing,
                        detail: detail.clone(),
                        call,
                    }));
                }
                if let Some(turn) = &remote_turn {
                    self.emit_remote(evrax_remote::RemoteEvent::Tool {
                        turn: turn.clone(),
                        name,
                        status: "preparing".into(),
                        detail: Some(detail),
                        call,
                    });
                }
            }
            AgentEvent::ToolStarted { name, summary } => {
                self.asst_open = false;
                // A preparing placeholder for the same call is upgraded in
                // place so the transcript shows one row per tool call.
                let call = if let Some(ChatEntry::Tool(tool)) = self.messages.last_mut()
                    && matches!(tool.status, ToolStatus::Preparing)
                    && tool.name == name
                {
                    tool.summary = summary;
                    tool.status = ToolStatus::Running;
                    tool.detail = "running…".to_string();
                    self.content_rev = self.content_rev.wrapping_add(1);
                    tool.call
                } else {
                    self.remote_tool_seq = self.remote_tool_seq.wrapping_add(1);
                    let call = self.remote_tool_seq;
                    self.push_entry(ChatEntry::Tool(ToolCall {
                        name: name.clone(),
                        summary,
                        status: ToolStatus::Running,
                        detail: "running…".to_string(),
                        call,
                    }));
                    call
                };
                if let Some(turn) = &remote_turn {
                    self.emit_remote(evrax_remote::RemoteEvent::Tool {
                        turn: turn.clone(),
                        name,
                        status: "running".into(),
                        detail: None,
                        call,
                    });
                }
            }
            AgentEvent::ToolFinished { name, ok, detail } => {
                self.asst_open = false;
                let mut call = 0;
                for entry in self.messages.iter_mut().rev() {
                    if let ChatEntry::Tool(tool) = entry
                        && matches!(tool.status, ToolStatus::Running | ToolStatus::Preparing)
                        && tool.name == name
                    {
                        tool.status = if ok {
                            ToolStatus::Done
                        } else {
                            ToolStatus::Error
                        };
                        tool.detail = detail.clone();
                        call = tool.call;
                        self.content_rev = self.content_rev.wrapping_add(1);
                        break;
                    }
                }
                if let Some(turn) = &remote_turn {
                    // `call` comes from the row that just finished, so the web
                    // updates the same row instead of adding another.
                    self.emit_remote(evrax_remote::RemoteEvent::Tool {
                        turn: turn.clone(),
                        name,
                        status: if ok { "done".into() } else { "error".into() },
                        detail: Some(detail),
                        call,
                    });
                }
            }
            AgentEvent::ApprovalRequested { name, summary, key } => {
                self.asst_open = false;
                // Ids start at 1: 0 means "client sent no nonce" and can never
                // match an open dialog.
                self.remote_approval_seq = self.remote_approval_seq.wrapping_add(1);
                let id = self.remote_approval_seq;
                if let Some(turn) = &remote_turn {
                    self.emit_remote(evrax_remote::RemoteEvent::Approval {
                        turn: turn.clone(),
                        name: name.clone(),
                        summary: summary.clone(),
                        id,
                    });
                }
                self.approval = Some(Approval {
                    name,
                    summary,
                    key,
                    selected: 0,
                    id,
                });
            }
            AgentEvent::Diff { path, lines } => {
                self.asst_open = false;
                self.push_entry(ChatEntry::Diff { path, lines });
            }
            AgentEvent::TurnFinished => {
                self.busy = false;
                self.iteration = None;
                self.turn_started = None;
                self.asst_open = false;
                self.save_current();
                self.plan_title_job();
                if let Some(turn) = &remote_turn {
                    self.emit_remote(evrax_remote::RemoteEvent::TurnDone { turn: turn.clone() });
                }
            }
            AgentEvent::TitleSuggested { session, title } => {
                if !self.manual_titles.contains(&session)
                    && (session == self.session_id || self.sessions.contains(&session))
                {
                    self.set_title(&session, title, false);
                }
            }
            AgentEvent::Notice { message } => {
                self.set_flash(message);
            }
            AgentEvent::Retrying {
                attempt,
                max,
                delay_ms,
                reason,
            } => {
                let seconds = delay_ms.div_ceil(1000);
                self.set_flash(format!("retry {attempt}/{max} · {seconds}s · {reason}"));
            }
            AgentEvent::Iteration { n, max } => {
                self.iteration = Some((n, max));
            }
            AgentEvent::Failed { message } => {
                self.busy = false;
                self.iteration = None;
                self.turn_started = None;
                self.asst_open = false;
                self.save_current();
                self.push_entry(ChatEntry::Assistant(format!("error: {message}")));
                if let Some(turn) = &remote_turn {
                    self.emit_remote(evrax_remote::RemoteEvent::Error {
                        turn: Some(turn.clone()),
                        message,
                    });
                }
            }
        }
    }

    /// Mirrors local agent events to the web subscribers when `/remote` is on.
    /// The shared remote server handle, if a server has been started.
    pub fn remote_server(&self) -> Option<&evrax_remote::RemoteServer> {
        self.remote_server.as_ref()
    }

    fn emit_remote(&mut self, event: evrax_remote::RemoteEvent) {
        // Test-only trace of every emission attempt (before the enabled/server
        // gates) so unit tests can assert which events a code path emits.
        #[cfg(test)]
        self.remote_log.push(event.clone());
        let enabled = self.remote.as_ref().map(|s| s.enabled).unwrap_or(false);
        if !enabled {
            return;
        }
        let Some(server) = &self.remote_server else {
            return;
        };
        let Some(session_id) = self.remote.as_ref().map(|s| s.session_id.clone()) else {
            return;
        };
        evrax_remote::emit(server, &session_id, event);
    }

    /// Name shown in the status line and sidebar: the model actually in use,
    /// or `mock` when the scripted backend is active.
    pub fn set_backend_label(&mut self, label: impl Into<String>) {
        self.backend_label = label.into();
    }

    /// Two-way Plan/Build sync: apply a mode requested from the web, then publish
    /// the TUI's mode back so every tab shows the same state.
    pub fn sync_remote_mode(&mut self) {
        if self.remote.as_ref().map(|s| s.enabled) != Some(true) {
            return;
        }
        let requested = {
            let Some(server) = &self.remote_server else {
                return;
            };
            let Some(session_id) = self.remote.as_ref().map(|s| s.session_id.clone()) else {
                return;
            };
            evrax_remote::take_pending_mode(server, &session_id)
        };
        if let Some(mode) = requested {
            self.mode = match mode.as_str() {
                "plan" => AgentMode::Plan,
                "build" => AgentMode::Build,
                _ => self.mode,
            };
        }
        let Some(server) = &self.remote_server else {
            return;
        };
        let Some(session_id) = self.remote.as_ref().map(|s| s.session_id.clone()) else {
            return;
        };
        evrax_remote::set_session_mode(server, &session_id, &self.mode.label().to_lowercase());
    }

    pub fn scroll_view(&mut self, amount: u16, up: bool, max_scroll: u16) {
        self.scroll_internal(amount, up, max_scroll);
    }

    fn scroll_internal(&mut self, amount: u16, up: bool, max_scroll: u16) {
        let current = self.scroll_from_bottom;
        let target = if up {
            current.saturating_add(amount).min(max_scroll)
        } else {
            current.saturating_sub(amount)
        };
        let delta = target.abs_diff(current);
        if delta == 0 {
            return;
        }
        self.scroll_from_bottom = target;
        if self.selection.is_dragging() {
            self.selection.shift_anchor(up, delta);
        } else {
            self.selection.clear();
        }
    }

    pub fn selection_edge(&self) -> Option<bool> {
        if !self.selection.is_dragging() {
            return None;
        }
        let head = self.selection.head()?;
        let area = self.selectable_area;
        if area.height == 0 {
            return None;
        }
        let last = area.bottom().saturating_sub(1);
        if head.1 <= area.top() {
            Some(true)
        } else if head.1 >= last {
            Some(false)
        } else {
            None
        }
    }

    pub fn set_viewport(&mut self, height: u16) {
        self.viewport_height = height;
    }

    pub fn clamp_scroll(&mut self, max: u16) {
        self.scroll_from_bottom = self.scroll_from_bottom.min(max);
    }

    pub fn page(&self) -> u16 {
        self.viewport_height.saturating_sub(2).max(1)
    }

    /// Spawns `n` worker panes (clamped to `MAX_WORKERS`) and nudges the main
    /// agent to split the work. Called by `/worker N`. The real process launch
    /// lives in the event loop: this only records the request + placeholder
    /// panes so the TUI can lay out the grid immediately.
    pub fn spawn_workers(&mut self, n: usize) {
        if self.busy {
            self.set_flash("not available while streaming".to_string());
            return;
        }
        let n = n.clamp(1, MAX_WORKERS);
        for _ in 0..n {
            self.worker_seq += 1;
            self.workers.push(WorkerPane {
                id: String::new(),
                name: format!("worker {}", self.worker_seq),
                status: "starting".to_string(),
                pid: 0,
                transcript: Vec::new(),
                last_reply: String::new(),
                cache: crate::ui::TranscriptCache::default(),
                events_seen: 0,
            });
        }
        self.worker_spawn_request = Some(n);
        self.set_flash(format!(
            "spawning {n} worker{}…",
            if n == 1 { "" } else { "s" }
        ));
    }

    /// Pending spawn count consumed by the event loop (lane D).
    pub fn take_worker_spawn_request(&mut self) -> Option<usize> {
        self.worker_spawn_request.take()
    }

    /// Reads worker state files + event JSONL into the panes (called each tick).
    /// Read-only: never writes state or events. Only the new JSONL lines are
    /// folded in (tracked by `WorkerPane.events_seen`), so an idle worker costs
    /// no rebuild.
    pub fn poll_workers(&mut self) {
        if self.workers.is_empty() {
            return;
        }
        for pane in &mut self.workers {
            if pane.id.is_empty() {
                continue;
            }
            if let Some(state) = evrax_agent::worker::load(&pane.id) {
                pane.name = state.name;
                pane.pid = state.pid;
                pane.status = state.status.as_str().to_string();
                pane.last_reply = state.last_reply;
            }
            if append_new_events(pane) {
                pane.cache.invalidate();
            }
        }
    }

    /// True when at least one worker pane exists (lane C renders the grid).
    pub fn has_workers(&self) -> bool {
        !self.workers.is_empty()
    }

    /// Closes/clears all worker panes (e.g. on `/new` or quit).
    pub fn clear_workers(&mut self) {
        self.workers.clear();
        self.worker_spawn_request = None;
    }

    /// `/workers [clear|kill <id>]` — manage this project's workers.
    /// No argument lists them; `clear` removes every worker of the project;
    /// `kill <id>` removes one. Pane view state is dropped to match.
    pub fn workers_command(&mut self, arg: &str) {
        let mut parts = arg.split_whitespace();
        match parts.next() {
            None => {
                let states = evrax_agent::worker::list_for(&self.root);
                if states.is_empty() {
                    self.set_flash("no workers in this project".to_string());
                    return;
                }
                let summary = states
                    .iter()
                    .map(|state| format!("{} [{}]", state.id, state.status))
                    .collect::<Vec<_>>()
                    .join(" · ");
                self.set_flash(format!("{} worker(s): {summary}", states.len()));
            }
            Some("clear") => {
                let states = evrax_agent::worker::list_for(&self.root);
                let mut removed = 0;
                for state in &states {
                    if remove_worker_dir(&state.id) {
                        removed += 1;
                    }
                }
                self.workers.clear();
                if removed == 0 && states.is_empty() {
                    self.set_flash("no workers to clear".to_string());
                } else {
                    self.set_flash(format!("cleared {removed} worker(s)"));
                }
            }
            Some("kill") => {
                let Some(id) = parts.next() else {
                    self.set_flash("usage: /workers kill <id>".to_string());
                    return;
                };
                if remove_worker_dir(id) {
                    self.workers.retain(|pane| pane.id != id);
                    self.set_flash(format!("killed {id}"));
                } else {
                    self.set_flash(format!("worker {id} not found"));
                }
            }
            Some(other) => {
                self.set_flash(format!("unknown /workers argument: {other}"));
            }
        }
    }

    /// Rebuilds worker panes from disk for a resumed session (P20). Live workers
    /// reconnect; workers whose heartbeat is gone still show as `stopped`.
    fn restore_worker_panes(&mut self, session: &str) {
        self.workers.clear();
        self.worker_spawn_request = None;
        self.workers =
            worker_panes_from(evrax_agent::worker::list_for_session(session, &self.root));
    }
}

/// Builds pane view state from worker `State` records (resume/P20). Split out
/// from the disk lookup so it can be tested without global worker-dir env.
fn worker_panes_from(states: Vec<evrax_agent::worker::State>) -> Vec<WorkerPane> {
    states
        .into_iter()
        .map(|state| WorkerPane {
            id: state.id,
            name: state.name,
            status: state.status.as_str().to_string(),
            pid: state.pid,
            transcript: Vec::new(),
            last_reply: state.last_reply,
            cache: crate::ui::TranscriptCache::default(),
            events_seen: 0,
        })
        .collect()
}

/// Removes one worker's state directory. Returns true when it existed.
/// Preference would be a `worker::remove` from the agent crate; until that
/// exists this derives the directory from `worker::state_path` (defensive:
/// a missing file is not an error).
fn remove_worker_dir(id: &str) -> bool {
    let state = evrax_agent::worker::state_path(id);
    let Some(dir) = state.parent() else {
        return false;
    };
    if !dir.is_dir() {
        return false;
    }
    std::fs::remove_dir_all(dir).is_ok()
}

/// Folds only the JSONL lines past `pane.events_seen` into `pane.transcript`,
/// mapping each new line with the same coalescing as a full rebuild. Returns
/// true when at least one entry changed (so the caller invalidates its cache).
fn append_new_events(pane: &mut WorkerPane) -> bool {
    let path = evrax_agent::worker::events_path(&pane.id);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return false;
    };
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= pane.events_seen {
        return false;
    }
    let mut changed = false;
    for &line in &lines[pane.events_seen..] {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        // A new line either pushes an entry or coalesces into the trailing one;
        // both change the rendered transcript, so invalidate either way.
        if let Some(entry) = event_value_to_entry(&value, &mut pane.transcript) {
            pane.transcript.push(entry);
        }
        changed = true;
    }
    pane.events_seen = lines.len();
    changed
}

/// Maps one JSONL event to a `ChatEntry`, coalescing text deltas into the
/// trailing assistant entry. Returns `None` for events that leave the
/// transcript unchanged. `AgentEvent` is not `Serialize` in `evrax-core`, so the
/// worker writes plain JSON values and this maps the shapes defensively (a
/// malformed or future line never crashes the TUI).
// ponytail: minimal mapping, extend when worker panes need richer rendering.
fn event_value_to_entry(
    value: &serde_json::Value,
    existing: &mut [ChatEntry],
) -> Option<ChatEntry> {
    let kind = value
        .get("type")
        .or_else(|| value.get("kind"))
        .or_else(|| value.get("event"))
        .and_then(serde_json::Value::as_str)?;
    let text = |keys: &[&str]| -> Option<String> {
        keys.iter()
            .find_map(|k| value.get(*k).and_then(serde_json::Value::as_str))
            .map(str::to_string)
    };
    match kind {
        "TextDelta" | "text_delta" | "delta" | "assistant" => {
            let chunk = text(&["text", "delta", "content"])?;
            if let Some(ChatEntry::Assistant(body)) = existing.last_mut() {
                body.push_str(&chunk);
                None
            } else {
                Some(ChatEntry::Assistant(chunk))
            }
        }
        "ThinkingDelta" | "thinking_delta" | "thinking" => {
            let chunk = text(&["text", "delta", "content"])?;
            if let Some(ChatEntry::Thinking { summary, .. }) = existing.last_mut() {
                summary.push_str(&chunk);
                None
            } else {
                Some(ChatEntry::Thinking {
                    summary: chunk,
                    seconds: 0.0,
                })
            }
        }
        "user" | "User" | "prompt" => {
            let chunk = text(&["text", "content", "prompt"])?;
            Some(ChatEntry::User(chunk))
        }
        "ToolStarted" | "tool_started" | "tool" => {
            let summary = text(&["summary", "name", "message"]).unwrap_or_else(|| kind.to_string());
            Some(ChatEntry::Tool(ToolCall {
                name: text(&["name"]).unwrap_or_else(|| "tool".to_string()),
                summary,
                status: ToolStatus::Done,
                detail: String::new(),
                call: 0,
            }))
        }
        "notice" | "Notice" => {
            let chunk = text(&["message", "text"]).unwrap_or_default();
            Some(ChatEntry::Assistant(chunk))
        }
        "Failed" | "failed" | "error" => {
            let message = text(&["message", "error"]).unwrap_or_default();
            Some(ChatEntry::Tool(ToolCall {
                name: "error".to_string(),
                summary: message,
                status: ToolStatus::Error,
                detail: String::new(),
                call: 0,
            }))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use evrax_core::AgentEvent;

    fn app() -> App {
        App::new()
    }

    /// Wrapped rows joined with `|`, for compact literal assertions.
    fn rows(ed: &mut LineEditor, width: usize) -> String {
        ed.visual_rows(width).join("|")
    }

    fn type_and_submit(app: &mut App, text: &str) -> Option<String> {
        for c in text.chars() {
            app.input.insert_char(c);
        }
        app.submit_prompt()
    }

    #[test]
    fn editor_handles_typing_and_movement() {
        let mut ed = LineEditor::new();
        for c in "hello".chars() {
            ed.insert_char(c);
        }
        ed.home();
        ed.insert_char('X');
        ed.end();
        ed.backspace();
        ed.move_left();
        ed.delete();
        assert_eq!(ed.text(), "Xhel");
        assert_eq!(ed.cursor(), 4);
        assert!(!ed.is_empty());
    }

    #[test]
    fn submit_pushes_user_message_and_clears_input() {
        let mut app = app();
        let before = app.messages.len();
        for c in "fix it".chars() {
            app.input.insert_char(c);
        }
        let prompt = app.submit_prompt();
        assert_eq!(prompt.as_deref(), Some("fix it"));
        assert!(app.busy);
        assert!(app.input.is_empty());
        assert_eq!(app.messages.len(), before + 1);
        assert!(matches!(
            app.messages.last(),
            Some(ChatEntry::User(text)) if text == "fix it"
        ));
        assert!(app.submit_prompt().is_none());
    }

    #[test]
    fn empty_submit_is_ignored() {
        let mut app = app();
        for c in "   ".chars() {
            app.input.insert_char(c);
        }
        assert!(app.submit_prompt().is_none());
        assert!(!app.busy);
    }

    #[test]
    fn queued_prompts_run_after_the_current_turn() {
        let mut app = app();
        type_and_submit(&mut app, "first");
        assert!(app.busy);
        let before = app.messages.len();
        // A second prompt typed mid-turn is queued, not dropped and not run.
        for c in "second".chars() {
            app.input.insert_char(c);
        }
        assert!(app.submit_prompt().is_none());
        assert!(app.input.is_empty());
        assert_eq!(app.queued.len(), 1);
        assert_eq!(app.messages.len(), before);
        // Nothing runs while the turn is streaming.
        assert!(app.take_queued_prompt().is_none());
        // The turn ends: the queued prompt becomes the next turn.
        app.busy = false;
        let prompt = app.take_queued_prompt().expect("queued prompt runs");
        assert_eq!(prompt, "second");
        assert!(app.busy);
        assert!(app.queued.is_empty());
    }

    #[test]
    fn queue_is_capped_and_cancel_clears_it() {
        let mut app = app();
        type_and_submit(&mut app, "first");
        for i in 0..(MAX_QUEUED_PROMPTS + 3) {
            app.queued.push_back(format!("queued-{i}"));
        }
        // Re-queueing through the public path respects the cap.
        app.queued.truncate(MAX_QUEUED_PROMPTS);
        for c in "extra".chars() {
            app.input.insert_char(c);
        }
        assert!(app.submit_prompt().is_none());
        assert_eq!(app.queued.len(), MAX_QUEUED_PROMPTS);
        app.cancel_turn();
        assert!(!app.busy);
        assert!(app.queued.is_empty());
        assert!(app.turn_started.is_none());
    }

    #[test]
    fn mutating_slash_is_blocked_while_streaming_read_only_is_not() {
        let mut app = app();
        type_and_submit(&mut app, "first");
        assert!(app.busy);
        // `/new` swaps the session out from under the running turn -> refused.
        for c in "/new".chars() {
            app.input.insert_char(c);
        }
        app.submit_prompt();
        assert!(app.sessions.is_empty());
        assert!(
            app.flash
                .as_ref()
                .is_some_and(|(text, _)| text.contains("not available while streaming"))
        );
        // `/mode` only flips a flag -> allowed while busy.
        let mode_before = app.mode;
        for c in "/mode".chars() {
            app.input.insert_char(c);
        }
        app.submit_prompt();
        assert_ne!(app.mode, mode_before);
    }

    #[test]
    fn stream_input_open_true_only_while_busy_without_overlay() {
        let mut app = app();
        assert!(!app.stream_input_open(), "idle composer is not stream-open");
        type_and_submit(&mut app, "hi");
        assert!(app.busy);
        assert!(
            app.stream_input_open(),
            "busy with no overlay is stream-open"
        );
        app.approval = Some(Approval {
            name: "shell".to_string(),
            summary: "ls".to_string(),
            key: "shell".to_string(),
            selected: 0,
            id: 1,
        });
        assert!(!app.stream_input_open(), "an approval overlay closes it");
    }

    #[test]
    fn slash_runs_while_streaming() {
        let mut app = app();
        type_and_submit(&mut app, "first");
        assert!(app.busy);
        let mode_before = app.mode;
        for c in "/mode".chars() {
            app.input.insert_char(c);
        }
        app.submit_prompt();
        assert_ne!(app.mode, mode_before, "/mode executes while streaming");
        assert!(app.queued.is_empty(), "a slash line is never queued");
    }

    #[test]
    fn text_queues_while_streaming() {
        let mut app = app();
        type_and_submit(&mut app, "first");
        assert!(app.busy);
        type_and_submit(&mut app, "hi");
        assert_eq!(app.queued.len(), 1);
        assert_eq!(app.queued.front().map(String::as_str), Some("hi"));
    }

    #[test]
    fn expanded_toggle_flips_the_flag_and_invalidates_the_cache() {
        let mut app = app();
        assert!(!app.expanded);
        app.toggle_expanded();
        assert!(app.expanded);
        app.toggle_expanded();
        assert!(!app.expanded);
    }

    #[test]
    fn turn_started_is_tracked_for_the_footer_clock() {
        let mut app = app();
        assert!(app.turn_started.is_none());
        type_and_submit(&mut app, "time me");
        assert!(app.turn_started.is_some());
        app.apply_event(AgentEvent::TurnFinished);
        assert!(app.turn_started.is_none());
    }

    #[test]
    fn tool_preparing_placeholder_is_upgraded_in_place() {
        let mut app = app();
        app.busy = true;
        app.apply_event(AgentEvent::ToolPreparing {
            name: "read".into(),
            bytes: 4096,
        });
        assert!(matches!(
            app.messages.last(),
            Some(ChatEntry::Tool(tool))
                if matches!(tool.status, ToolStatus::Preparing) && tool.name == "read"
        ));
        let before = app.messages.len();
        app.apply_event(AgentEvent::ToolStarted {
            name: "read".into(),
            summary: "src/lib.rs".into(),
        });
        // Same row, now running — not a second entry.
        assert_eq!(app.messages.len(), before);
        assert!(matches!(
            app.messages.last(),
            Some(ChatEntry::Tool(tool))
                if matches!(tool.status, ToolStatus::Running) && tool.summary == "src/lib.rs"
        ));
    }

    #[test]
    fn editor_supports_multiline_navigation() {
        let mut ed = LineEditor::new();
        ed.insert_str("first line");
        ed.insert_newline();
        ed.insert_str("second");
        assert_eq!(rows(&mut ed, 80), "first line|second");
        assert_eq!(ed.cursor_line_col(), (1, 6));
        ed.move_up();
        assert_eq!(ed.cursor_line_col(), (0, 6));
        ed.home();
        assert_eq!(ed.cursor(), 0);
        ed.end();
        assert_eq!(ed.cursor(), 10);
        ed.move_down();
        assert_eq!(ed.cursor_line_col(), (1, 6));
        ed.move_down();
        assert_eq!(ed.cursor_line_col(), (1, 6), "clamps on last line");
    }

    #[test]
    fn editor_up_down_clamps_column() {
        let mut ed = LineEditor::new();
        ed.insert_str("abcdef");
        ed.insert_newline();
        ed.insert_str("xy");
        ed.move_up();
        assert_eq!(ed.cursor_line_col(), (0, 2));
        ed.end();
        assert_eq!(ed.cursor_line_col(), (0, 6));
        ed.move_down();
        assert_eq!(
            ed.cursor_line_col(),
            (1, 2),
            "column clamps to shorter line"
        );
    }

    #[test]
    fn composer_height_grows_with_lines() {
        let mut app = app();
        assert_eq!(app.composer_height(40), 3);
        app.input.insert_newline();
        assert_eq!(app.composer_height(40), 4);
        for _ in 0..10 {
            app.input.insert_newline();
        }
        assert_eq!(app.composer_height(40), 9, "caps at seven visible lines");
    }

    #[test]
    fn composer_height_caps_long_single_line_after_wrap() {
        let mut app = app();
        app.input.insert_str(&"a".repeat(400));
        assert_eq!(app.composer_height(40), 9, "long line wraps and caps at 9");
    }

    #[test]
    fn visual_rows_wrap_and_track_cursor() {
        let mut ed = LineEditor::new();
        ed.insert_str("0123456789");
        assert_eq!(rows(&mut ed, 4), "0123|4567|89");
        ed.insert_str("\nxy");
        assert_eq!(rows(&mut ed, 4), "0123|4567|89|xy");
        ed.move_left();
        assert_eq!(ed.cursor_visual(4), (3, 1));
    }

    #[test]
    fn visual_rows_cache_reuses_when_unchanged() {
        let mut ed = LineEditor::new();
        ed.insert_str("abc");
        assert_eq!(rows(&mut ed, 2), "ab|c");
        // Same width, no mutation: cache hit still returns the same rows.
        assert_eq!(rows(&mut ed, 2), "ab|c");
        // Mutation invalidates, and a new width re-wraps.
        ed.insert_char('d');
        assert_eq!(rows(&mut ed, 2), "ab|cd");
        assert_eq!(rows(&mut ed, 1), "a|b|c|d");
    }

    #[test]
    fn paste_over_ten_lines_becomes_chip() {
        let mut app = app();
        let big = "x\n".repeat(12);
        assert!(app.paste_text_input(&big));
        assert_eq!(app.pasted_text.len(), 1);
        assert_eq!(app.pasted_text[0].chars, big.chars().count());
        assert!(app.input.is_empty());
    }

    #[test]
    fn paste_short_goes_to_input() {
        let mut app = app();
        assert!(!app.paste_text_input("hello"));
        assert_eq!(app.input.text(), "hello");
        assert!(app.pasted_text.is_empty());
    }

    #[test]
    fn pasted_text_is_sent_and_cleared() {
        let mut app = app();
        app.paste_text_input(&"x\n".repeat(12));
        for c in "summarize".chars() {
            app.input.insert_char(c);
        }
        let prompt = app.submit_prompt().expect("prompt submitted");
        assert!(prompt.starts_with("summarize"), "prompt: {prompt}");
        assert!(prompt.contains("[pasted 24 chars]"), "marker: {prompt}");
        assert!(prompt.contains('x'), "body: {prompt}");
        assert!(app.pasted_text.is_empty(), "chip cleared after submit");
        assert!(matches!(
            app.messages.last(),
            Some(ChatEntry::User(text)) if text.contains("[pasted 24 chars]")
        ));
    }

    #[test]
    fn new_session_clears_pasted_text() {
        let mut app = app();
        app.paste_text_input(&"x\n".repeat(12));
        assert_eq!(app.pasted_text.len(), 1);
        app.new_session();
        assert!(app.pasted_text.is_empty());
    }

    #[test]
    fn large_paste_shows_a_chip() {
        let mut app = app();
        let base = app.composer_height(40);
        app.paste_text_input(&"x\n".repeat(12));
        assert_eq!(app.pasted_text.len(), 1, "one chip");
        assert_eq!(app.pasted_text[0].chars, 24);
        assert!(
            app.composer_height(40) > base,
            "composer grows to fit the paste chip"
        );
    }

    #[test]
    fn backspace_removes_paste_chip() {
        let mut app = app();
        app.paste_text_input(&"x\n".repeat(12));
        app.paste_text_input(&"y\n".repeat(12));
        assert_eq!(app.pasted_text.len(), 2);
        // Editor empty → backspace drops the last chip as one unit.
        app.backspace_input();
        assert_eq!(app.pasted_text.len(), 1, "last chip removed");
        assert_eq!(app.pasted_text[0].chars, 24, "first chip kept");
        app.backspace_input();
        assert!(app.pasted_text.is_empty());
        // With no chips left, backspace is a no-op on an empty editor.
        app.backspace_input();
        assert!(app.input.is_empty());
        assert!(app.pasted_text.is_empty());
    }

    #[test]
    fn paste_chip_content_is_sent() {
        let mut app = app();
        app.paste_text_input(&"x\n".repeat(12));
        for c in "summarize".chars() {
            app.input.insert_char(c);
        }
        let prompt = app.submit_prompt().expect("prompt submitted");
        assert!(prompt.contains("[pasted 24 chars]"), "marker: {prompt}");
        assert!(prompt.contains('x'), "body: {prompt}");
        assert!(app.pasted_text.is_empty(), "chip consumed on submit");
    }

    #[test]
    fn preview_opens_and_closes_within_bounds() {
        let mut app = app();
        assert!(app.preview.is_none());
        app.open_preview(0);
        assert!(app.preview.is_none(), "no attachment yet");
        app.attachments.push(Attachment {
            id: 7,
            path: "x.png".into(),
            name: "x.png".into(),
            width: 10,
            height: 20,
        });
        app.open_preview(0);
        assert_eq!(app.preview, Some(0));
        app.close_preview();
        assert!(app.preview.is_none());
    }

    #[test]
    fn remove_attachment_reindexes_preview() {
        let mut app = app();
        for n in 0..3 {
            app.attachments.push(Attachment {
                id: n,
                path: format!("{n}.png"),
                name: format!("{n}.png"),
                width: 10,
                height: 20,
            });
        }
        app.open_preview(2);
        assert_eq!(app.preview, Some(2));
        app.remove_attachment(0);
        assert_eq!(app.preview, Some(1));
        assert_eq!(app.attachments.len(), 2);
        app.remove_attachment(1);
        assert_eq!(app.preview, None);
    }

    #[test]
    fn attachment_ids_are_unique() {
        // from_path assigns ids from a monotonic counter, so two paths differ.
        let dir = std::env::temp_dir();
        let p1 = dir.join("evrax-id-a.png");
        let p2 = dir.join("evrax-id-b.png");
        let png = [
            0x89u8, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13, b'I', b'H', b'D', b'R',
            0, 0, 0, 1, 0, 0, 0, 1,
        ];
        std::fs::write(&p1, png).unwrap();
        std::fs::write(&p2, png).unwrap();
        let a = Attachment::from_path(p1.to_str().unwrap()).unwrap();
        let b = Attachment::from_path(p2.to_str().unwrap()).unwrap();
        assert_ne!(a.id, b.id);
        std::fs::remove_file(&p1).unwrap();
        std::fs::remove_file(&p2).unwrap();
    }

    #[test]
    fn status_events_update_flash_and_iteration() {
        let mut app = app();
        app.apply_event(AgentEvent::Iteration { n: 4, max: 500 });
        assert_eq!(app.iteration, Some((4, 500)));
        app.apply_event(AgentEvent::Retrying {
            attempt: 2,
            max: 10,
            delay_ms: 4_000,
            reason: "HTTP 429".into(),
        });
        assert!(
            app.flash.as_ref().unwrap().0.contains("retry 2/10"),
            "flash: {:?}",
            app.flash
        );
        app.apply_event(AgentEvent::Notice {
            message: "guardrail".into(),
        });
        assert_eq!(app.flash.as_ref().unwrap().0, "guardrail");
        app.apply_event(AgentEvent::TurnFinished);
        assert_eq!(app.iteration, None);
    }

    #[test]
    fn last_assistant_text_returns_latest_reply() {
        let mut app = app();
        type_and_submit(&mut app, "fix it").unwrap();
        app.apply_event(AgentEvent::TextDelta {
            text: "done".into(),
        });
        assert_eq!(app.last_assistant_text().as_deref(), Some("done"));
    }

    #[test]
    fn streaming_deltas_accumulate_into_one_entry() {
        let mut app = app();
        type_and_submit(&mut app, "fix the layout").unwrap();
        let before = app.messages.len();
        app.apply_event(AgentEvent::TextDelta { text: "Hel".into() });
        app.apply_event(AgentEvent::TextDelta { text: "lo".into() });
        assert_eq!(app.messages.len(), before + 1);
        assert!(matches!(
            app.messages.last(),
            Some(ChatEntry::Assistant(text)) if text == "Hello"
        ));
    }

    #[test]
    fn new_tool_breaks_assistant_stream() {
        let mut app = app();
        type_and_submit(&mut app, "fix the layout").unwrap();
        app.apply_event(AgentEvent::TextDelta { text: "a".into() });
        app.apply_event(AgentEvent::ToolStarted {
            name: "read".into(),
            summary: "file.rs".into(),
        });
        app.apply_event(AgentEvent::TextDelta { text: "b".into() });
        let tail: Vec<_> = app.messages.iter().rev().take(2).collect();
        assert!(matches!(tail[0], ChatEntry::Assistant(text) if text == "b"));
        assert!(matches!(tail[1], ChatEntry::Tool(_)));
    }

    #[test]
    fn tool_finished_updates_running_call() {
        let mut app = app();
        type_and_submit(&mut app, "fix the layout").unwrap();
        app.apply_event(AgentEvent::ToolStarted {
            name: "shell".into(),
            summary: "cargo test".into(),
        });
        app.apply_event(AgentEvent::ToolFinished {
            name: "shell".into(),
            ok: true,
            detail: "5 passed".into(),
        });
        assert!(matches!(
            app.messages.last(),
            Some(ChatEntry::Tool(tool))
                if matches!(tool.status, ToolStatus::Done) && tool.detail == "5 passed"
        ));
    }

    #[test]
    fn cancel_marks_running_tools_as_error() {
        let mut app = app();
        type_and_submit(&mut app, "fix the layout").unwrap();
        app.apply_event(AgentEvent::ToolStarted {
            name: "shell".into(),
            summary: "cargo test".into(),
        });
        app.cancel_turn();
        assert!(!app.busy);
        assert!(matches!(
            app.messages.last(),
            Some(ChatEntry::Tool(tool))
                if matches!(tool.status, ToolStatus::Error) && tool.detail == "cancelled"
        ));
    }

    #[test]
    fn turn_finished_clears_busy() {
        let mut app = app();
        type_and_submit(&mut app, "fix the layout").unwrap();
        app.apply_event(AgentEvent::TurnFinished);
        assert!(!app.busy);
        assert!(app.submit_prompt().is_none());
    }

    #[test]
    fn clamp_scroll_limits_to_content() {
        let mut app = app();
        app.scroll_view(500, true, u16::MAX);
        assert_eq!(app.scroll_from_bottom, 500);
        app.clamp_scroll(12);
        assert_eq!(app.scroll_from_bottom, 12);
        app.clamp_scroll(50);
        assert_eq!(app.scroll_from_bottom, 12);
    }

    #[test]
    fn scroll_saturates_at_bottom() {
        let mut app = app();
        app.set_viewport(20);
        assert_eq!(app.page(), 18);
        app.scroll_view(18, true, u16::MAX);
        assert_eq!(app.scroll_from_bottom, 18);
        app.scroll_view(100, false, u16::MAX);
        assert_eq!(app.scroll_from_bottom, 0);
    }

    #[test]
    fn selection_normalizes_drag_direction() {
        let mut sel = Selection::default();
        assert_eq!(sel.normalized(), None);
        sel.begin((10, 5));
        sel.update((3, 2));
        assert_eq!(sel.normalized(), Some((3, 2, 10, 5)));
        sel.finish();
        assert_eq!(sel.take_copy_request(), Some((3, 2, 10, 5)));
        assert_eq!(sel.take_copy_request(), None);
        sel.clear();
        assert_eq!(sel.normalized(), None);
    }

    #[test]
    fn click_without_drag_does_not_request_copy() {
        let mut sel = Selection::default();
        sel.begin((4, 4));
        sel.finish();
        assert_eq!(sel.take_copy_request(), None);
    }

    #[test]
    fn scroll_during_drag_keeps_anchor_on_content() {
        let mut app = app();
        app.selectable_area = Rect::new(0, 1, 80, 10);
        app.selection.begin((10, 5));
        app.scroll_view(3, true, 100);
        assert_eq!(app.scroll_from_bottom, 3);
        assert_eq!(app.selection.normalized(), Some((10, 5, 10, 8)));
        app.scroll_view(10, false, 100);
        assert_eq!(app.scroll_from_bottom, 0);
        assert_eq!(app.selection.normalized(), Some((10, 5, 10, 5)));
    }

    #[test]
    fn selection_edge_detects_drag_at_messages_border() {
        let mut app = app();
        app.selectable_area = Rect::new(0, 1, 80, 10);
        app.selection.begin((10, 5));
        assert_eq!(app.selection_edge(), None);
        app.selection.update((10, 1));
        assert_eq!(app.selection_edge(), Some(true));
        app.selection.update((10, 10));
        assert_eq!(app.selection_edge(), Some(false));
        app.selection.finish();
        assert_eq!(app.selection_edge(), None);
    }

    #[test]
    fn mode_cycles_between_build_and_plan() {
        let mut app = app();
        assert_eq!(app.mode, AgentMode::Build);
        app.cycle_mode();
        assert_eq!(app.mode, AgentMode::Plan);
        assert!(app.flash.is_some(), "mode switch should flash");
        app.cycle_mode();
        assert_eq!(app.mode, AgentMode::Build);
    }

    #[test]
    fn palette_filters_and_confirms_command() {
        let mut app = app();
        app.toggle_palette();
        assert!(app.palette.is_some());
        assert_eq!(app.filtered_commands().len(), 11);
        for c in "plan".chars() {
            app.palette_type(c);
        }
        let filtered = app.filtered_commands();
        assert_eq!(filtered.len(), 1, "expected one match for 'plan'");
        app.palette_confirm();
        assert!(app.palette.is_none(), "confirm should close palette");
        assert_eq!(app.mode, AgentMode::Plan);
    }

    #[test]
    fn palette_fuzzy_matches_subsequences() {
        let mut app = app();
        app.toggle_palette();
        for c in "sw".chars() {
            app.palette_type(c);
        }
        let filtered = app.filtered_commands();
        assert!(
            filtered
                .iter()
                .any(|(label, _)| label == "Switch to plan mode"),
            "'sw' should fuzzy-match Switch mode"
        );
        assert_eq!(
            filtered.len(),
            2,
            "'sw' matches Switch mode and Spawn workers"
        );
    }

    #[test]
    fn palette_selection_wraps_and_backspace_resets() {
        let mut app = app();
        app.toggle_palette();
        app.palette_move(-1);
        assert_eq!(app.palette.as_ref().unwrap().selected, 10);
        app.palette_move(1);
        assert_eq!(app.palette.as_ref().unwrap().selected, 0);
        app.palette_type('z');
        assert_eq!(app.filtered_commands().len(), 0);
        app.palette_backspace();
        assert_eq!(app.filtered_commands().len(), 11);
    }

    #[test]
    fn palette_quit_command_sets_should_quit() {
        let mut app = app();
        app.toggle_palette();
        for c in "quit".chars() {
            app.palette_type(c);
        }
        assert_eq!(app.filtered_commands().len(), 1);
        app.palette_confirm();
        assert!(app.should_quit);
    }

    #[test]
    fn slash_opens_filters_and_runs_command() {
        let mut app = app();
        app.input.insert_char('/');
        app.slash_sync();
        assert!(app.slash.is_some(), "leading slash should open the menu");
        assert!(app.filtered_slash().iter().any(|option| option.0 == "new"));
        for c in "se".chars() {
            app.input.insert_char(c);
        }
        app.slash_sync();
        let options = app.filtered_slash();
        assert!(
            options.iter().any(|option| option.0 == "sessions"),
            "fuzzy filter should keep sessions: {options:?}"
        );
        let selected = app.slash.as_ref().map(|menu| menu.selected).unwrap_or(0);
        let chosen = app.filtered_slash().get(selected).map(|option| option.0);
        assert!(app.slash_confirm(), "confirm should run the command");
        assert!(app.slash.is_none());
        assert!(
            app.session_dialog.is_some(),
            "sessions dialog should open (chosen={chosen:?})"
        );
        assert!(app.input.is_empty(), "command text should be cleared");
    }

    #[test]
    fn slash_dismiss_hides_until_text_changes() {
        let mut app = app();
        app.input.insert_char('/');
        app.slash_sync();
        assert!(app.slash.is_some());
        app.slash_dismiss();
        assert!(app.slash.is_none());
        app.slash_sync();
        assert!(
            app.slash.is_none(),
            "menu stays closed while text is unchanged"
        );
        app.input.insert_char('n');
        app.slash_sync();
        assert!(app.slash.is_some(), "typing should reopen the menu");
    }

    #[test]
    fn slash_closes_when_space_or_slash_typed() {
        let mut app = app();
        for c in "/new ".chars() {
            app.input.insert_char(c);
        }
        app.slash_sync();
        assert!(app.slash.is_none(), "space closes the menu");
        app.input.take();
        for c in "/a/b".chars() {
            app.input.insert_char(c);
        }
        app.slash_sync();
        assert!(app.slash.is_none(), "second slash closes the menu");
    }

    #[test]
    fn slash_theme_opens_theme_dialog() {
        let mut app = app();
        for c in "/theme".chars() {
            app.input.insert_char(c);
        }
        app.slash_sync();
        assert!(app.slash.is_some());
        assert!(app.slash_confirm(), "confirm should run the command");
        assert!(app.slash.is_none());
        assert!(app.theme_dialog.is_some(), "theme dialog should open");
        app.theme_close();
    }

    #[test]
    fn theme_dialog_previews_and_reverts() {
        let mut app = app();
        let original = theme::index();
        app.open_theme_dialog();
        assert!(app.theme_dialog.is_some());
        app.theme_move(1);
        assert_ne!(theme::index(), original, "preview should apply immediately");
        assert_eq!(theme::index(), (original + 1) % theme::count());
        app.theme_close();
        assert!(app.theme_dialog.is_none());
        assert_eq!(theme::index(), original, "esc should revert the preview");
    }

    #[test]
    fn theme_confirm_keeps_selection_and_flashes() {
        let mut app = app();
        let original = theme::index();
        app.open_theme_dialog();
        app.theme_move(1);
        app.theme_confirm();
        assert!(app.theme_dialog.is_none());
        assert_eq!(theme::index(), (original + 1) % theme::count());
        assert!(app.flash.is_some(), "confirm should flash the theme name");
        theme::set_index(original);
    }

    #[test]
    fn new_session_clears_conversation_and_input() {
        let mut app = app();
        type_and_submit(&mut app, "hello").unwrap();
        assert!(app.busy);
        app.new_session();
        assert!(app.messages.is_empty());
        assert!(!app.busy);
        assert_eq!(app.scroll_from_bottom, 0);
        assert!(app.input.is_empty());
    }

    #[test]
    fn approval_settles_and_auto_allows() {
        let mut app = app();
        app.apply_event(AgentEvent::ApprovalRequested {
            name: "edit".into(),
            summary: "src/ui.rs".into(),
            key: "edit".into(),
        });
        assert!(app.approval.is_some());
        app.approval_move(1);
        assert_eq!(
            app.approval.as_ref().unwrap().selected,
            1,
            "next moves to Always allow"
        );
        let allow = app.settle_approval(ApprovalDecision::Always);
        assert_eq!(allow, Some(true));
        assert!(app.auto_allow.contains("edit"));
        assert!(app.approval.is_none());

        app.apply_event(AgentEvent::ApprovalRequested {
            name: "edit".into(),
            summary: "src/ui.rs".into(),
            key: "edit".into(),
        });
        assert!(app.take_auto_approval(), "known tool auto-approves");
        assert!(app.approval.is_none());

        app.apply_event(AgentEvent::ApprovalRequested {
            name: "shell".into(),
            summary: "cargo test".into(),
            key: "shell".into(),
        });
        assert!(!app.take_auto_approval());
        assert!(app.approval.is_some());
        let allow = app.settle_approval(ApprovalDecision::Reject);
        assert_eq!(allow, Some(false));
    }

    #[test]
    fn auto_allow_is_scoped_to_the_approval_key() {
        let mut app = app();
        app.apply_event(AgentEvent::ApprovalRequested {
            name: "read".into(),
            summary: "C:/secrets/a.txt".into(),
            key: "read:C:/secrets".into(),
        });
        assert_eq!(app.settle_approval(ApprovalDecision::Always), Some(true));
        assert!(app.auto_allow.contains("read:C:/secrets"));

        app.apply_event(AgentEvent::ApprovalRequested {
            name: "read".into(),
            summary: "C:/other/b.txt".into(),
            key: "read:C:/other".into(),
        });
        assert!(
            !app.take_auto_approval(),
            "a different outside folder must still ask"
        );
        assert!(app.approval.is_some());
    }

    #[test]
    fn settle_remote_approval_maps_web_decisions() {
        let mut app = app();
        app.apply_event(AgentEvent::ApprovalRequested {
            name: "shell".into(),
            summary: "cargo test".into(),
            key: "shell".into(),
        });
        let id = app.approval.as_ref().expect("open dialog").id;
        assert!(id >= 1, "ids start at 1 so 0 means unspecified");
        assert_eq!(
            app.settle_remote_approval("bogus", id),
            None,
            "unknown ignored"
        );
        assert!(
            app.approval.is_some(),
            "unknown must not consume the dialog"
        );
        assert_eq!(
            app.settle_remote_approval("reject", id + 99),
            None,
            "stale id ignored"
        );
        assert!(
            app.approval.is_some(),
            "a decision for another dialog must not settle this one"
        );
        assert_eq!(app.settle_remote_approval("reject", id), Some(false));
        assert!(app.approval.is_none(), "decision consumed the dialog");

        app.apply_event(AgentEvent::ApprovalRequested {
            name: "shell".into(),
            summary: "cargo test".into(),
            key: "shell".into(),
        });
        let id2 = app.approval.as_ref().expect("second dialog").id;
        assert_ne!(id2, id, "every dialog gets a fresh id");
        assert_eq!(app.settle_remote_approval("always", id2), Some(true));
        assert!(app.auto_allow.contains("shell"));
        assert_eq!(
            app.settle_remote_approval("once", id),
            None,
            "the previous dialog's id stays dead"
        );
    }

    #[test]
    fn web_prompt_does_not_echo_user_but_external_does() {
        let mut app = app();
        app.ensure_remote();
        app.remote_log.clear();

        assert!(app.submit_web_prompt("halo dari web".into()).is_some());
        let kinds: Vec<&str> = app
            .remote_log
            .iter()
            .map(|e| match e {
                evrax_remote::RemoteEvent::User { .. } => "user",
                evrax_remote::RemoteEvent::TurnStart { .. } => "turn_start",
                _ => "other",
            })
            .collect();
        assert_eq!(
            kinds,
            vec!["turn_start"],
            "the server already echoed the web prompt's bubble"
        );

        app.busy = false;
        app.remote_log.clear();
        assert!(app.submit_external_prompt("dari inbox".into()).is_some());
        let kinds: Vec<&str> = app
            .remote_log
            .iter()
            .map(|e| match e {
                evrax_remote::RemoteEvent::User { .. } => "user",
                evrax_remote::RemoteEvent::TurnStart { .. } => "turn_start",
                _ => "other",
            })
            .collect();
        assert_eq!(
            kinds,
            vec!["user", "turn_start"],
            "worker prompts are echoed only by the TUI"
        );
    }

    #[test]
    fn remote_tool_events_carry_distinct_call_ids() {
        let mut app = app();
        app.ensure_remote();
        app.remote_log.clear();

        app.apply_event(AgentEvent::ToolStarted {
            name: "shell".into(),
            summary: "cargo test".into(),
        });
        app.apply_event(AgentEvent::ToolFinished {
            name: "shell".into(),
            ok: true,
            detail: "ok".into(),
        });
        app.apply_event(AgentEvent::ToolStarted {
            name: "shell".into(),
            summary: "cargo check".into(),
        });
        app.apply_event(AgentEvent::ToolFinished {
            name: "shell".into(),
            ok: true,
            detail: "ok".into(),
        });

        let calls: Vec<u64> = app
            .remote_log
            .iter()
            .filter_map(|e| match e {
                evrax_remote::RemoteEvent::Tool { call, .. } => Some(*call),
                _ => None,
            })
            .collect();
        assert_eq!(calls.len(), 4, "four tool frames: {calls:?}");
        assert!(
            calls[0] >= 1 && calls[0] == calls[1],
            "first run's running+done share one id: {calls:?}"
        );
        assert!(
            calls[2] > calls[1],
            "the second invocation gets a fresh id: {calls:?}"
        );
        assert_eq!(calls[2], calls[3], "second run shares its id: {calls:?}");

        // The transcript rows carry the same ids, one per invocation.
        let row_calls: Vec<u64> = app
            .messages
            .iter()
            .filter_map(|e| match e {
                ChatEntry::Tool(t) => Some(t.call),
                _ => None,
            })
            .collect();
        assert_eq!(row_calls, vec![calls[0], calls[2]], "rows: {row_calls:?}");
    }

    #[test]
    fn session_dialog_switches_sessions() {
        let mut app = app();
        app.new_session();
        app.open_sessions();
        let rows = app.session_rows();
        assert!(rows.len() >= 3, "rows: {rows:?}");
        let target_id = app.sessions[0].clone();
        app.session_dialog.as_mut().unwrap().selected = 2;
        app.sessions_confirm();
        assert!(app.session_dialog.is_none());
        assert_eq!(app.session_id, target_id);
        assert!(app.flash.is_some(), "switch should flash");
    }

    #[test]
    fn switching_sessions_saves_and_restores_transcripts() {
        let mut app = app();
        type_and_submit(&mut app, "halo").unwrap();
        assert!(!app.messages.is_empty());
        let old_id = app.session_id.clone();
        app.new_session();
        assert_ne!(app.session_id, old_id);
        assert!(app.messages.is_empty(), "new session starts empty");
        app.open_sessions();
        let idx = 2 + app.sessions.iter().position(|s| *s == old_id).unwrap();
        app.session_dialog.as_mut().unwrap().selected = idx;
        app.sessions_confirm();
        assert_eq!(app.session_id, old_id);
        assert!(!app.messages.is_empty(), "transcript replayed");
    }

    #[test]
    fn new_session_names_are_unique_and_keep_the_old_session() {
        let mut app = app();
        let first = app.session_id.clone();
        app.new_session();
        let second = app.session_id.clone();
        app.new_session();
        assert_ne!(first, second);
        assert_ne!(app.session_id, second);
        assert!(app.sessions.contains(&first));
        assert!(app.sessions.contains(&second));
    }

    #[test]
    fn placeholder_title_survives_session_switch() {
        let mut app = app();
        type_and_submit(&mut app, "perbaiki parser SSE");
        let first_id = app.session_id.clone();
        app.new_session();
        assert_eq!(app.title_of(&first_id), "perbaiki parser SSE");
        assert_eq!(app.display_name(), "new session");
        app.open_sessions();
        let idx = 2 + app.sessions.iter().position(|s| *s == first_id).unwrap();
        app.session_dialog.as_mut().unwrap().selected = idx;
        app.sessions_confirm();
        assert_eq!(app.session_id, first_id);
        assert_eq!(app.display_name(), "perbaiki parser SSE");
        assert!(!app.messages.is_empty());
    }

    #[test]
    fn slash_prompt_gets_no_placeholder() {
        let mut app = app();
        app.input.insert_str("/mode");
        app.submit_prompt();
        assert!(app.titles.is_empty());
    }

    #[test]
    fn title_from_prompt_strips_markdown_and_caps() {
        assert_eq!(
            title_from_prompt("# Refactor **ui.rs** sekarang juga sebelum kepecah").as_deref(),
            Some("Refactor ui.rs sekarang juga sebelum kepecah")
        );
        assert_eq!(title_from_prompt("/new halo").as_deref(), None);
        assert_eq!(
            title_from_prompt("   \n\nfix test").as_deref(),
            Some("fix test")
        );
        assert_eq!(title_from_prompt("a").as_deref(), None);
    }

    #[test]
    fn rename_is_manual_and_survives_late_llm_title() {
        let mut app = app();
        type_and_submit(&mut app, "perbaiki bug");
        app.rename_session_title("Pekerjaan Rilis v2");
        let id = app.session_id.clone();
        assert_eq!(
            app.titles.get(&id).map(String::as_str),
            Some("Pekerjaan Rilis v2")
        );
        assert!(app.manual_titles.contains(&id));
        app.apply_event(AgentEvent::TitleSuggested {
            session: id.clone(),
            title: "Perbaiki Bug Kecil".to_string(),
        });
        assert_eq!(
            app.titles.get(&id).map(String::as_str),
            Some("Pekerjaan Rilis v2")
        );
        assert_eq!(app.session_rows()[1], "Pekerjaan Rilis v2 (current)");
    }

    #[test]
    fn llm_title_replaces_placeholder_for_known_session() {
        let mut app = app();
        type_and_submit(&mut app, "perbaiki bug");
        let id = app.session_id.clone();
        app.apply_event(AgentEvent::TitleSuggested {
            session: id.clone(),
            title: "Perbaiki Bug Streaming".to_string(),
        });
        assert_eq!(
            app.titles.get(&id).map(String::as_str),
            Some("Perbaiki Bug Streaming")
        );
        app.apply_event(AgentEvent::TitleSuggested {
            session: "session-hantu".to_string(),
            title: "Tidak Relevan".to_string(),
        });
        assert!(!app.titles.contains_key("session-hantu"));
    }

    #[test]
    fn turn_finished_queues_one_title_job() {
        let mut app = app();
        type_and_submit(&mut app, "perbaiki bug");
        app.apply_event(AgentEvent::TextDelta {
            text: "selesai".to_string(),
        });
        assert!(
            app.take_title_job().is_none(),
            "no TurnFinished yet, no job"
        );
        app.apply_event(AgentEvent::TurnFinished);
        let job = app.take_title_job().expect("job should be queued");
        assert_eq!(job.0, app.session_id.clone());
        assert_eq!(job.1, "perbaiki bug");
        assert_eq!(job.2, "selesai");
        app.apply_event(AgentEvent::TurnFinished);
        assert!(app.take_title_job().is_none(), "job hanya sekali per sesi");
    }

    #[test]
    fn transcript_entries_are_capped() {
        let mut app = app();
        for i in 0..600 {
            app.apply_event(AgentEvent::TextDelta {
                text: format!("m{i}"),
            });
            app.asst_open = false;
        }
        assert!(
            app.messages.len() >= MAX_ENTRIES_PER_SESSION
                && app.messages.len() <= MAX_ENTRIES_PER_SESSION + 80,
            "len {}",
            app.messages.len()
        );
        match app.messages.first() {
            Some(ChatEntry::Assistant(text)) => {
                assert_ne!(text, "m0", "oldest entries must be evicted")
            }
            _ => panic!("expected assistant entry first"),
        }
    }

    #[test]
    fn transcripts_are_evicted_lru_beyond_cap() {
        let mut app = app();
        for _ in 0..(MAX_RETAINED_SESSIONS + 5) {
            app.new_session();
        }
        assert!(app.transcripts.len() <= MAX_RETAINED_SESSIONS);
        assert!(app.sessions.len() <= MAX_RETAINED_SESSIONS);
    }

    #[test]
    fn double_escape_cancels_within_window() {
        let mut app = app();
        assert!(!app.request_cancel(), "first esc only warns");
        assert!(app.flash.is_some());
        app.tick();
        assert!(app.request_cancel(), "second esc within window cancels");
        assert!(!app.request_cancel(), "guard resets after cancel");
        for _ in 0..30 {
            app.tick();
        }
        assert!(!app.request_cancel(), "expired esc warns again");
    }

    #[test]
    fn flash_expires_with_ticks() {
        let mut app = app();
        app.set_flash("copied".into());
        assert!(app.flash.is_some());
        for _ in 0..10 {
            app.tick();
        }
        assert!(app.flash.is_none());
    }

    #[test]
    fn submit_external_prompt_idle_then_busy() {
        let mut app = app();
        let p = app.submit_external_prompt("  hai  ".into());
        assert_eq!(p.as_deref(), Some("hai"));
        assert!(app.busy);
        assert!(matches!(app.messages.last(), Some(ChatEntry::User(t)) if t == "hai"));
        assert!(
            app.submit_external_prompt("next".into()).is_none(),
            "busy rejects"
        );
    }

    #[test]
    fn submit_external_prompt_rejects_blank() {
        let mut app = app();
        assert!(app.submit_external_prompt("   ".into()).is_none());
        assert!(!app.busy);
    }

    #[test]
    fn palette_page_clamps_to_bounds() {
        let mut app = app();
        app.toggle_palette();
        // 10 commands; page size 8 → down lands on row 8.
        app.palette_page(1, 8);
        assert_eq!(app.palette.as_ref().unwrap().selected, 8);
        // Up two pages of 8 â†’ still on the first row.
        app.palette_page(-1, 8);
        app.palette_page(-1, 8);
        assert_eq!(app.palette.as_ref().unwrap().selected, 0);
    }

    #[test]
    fn remote_edit_commit_validates_then_stores() {
        let mut app = app();
        app.open_remote_dialog();
        app.remote_dialog.as_mut().unwrap().selected = 2; // password row
        app.remote_toggle(); // begins editing
        assert!(app.remote_dialog.as_ref().unwrap().editing.is_some());

        app.remote_dialog.as_mut().unwrap().editing = Some("short".into());
        assert!(app.remote_edit_commit().is_err(), "min length enforced");
        assert!(
            app.remote_dialog.as_ref().unwrap().editing.is_some(),
            "kept on error"
        );

        app.remote_dialog.as_mut().unwrap().editing = Some("longenough".into());
        let cfg = app.remote_edit_commit().unwrap();
        assert_eq!(cfg.password, "longenough");
        assert_eq!(app.remote.as_ref().unwrap().config.password, "longenough");
        assert!(app.remote_dialog.as_ref().unwrap().editing.is_none());
    }

    #[test]
    fn remote_move_visits_interactive_rows_only() {
        let mut app = app();
        app.open_remote_dialog();
        let selected = |app: &App| app.remote_dialog.as_ref().unwrap().selected;
        assert_eq!(app.remote_row_count(), 8, "tunnel row is always shown");
        // Info rows (3/4/5/7) are display-only: 0 -> 1 -> 2 -> 6 -> 0.
        for row in [1usize, 2, 6, 0] {
            app.remote_move(1);
            assert_eq!(selected(&app), row, "row {row} reachable");
        }
        app.remote_move(-1);
        assert_eq!(selected(&app), 6, "backwards skips info rows too");
    }

    #[test]
    fn remote_info_rows_are_read_only() {
        let mut app = app();
        app.open_remote_dialog();
        // bind, port, url, session are display-only.
        for row in [3usize, 4, 5, 7] {
            app.remote_dialog.as_mut().unwrap().selected = row;
            app.remote_toggle();
            assert!(
                app.remote_dialog.as_ref().unwrap().editing.is_none(),
                "row {row} must not start editing"
            );
            let (flash, _) = app.flash.as_ref().expect("hint flashed");
            assert!(flash.contains("remote.json"), "row {row} flash: {flash}");
        }
    }

    #[test]
    fn remote_row_info_mapping_matches_toggle_actions() {
        let app = app();
        for row in 0..app.remote_row_count() {
            let expected = matches!(row, 3 | 4 | 5 | 7);
            assert_eq!(app.remote_row_is_info(row), expected, "row {row}");
        }
    }

    #[test]
    fn remote_display_url_resolves_wildcard_bind() {
        let mut app = app();
        app.open_remote_dialog();
        {
            let state = app.remote.as_mut().unwrap();
            state.config.bind = "0.0.0.0".into();
            state.config.port = 444;
        }
        let url = app.remote_display_url();
        assert!(url.starts_with("http://"), "{url}");
        assert!(!url.contains("0.0.0.0"), "wildcard bind leaked: {url}");
        assert!(url.ends_with(":444"), "{url}");

        app.remote.as_mut().unwrap().config.bind = "127.0.0.1".into();
        assert_eq!(app.remote_display_url(), "http://127.0.0.1:444");
        app.remote.as_mut().unwrap().config.bind = "10.0.0.7".into();
        assert_eq!(app.remote_display_url(), "http://10.0.0.7:444");
    }

    #[test]
    fn remote_edit_starts_empty_and_typing_replaces() {
        let mut app = app();
        app.open_remote_dialog();
        app.remote_dialog.as_mut().unwrap().selected = 2;
        app.remote_toggle();
        assert_eq!(
            app.remote_dialog.as_ref().unwrap().editing.as_deref(),
            Some(""),
            "edit starts from an empty buffer"
        );
        for c in "secret99".chars() {
            app.remote_edit_input(c);
        }
        app.remote_edit_backspace();
        assert_eq!(
            app.remote_dialog.as_ref().unwrap().editing.as_deref(),
            Some("secret9")
        );

        // Enter on an untouched buffer just stops editing.
        app.remote_dialog.as_mut().unwrap().editing = Some(String::new());
        assert_eq!(app.remote_edit_commit().unwrap_err(), "password unchanged");
        assert!(app.remote_dialog.as_ref().unwrap().editing.is_none());
    }

    #[test]
    fn tunnel_dialog_lists_actions_and_wraps() {
        let mut app = app();
        app.open_remote_dialog();
        app.remote_dialog.as_mut().unwrap().selected = 6;
        app.remote_toggle();
        assert!(app.tunnel_dialog.is_some(), "row 6 opens the tunnel menu");
        assert_eq!(app.tunnel_rows().len(), 5);
        app.tunnel_move(1);
        assert_eq!(app.tunnel_dialog.as_ref().unwrap().selected, 1);
        app.tunnel_move(-1);
        app.tunnel_move(-1);
        assert_eq!(
            app.tunnel_dialog.as_ref().unwrap().selected,
            4,
            "wraps to the last row"
        );
    }

    #[test]
    fn tunnel_quick_is_blocked_with_default_password() {
        let mut app = app();
        app.ensure_remote();
        {
            let cfg = &mut app.remote.as_mut().unwrap().config;
            cfg.password = "12345678".into();
            cfg.tunnel_mode.clear();
        }
        app.tunnel_dialog = Some(TunnelDialog {
            selected: 1,
            editing: None,
            kind: TunnelEditKind::ManualUrl,
        });
        assert!(matches!(app.tunnel_confirm(), TunnelEffect::None));
        let (flash, _) = app.flash.as_ref().expect("guard flash");
        assert!(flash.contains("password"), "flash: {flash}");
        assert!(
            app.remote.as_ref().unwrap().config.tunnel_mode.is_empty(),
            "mode untouched while blocked"
        );
    }

    #[test]
    fn tunnel_quick_sets_mode_and_follows_remote_switch() {
        let mut app = app();
        app.ensure_remote();
        app.remote.as_mut().unwrap().config.password = "longenough".into();
        app.tunnel_dialog = Some(TunnelDialog {
            selected: 1,
            editing: None,
            kind: TunnelEditKind::ManualUrl,
        });
        assert!(matches!(app.tunnel_confirm(), TunnelEffect::Start));
        {
            let state = app.remote.as_ref().unwrap();
            assert_eq!(state.config.tunnel_mode, "quick");
            assert!(state.enabled, "provider selection turns the remote on");
        }
        app.tunnel_close();
        // Remote OFF: no start, and any active tunnel is stopped.
        app.remote.as_mut().unwrap().enabled = false;
        assert!(matches!(app.tunnel_follow_remote(), TunnelEffect::None));
        // Remote ON again with a quick mode configured: start requested.
        app.remote.as_mut().unwrap().enabled = true;
        assert!(matches!(app.tunnel_follow_remote(), TunnelEffect::Start));
    }

    #[test]
    fn tunnel_manual_url_validates_and_clears() {
        let mut app = app();
        app.ensure_remote();
        {
            let cfg = &mut app.remote.as_mut().unwrap().config;
            cfg.public_url.clear();
            cfg.tunnel_mode.clear();
        }
        app.tunnel_dialog = Some(TunnelDialog {
            selected: 3,
            editing: None,
            kind: TunnelEditKind::ManualUrl,
        });
        app.tunnel_confirm();
        assert_eq!(
            app.tunnel_dialog.as_ref().unwrap().editing.as_deref(),
            Some(""),
            "manual editor starts from the current (empty) url"
        );
        for c in "ftp://x".chars() {
            app.tunnel_edit_input(c);
        }
        assert!(app.tunnel_edit_commit().is_err(), "only http(s) accepted");
        assert!(
            app.tunnel_dialog.as_ref().unwrap().editing.is_some(),
            "kept on error"
        );
        app.tunnel_dialog.as_mut().unwrap().editing = Some("https://evrax.example.com".into());
        assert!(matches!(
            app.tunnel_edit_commit(),
            Ok(TunnelEditResult::UrlSaved)
        ));
        {
            let cfg = &app.remote.as_ref().unwrap().config;
            assert_eq!(cfg.public_url, "https://evrax.example.com");
            assert_eq!(cfg.tunnel_mode, "manual");
        }
        // Empty commit clears the recorded URL and the manual mode.
        app.tunnel_edit_start();
        app.tunnel_dialog.as_mut().unwrap().editing = Some(String::new());
        assert!(matches!(
            app.tunnel_edit_commit(),
            Ok(TunnelEditResult::UrlCleared)
        ));
        {
            let cfg = &app.remote.as_ref().unwrap().config;
            assert!(cfg.public_url.is_empty());
            assert!(cfg.tunnel_mode.is_empty());
        }
    }

    #[test]
    fn tunnel_hostname_validates_and_readies_domain() {
        let mut app = app();
        app.ensure_remote();
        {
            let cfg = &mut app.remote.as_mut().unwrap().config;
            cfg.tunnel_hostname.clear();
            cfg.tunnel_mode.clear();
            cfg.password = "longenough".into();
        }
        app.tunnel_dialog = Some(TunnelDialog {
            selected: 2,
            editing: None,
            kind: TunnelEditKind::ManualUrl,
        });
        app.tunnel_confirm();
        assert_eq!(
            app.tunnel_dialog.as_ref().unwrap().editing.as_deref(),
            Some(""),
            "hostname editor starts from the current (empty) value"
        );
        assert_eq!(
            app.tunnel_dialog.as_ref().unwrap().kind,
            TunnelEditKind::Hostname
        );
        app.tunnel_dialog.as_mut().unwrap().editing = Some("localhost".into());
        assert!(app.tunnel_edit_commit().is_err(), "needs a dotted domain");
        assert!(
            app.tunnel_dialog.as_ref().unwrap().editing.is_some(),
            "kept on error"
        );
        // Pasted URLs are normalized down to the bare hostname.
        app.tunnel_dialog.as_mut().unwrap().editing = Some("https://Evrax.Example.com/".into());
        assert!(matches!(
            app.tunnel_edit_commit(),
            Ok(TunnelEditResult::HostReady)
        ));
        {
            let state = app.remote.as_ref().unwrap();
            assert_eq!(state.config.tunnel_hostname, "evrax.example.com");
            assert_eq!(state.config.tunnel_mode, "domain");
            assert!(state.enabled, "hostname commit turns the remote on");
        }
        // Follow: unresolved process with a configured domain asks for a start.
        app.remote.as_mut().unwrap().enabled = true;
        assert!(matches!(app.tunnel_follow_remote(), TunnelEffect::Start));
        // Domain without hostname only flashes.
        app.remote.as_mut().unwrap().config.tunnel_hostname.clear();
        assert!(matches!(app.tunnel_follow_remote(), TunnelEffect::None));
        let (flash, _) = app.flash.as_ref().expect("hint flashed");
        assert!(flash.contains("hostname"), "flash: {flash}");
    }

    #[test]
    fn tunnel_hostname_commit_refused_with_weak_password() {
        for weak in ["12345678", "", "short", "   "] {
            let mut app = app();
            app.ensure_remote();
            {
                let cfg = &mut app.remote.as_mut().unwrap().config;
                cfg.password = weak.into();
                cfg.tunnel_hostname.clear();
                cfg.tunnel_mode.clear();
            }
            app.tunnel_dialog = Some(TunnelDialog {
                selected: 2,
                editing: Some("evrax.example.com".into()),
                kind: TunnelEditKind::Hostname,
            });
            let err = app.tunnel_edit_commit().unwrap_err();
            assert!(err.contains("password"), "{weak:?} err: {err}");
            assert!(
                app.tunnel_dialog.as_ref().unwrap().editing.is_some(),
                "{weak:?}: editor must stay open"
            );
            let cfg = &app.remote.as_ref().unwrap().config;
            assert!(
                cfg.tunnel_mode.is_empty(),
                "{weak:?}: mode must stay untouched"
            );
            assert!(
                cfg.tunnel_hostname.is_empty(),
                "{weak:?}: hostname not saved"
            );
            assert!(
                !app.remote.as_ref().unwrap().enabled,
                "{weak:?}: remote stays off"
            );
        }
    }

    #[test]
    fn remote_toggle_enable_refused_with_weak_password() {
        let mut app = app();
        app.open_remote_dialog();
        app.remote.as_mut().unwrap().config.password = "12345678".into();
        app.remote.as_mut().unwrap().enabled = false;
        app.remote_dialog.as_mut().unwrap().selected = 0;
        app.remote_toggle();
        assert!(!app.remote.as_ref().unwrap().enabled, "must stay off");
        let (flash, _) = app.flash.as_ref().expect("guard flash");
        assert!(flash.contains("password"), "flash: {flash}");

        // A real password lets the switch turn on.
        app.remote.as_mut().unwrap().config.password = "longenough".into();
        app.remote_toggle();
        assert!(
            app.remote.as_ref().unwrap().enabled,
            "strong password enables"
        );

        // Turning it OFF always works, even after the password turns weak.
        app.remote.as_mut().unwrap().config.password = "12345678".into();
        app.remote_toggle();
        assert!(
            !app.remote.as_ref().unwrap().enabled,
            "disable is never blocked"
        );
    }

    #[test]
    fn tunnel_follow_remote_waits_for_weak_password() {
        let mut app = app();
        app.ensure_remote();
        {
            let state = app.remote.as_mut().unwrap();
            state.enabled = true;
            state.config.tunnel_mode = "quick".into();
            state.config.password = "12345678".into();
        }
        assert!(matches!(app.tunnel_follow_remote(), TunnelEffect::None));
        let (flash, _) = app.flash.as_ref().expect("waiting flash");
        assert!(flash.contains("password"), "flash: {flash}");
        assert!(
            !app.remote.as_ref().unwrap().tunnel.is_active(),
            "no process started"
        );
    }

    #[test]
    fn tunnel_rows_mark_password_requirement() {
        let mut app = app();
        app.ensure_remote();
        app.remote.as_mut().unwrap().config.password = "12345678".into();
        let rows = app.tunnel_rows();
        assert!(rows[1].contains("password required"), "rows: {rows:?}");
        assert!(rows[2].contains("password required"), "rows: {rows:?}");
        assert!(!rows[0].contains("password required"));
        assert!(!rows[3].contains("password required"));
        app.remote.as_mut().unwrap().config.password = "longenough".into();
        let rows = app.tunnel_rows();
        assert!(
            rows.iter().all(|r| !r.contains("password required")),
            "rows: {rows:?}"
        );
    }

    #[test]
    fn remote_edit_commit_rejects_default_and_blank_passwords() {
        let mut app = app();
        app.open_remote_dialog();
        // App::new() may seed a different default; force the shipped default
        // to exercise the rejection path deterministically.
        app.remote.as_mut().unwrap().config.password = "12345678".into();
        app.remote_dialog.as_mut().unwrap().selected = 2;
        app.remote_toggle();
        app.remote_dialog.as_mut().unwrap().editing = Some("12345678".into());
        let err = app.remote_edit_commit().unwrap_err();
        assert!(err.contains("default is not allowed"), "err: {err}");
        assert_eq!(app.remote.as_ref().unwrap().config.password, "12345678");

        app.remote_dialog.as_mut().unwrap().editing = Some("        ".into());
        let err = app.remote_edit_commit().unwrap_err();
        assert!(err.contains("default is not allowed"), "err: {err}");
        assert!(
            app.remote_dialog.as_ref().unwrap().editing.is_some(),
            "kept on error"
        );
    }
    #[test]
    fn sessions_persist_and_resume_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        let store =
            evrax_store::ProjectStore::open_in(dir.path(), &dir.path().join("data")).unwrap();
        let mut app = App::new();
        app.attach_project(store.clone());
        let id = app.session_id.clone();
        app.push_entry(ChatEntry::User("halo".into()));
        app.push_entry(ChatEntry::Assistant("hai".into()));
        app.save_current();

        let mut resumed = App::new();
        resumed.attach_project(store);
        assert!(resumed.resume(&id));
        assert_eq!(resumed.messages.len(), 2);
        assert!(matches!(resumed.messages[0], ChatEntry::User(_)));
        assert!(resumed.sessions.contains(&id));
    }

    #[test]
    fn empty_sessions_are_not_written() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        let store =
            evrax_store::ProjectStore::open_in(dir.path(), &dir.path().join("data")).unwrap();
        let mut app = App::new();
        app.attach_project(store.clone());
        app.save_current();
        assert!(store.list().unwrap().is_empty());
    }
    fn wizard_at(step: WizardStep) -> App {
        let mut app = App::new();
        app.open_wizard();
        if let Some(wizard) = app.wizard.as_mut() {
            wizard.step = step;
            wizard.editing = false;
        }
        app
    }

    #[test]
    fn wizard_startswith_providers_when_any_saved() {
        let mut app = App::new();
        app.providers = vec![evrax_agent::Provider::new(
            "9router",
            "http://127.0.0.1:20128/v1",
        )];
        app.open_wizard();
        assert_eq!(app.wizard_step(), Some(WizardStep::Providers));
        let mut fresh = App::new();
        fresh.open_wizard();
        assert_eq!(fresh.wizard_step(), Some(WizardStep::Kind));
    }

    #[test]
    fn wizard_presets_fill_base_url() {
        let mut app = wizard_at(WizardStep::BaseUrl);
        app.wizard_preset(1);
        let wizard = app.wizard.as_ref().unwrap();
        assert_eq!(wizard.base_url, "https://openrouter.ai/api/v1");
        assert_eq!(wizard.name, "OpenRouter");
    }

    #[test]
    fn wizard_settings_trims_and_drops_empty_key() {
        let mut app = wizard_at(WizardStep::Review);
        if let Some(wizard) = app.wizard.as_mut() {
            wizard.name = "  http://x/v1  ".into();
            wizard.base_url = " http://x/v1 ".into();
            wizard.model = " m ".into();
            wizard.api_key = "   ".into();
        }
        let settings = app.wizard_settings().unwrap();
        assert_eq!(settings.active, "http://x/v1");
        assert_eq!(settings.providers[0].base_url, "http://x/v1");
        assert_eq!(settings.providers[0].model, "m");
        assert!(settings.providers[0].api_key.is_none());
    }

    #[test]
    fn wizard_api_key_step_requests_model_list() {
        let mut app = wizard_at(WizardStep::ApiKey);
        if let Some(wizard) = app.wizard.as_mut() {
            wizard.name = "Nimbus Go".into();
            wizard.base_url = "https://api.example.com/v1".into();
            wizard.api_key = " secret ".into();
        }
        let WizardAction::FetchModels(request) = app.wizard_enter() else {
            panic!("expected FetchModels");
        };
        assert_eq!(request.base_url, "https://api.example.com/v1");
        assert_eq!(request.api_key.as_deref(), Some("secret"));
        assert_eq!(app.wizard_step(), Some(WizardStep::Checking));
    }

    #[test]
    fn wizard_fetched_models_are_filterable_and_saved() {
        let mut app = wizard_at(WizardStep::Checking);
        if let Some(wizard) = app.wizard.as_mut() {
            wizard.name = "Nimbus Go".into();
            wizard.base_url = "http://x/v1".into();
            wizard.api_key = "k".into();
            wizard.request_id = 1;
        }
        app.wizard_models_ready(
            1,
            Ok(vec![
                "deepseek-chat".into(),
                "gpt-5-mini".into(),
                "bebas".into(),
            ]),
        );
        assert_eq!(app.wizard_step(), Some(WizardStep::Models));
        assert_eq!(app.wizard.as_ref().unwrap().filtered.len(), 3);
        for c in "gpt".chars() {
            app.wizard_type(c);
        }
        assert_eq!(app.wizard.as_ref().unwrap().filtered.len(), 1);
        app.wizard_enter();
        assert_eq!(app.wizard_step(), Some(WizardStep::Review));
        let WizardAction::Save(settings) = app.wizard_enter() else {
            panic!("expected Save");
        };
        assert_eq!(settings.providers[0].model, "gpt-5-mini");
        assert_eq!(settings.active, "Nimbus Go");
    }

    #[test]
    fn wizard_keeps_modal_open_until_saved_and_upserts_existing() {
        let mut app = wizard_at(WizardStep::Review);
        app.providers = vec![evrax_agent::Provider {
            name: "Muse Spark".into(),
            kind: evrax_agent::ProviderKind::Anthropic,
            base_url: "http://b/v1".into(),
            api_key: Some("old".into()),
            model: "old-model".into(),
        }];
        if let Some(wizard) = app.wizard.as_mut() {
            wizard.name = "Muse Spark".into();
            wizard.kind = evrax_agent::ProviderKind::Anthropic;
            wizard.base_url = "http://b/v1".into();
            wizard.api_key = "new".into();
            wizard.model = "claude-x".into();
        }
        assert!(matches!(app.wizard_enter(), WizardAction::Save(_)));
        assert!(app.wizard.is_some());
        let settings = app.wizard_settings().unwrap();
        assert_eq!(settings.providers.len(), 1, "same name updates in place");
        assert_eq!(settings.providers[0].api_key.as_deref(), Some("new"));
    }

    #[test]
    fn wizard_escape_steps_back_then_closes() {
        let mut app = wizard_at(WizardStep::Models);
        assert!(!app.wizard_escape());
        assert_eq!(app.wizard_step(), Some(WizardStep::ApiKey));
        let mut app = wizard_at(WizardStep::Providers);
        assert!(app.wizard_escape());
    }

    #[test]
    fn model_dialog_fetches_then_applies_settings() {
        let mut app = App::new();
        app.providers = vec![
            evrax_agent::Provider::new("Nimbus Go", "http://a/v1"),
            evrax_agent::Provider {
                name: "Muse Spark".into(),
                kind: evrax_agent::ProviderKind::Anthropic,
                base_url: "http://b/v1".into(),
                api_key: Some("k".into()),
                model: "old".into(),
            },
        ];
        app.active_provider = Some("Nimbus Go".into());
        app.open_model_dialog();
        app.model_move(1);
        let ModelAction::FetchModels(request) = app.model_enter() else {
            panic!("expected FetchModels");
        };
        assert_eq!(request.kind, evrax_agent::ProviderKind::Anthropic);
        assert_eq!(request.api_key.as_deref(), Some("k"));
        app.model_ready(
            request.request_id,
            Ok(vec!["claude-x".into(), "claude-y".into()]),
        );
        assert!(!app.model_dialog.as_ref().unwrap().loading);
        let ModelAction::Apply(settings) = app.model_enter() else {
            panic!("expected Apply");
        };
        assert_eq!(settings.active, "Muse Spark");
        assert_eq!(settings.providers[1].model, "claude-x");
    }

    #[test]
    fn model_dialog_falls_back_to_typed_model() {
        let mut app = App::new();
        app.providers = vec![evrax_agent::Provider::new("Nimbus Go", "http://a/v1")];
        app.open_model_dialog();
        let ModelAction::FetchModels(request) = app.model_enter() else {
            panic!("expected FetchModels");
        };
        app.model_ready(request.request_id, Err("404 no /models".into()));
        for c in "custom-model".chars() {
            app.model_type(c);
        }
        let ModelAction::Apply(settings) = app.model_enter() else {
            panic!("expected Apply");
        };
        assert_eq!(settings.providers[0].model, "custom-model");
        assert_eq!(settings.active, "Nimbus Go");
    }

    #[test]
    fn reload_config_is_taken_once() {
        let mut app = App::new();
        app.queue_backend_reload(evrax_agent::AgentConfig {
            model: "m".into(),
            ..Default::default()
        });
        assert_eq!(app.take_reload_config().unwrap().model, "m");
        assert!(app.take_reload_config().is_none());
    }

    #[test]
    fn update_check_explicit_up_to_date_flashes() {
        let mut app = app();
        app.apply_update_check(true, Ok(None));
        let flash = app.flash.as_ref().map(|(text, _)| text.clone());
        assert!(
            flash.as_deref().is_some_and(|t| t.contains("up to date")),
            "explicit up-to-date must flash: {flash:?}"
        );
    }

    #[test]
    fn update_check_startup_error_stays_quiet() {
        let mut app = app();
        app.apply_update_check(false, Err("boom".into()));
        assert!(app.flash.is_none(), "startup error must not flash");
    }

    #[test]
    fn update_check_startup_available_flashes() {
        let mut app = app();
        app.apply_update_check(
            false,
            Ok(Some(evrax_agent::update::UpdateCheck {
                latest: "v9.9.9".into(),
                url: "https://x".into(),
            })),
        );
        let flash = app.flash.as_ref().map(|(text, _)| text.clone());
        assert!(
            flash
                .as_deref()
                .is_some_and(|t| t.contains("update available")),
            "startup update must flash: {flash:?}"
        );
    }

    #[test]
    fn update_check_explicit_error_flashes() {
        let mut app = app();
        app.apply_update_check(true, Err("boom".into()));
        let flash = app.flash.as_ref().map(|(text, _)| text.clone());
        assert!(
            flash
                .as_deref()
                .is_some_and(|t| t.contains("update check failed")),
            "explicit error must flash: {flash:?}"
        );
    }

    #[test]
    fn slash_has_update_command() {
        assert!(
            slash_options().iter().any(|option| option.0 == "update"),
            "slash menu must offer /update"
        );
    }

    #[test]
    fn update_job_is_none_while_inflight() {
        let mut app = app();
        app.request_update_check();
        assert_eq!(app.update_job(), Some(true));
        assert_eq!(
            app.update_job(),
            None,
            "in-flight check blocks a second spawn"
        );
        app.apply_update_check(true, Ok(None));
        app.request_update_check();
        assert_eq!(app.update_job(), Some(true), "check can be requested again");
    }

    #[test]
    fn update_is_read_only_while_streaming() {
        let mut app = app();
        type_and_submit(&mut app, "hello");
        assert!(app.busy);
        app.flash = None;
        app.execute_command(Command::Update);
        let flash = app.flash.as_ref().map(|(text, _)| text.clone());
        assert!(
            flash
                .as_deref()
                .is_some_and(|t| !t.contains("not available while streaming")),
            "/update must work while streaming: {flash:?}"
        );
    }

    #[test]
    fn slash_has_worker_command() {
        let names: Vec<&str> = SLASH_COMMANDS.iter().map(|entry| entry.name).collect();
        assert!(names.contains(&"worker"));
        let worker = SLASH_COMMANDS
            .iter()
            .find(|entry| entry.name == "worker")
            .expect("worker entry");
        assert!(worker.aliases.contains(&"spawn"));
    }

    #[test]
    fn spawn_workers_clamps_and_names() {
        let mut wide = app();
        wide.spawn_workers(5);
        assert_eq!(wide.workers.len(), MAX_WORKERS);
        assert!(wide.workers.iter().all(|pane| pane.name.contains("worker")));
        assert!(wide.workers.iter().all(|pane| pane.status == "starting"));

        let mut fresh = app();
        fresh.spawn_workers(0);
        assert_eq!(fresh.workers.len(), 1);
    }

    #[test]
    fn spawn_workers_requests_process_spawn() {
        let mut app = app();
        app.spawn_workers(2);
        assert_eq!(app.take_worker_spawn_request(), Some(2));
        assert_eq!(app.take_worker_spawn_request(), None);
    }

    #[test]
    fn spawn_workers_blocked_while_streaming() {
        let mut app = app();
        type_and_submit(&mut app, "hello");
        assert!(app.busy);
        app.flash = None;
        app.spawn_workers(1);
        assert!(app.workers.is_empty(), "no panes while streaming");
        assert_eq!(app.take_worker_spawn_request(), None);
        let flash = app.flash.as_ref().map(|(text, _)| text.clone());
        assert_eq!(flash.as_deref(), Some("not available while streaming"));
    }

    #[test]
    fn new_session_clears_workers() {
        let mut app = app();
        app.spawn_workers(2);
        assert!(!app.workers.is_empty());
        app.new_session();
        assert!(app.workers.is_empty());
    }

    #[test]
    fn slash_has_workers_command() {
        let names: Vec<&str> = SLASH_COMMANDS.iter().map(|entry| entry.name).collect();
        assert!(names.contains(&"workers"), "slash menu must offer /workers");
    }

    #[test]
    fn workers_command_reports_empty_project() {
        let mut app = app();
        app.flash = None;
        app.workers_command("");
        let flash = app.flash.as_ref().map(|(text, _)| text.clone());
        assert!(
            flash.as_deref().is_some_and(|t| t.contains("no workers")),
            "empty project should say so: {flash:?}"
        );
    }

    #[test]
    fn workers_command_rejects_unknown_argument() {
        let mut app = app();
        app.flash = None;
        app.workers_command("bogus");
        let flash = app.flash.as_ref().map(|(text, _)| text.clone());
        assert!(
            flash.as_deref().is_some_and(|t| t.contains("unknown")),
            "unknown arg should flash: {flash:?}"
        );
    }

    #[test]
    fn workers_kill_requires_id() {
        let mut app = app();
        app.flash = None;
        app.workers_command("kill");
        let flash = app.flash.as_ref().map(|(text, _)| text.clone());
        assert!(
            flash.as_deref().is_some_and(|t| t.contains("usage")),
            "kill without id should show usage: {flash:?}"
        );
    }

    #[test]
    fn resume_rebuilds_worker_panes() {
        use evrax_agent::worker::{State, WorkerStatus};
        let states = vec![
            State {
                id: "worker-1".into(),
                name: "one".into(),
                pid: 42,
                status: WorkerStatus::Busy,
                root: "/proj".into(),
                session: "session-a".into(),
                updated_ms: 1234,
                last_reply: "done".into(),
            },
            State {
                id: "worker-2".into(),
                name: "two".into(),
                pid: 0,
                status: WorkerStatus::Stopped,
                root: "/proj".into(),
                session: "session-a".into(),
                updated_ms: 1234,
                last_reply: String::new(),
            },
        ];
        let panes = worker_panes_from(states);
        assert_eq!(panes.len(), 2, "both workers rebuilt as panes");
        assert_eq!(panes[0].id, "worker-1");
        assert_eq!(panes[0].status, "busy");
        assert_eq!(panes[0].pid, 42);
        assert_eq!(panes[0].last_reply, "done");
        assert_eq!(panes[1].status, "stopped", "dead worker shows stopped");

        // Wiring: resume() rebuilds from the same mapping.
        let mut app = app();
        app.workers.push(WorkerPane {
            id: "stale".into(),
            name: "stale".into(),
            status: "idle".into(),
            pid: 0,
            transcript: Vec::new(),
            last_reply: String::new(),
            cache: crate::ui::TranscriptCache::default(),
            events_seen: 0,
        });
        app.restore_worker_panes("session-a");
        assert!(
            app.workers.is_empty(),
            "resume drops panes not backed by this session's state on disk"
        );
    }
}
