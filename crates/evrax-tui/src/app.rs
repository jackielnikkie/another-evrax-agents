use std::collections::HashSet;

use evrax_agent::catalog::Catalog;
use evrax_core::{AgentEvent, DiffLine};
use evrax_store::{ProjectStore, SessionMeta};
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use ratatui::layout::Rect;

use crate::theme;

pub const ESC_CANCEL_TICKS: u64 = 20;
pub const APPROVAL_OPTIONS: [&str; 3] = ["Allow once", "Always allow", "Reject"];

pub struct App {
    pub should_quit: bool,
    pub tick: u64,
    pub cwd: String,
    pub messages: Vec<ChatEntry>,
    pub input: LineEditor,
    pub busy: bool,
    /// Current tool-calling iteration inside the running turn (`n/max`).
    pub iteration: Option<(usize, usize)>,
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
    pub preview: Option<usize>,
    pub pending_kitty: Vec<String>,
    pub chip_rects: Vec<Rect>,
    pub chip_x_rects: Vec<Rect>,
    pub wizard: Option<Wizard>,
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

pub struct Wizard {
    pub row: usize,
    pub editing: bool,
    pub provider: usize,
    pub base_url: String,
    pub model: String,
    pub api_key: String,
    pub status: Option<String>,
    pub testing: bool,
}

pub enum WizardAction {
    None,
    Test(evrax_agent::AgentConfig),
    Save(evrax_agent::AgentConfig),
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
    Remote,
    Rename,
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

pub struct Attachment {
    pub id: u32,
    pub path: String,
    pub name: String,
    pub width: u32,
    pub height: u32,
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
}

#[derive(serde::Serialize, serde::Deserialize)]
pub enum ToolStatus {
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
}

impl LineEditor {
    pub fn new() -> Self {
        Self {
            chars: Vec::new(),
            cursor: 0,
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
    }

    pub fn insert_str(&mut self, text: &str) {
        let chars: Vec<char> = text.chars().collect();
        self.chars.splice(self.cursor..self.cursor, chars);
        self.cursor += text.chars().count();
    }

    pub fn insert_newline(&mut self) {
        self.insert_char('\n');
    }

    pub fn visual_rows(&self, width: usize) -> Vec<String> {
        let width = width.max(1);
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
        rows
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
        }
    }

    pub fn delete(&mut self) {
        if self.cursor < self.chars.len() {
            self.chars.remove(self.cursor);
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
            messages: Vec::new(),
            input: LineEditor::new(),
            busy: false,
            iteration: None,
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
            preview: None,
            pending_kitty: Vec::new(),
            chip_rects: Vec::new(),
            chip_x_rects: Vec::new(),
            wizard: None,
            session_id: unique_session_id(),
            sessions: Vec::new(),
            store: None,
            transcripts: std::collections::HashMap::new(),
            titles: std::collections::HashMap::new(),
            manual_titles: std::collections::HashSet::new(),
            title_pending: std::collections::HashSet::new(),
            title_job: None,
            transcript: crate::ui::TranscriptCache::default(),
            content_rev: 0,
            last_esc: None,
            asst_open: false,
        }
    }

    pub fn open_wizard(&mut self, cfg: &evrax_agent::AgentConfig) {
        self.wizard = Some(Wizard {
            row: 0,
            editing: false,
            provider: 0,
            base_url: cfg.base_url.clone(),
            model: cfg.model.clone(),
            api_key: cfg.api_key.clone().unwrap_or_default(),
            status: None,
            testing: false,
        });
    }

    pub fn wizard_row(&self) -> usize {
        self.wizard.as_ref().map(|w| w.row).unwrap_or(0)
    }

    pub fn wizard_config(&self) -> Option<evrax_agent::AgentConfig> {
        let wizard = self.wizard.as_ref()?;
        Some(evrax_agent::AgentConfig {
            base_url: wizard.base_url.trim().to_string(),
            model: wizard.model.trim().to_string(),
            api_key: Some(wizard.api_key.trim().to_string()).filter(|key| !key.is_empty()),
            ..evrax_agent::AgentConfig::default()
        })
    }

    pub fn wizard_move(&mut self, delta: i32) {
        if let Some(wizard) = self.wizard.as_mut() {
            wizard.editing = false;
            wizard.row = ((wizard.row as i32 + delta).rem_euclid(6)) as usize;
        }
    }

    pub fn wizard_preset(&mut self, delta: i32) {
        if let Some(wizard) = self.wizard.as_mut() {
            let count = WIZARD_PRESETS.len() as i32;
            wizard.provider = ((wizard.provider as i32 + delta).rem_euclid(count)) as usize;
            let (_, url, model) = WIZARD_PRESETS[wizard.provider];
            if !url.is_empty() {
                wizard.base_url = url.to_string();
            }
            if !model.is_empty() {
                wizard.model = model.to_string();
            }
        }
    }

    pub fn wizard_enter(&mut self) -> WizardAction {
        let Some(row) = self.wizard.as_ref().map(|wizard| wizard.row) else {
            return WizardAction::None;
        };
        if self.wizard.as_ref().is_some_and(|wizard| wizard.editing) {
            if let Some(wizard) = self.wizard.as_mut() {
                wizard.editing = false;
            }
            return WizardAction::None;
        }
        match row {
            0 => {
                self.wizard_preset(1);
                WizardAction::None
            }
            1..=3 => {
                if let Some(wizard) = self.wizard.as_mut() {
                    wizard.editing = true;
                }
                WizardAction::None
            }
            4 => {
                let cfg = self.wizard_config();
                if let Some(wizard) = self.wizard.as_mut() {
                    wizard.testing = true;
                    wizard.status = Some("testing...".to_string());
                }
                cfg.map(WizardAction::Test).unwrap_or(WizardAction::None)
            }
            _ => {
                let cfg = self.wizard_config();
                self.wizard = None;
                cfg.map(WizardAction::Save).unwrap_or(WizardAction::None)
            }
        }
    }

    pub fn wizard_escape(&mut self) -> bool {
        if let Some(wizard) = self.wizard.as_mut()
            && wizard.editing
        {
            wizard.editing = false;
            return true;
        }
        false
    }

    pub fn wizard_type(&mut self, c: char) {
        if let Some(wizard) = self.wizard.as_mut() {
            match wizard.row {
                1 => wizard.base_url.push(c),
                2 => wizard.model.push(c),
                3 => wizard.api_key.push(c),
                _ => {}
            }
        }
    }

    pub fn wizard_backspace(&mut self) {
        if let Some(wizard) = self.wizard.as_mut() {
            match wizard.row {
                1 => {
                    wizard.base_url.pop();
                }
                2 => {
                    wizard.model.pop();
                }
                3 => {
                    wizard.api_key.pop();
                }
                _ => {}
            }
        }
    }

    pub fn wizard_status(&mut self, status: impl Into<String>, testing: bool) {
        if let Some(wizard) = self.wizard.as_mut() {
            wizard.status = Some(status.into());
            wizard.testing = testing;
        }
    }

    pub fn wizard_close(&mut self) {
        self.wizard = None;
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
        self.asst_open = false;
        self.busy = false;
        self.scroll_from_bottom = 0;
        self.transcript.invalidate();
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
            ("Remote web".to_string(), Command::Remote),
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
            Command::Remote => self.open_remote_dialog(),
            Command::Rename => self.rename_session_title(""),
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
        let open = query.is_some() && !dismissed && !self.busy;
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
        self.asst_open = false;
        self.busy = false;
        self.scroll_from_bottom = 0;
        self.selection.clear();
        self.transcript.invalidate();
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
        vec![
            "stop tunnel".to_string(),
            "quick tunnel (trycloudflare.com)".to_string(),
            "custom domain…".to_string(),
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
                    self.set_flash("tunnel: belum ada url".to_string());
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
            .map(|s| evrax_remote::is_default_password(&s.config.password))
            .unwrap_or(true);
        if blocked {
            self.set_flash("ganti password dulu (min 8) sebelum pakai tunnel".to_string());
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
                        return Err("url harus mulai dengan http:// atau https://".into());
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
                    return Err("hostname tidak valid (contoh: evrax.example.com)".into());
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
            evrax_remote::is_default_password(&state.config.password),
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
            self.set_flash("tunnel menunggu · ganti password dulu (min 8)".to_string());
            return TunnelEffect::None;
        }
        if active {
            return TunnelEffect::None;
        }
        match mode.as_str() {
            "quick" => TunnelEffect::Start,
            "domain" if !hostname.is_empty() => TunnelEffect::Start,
            "domain" => {
                self.set_flash("custom domain: isi hostname dulu (menu tunnel)".to_string());
                TunnelEffect::None
            }
            _ => TunnelEffect::None,
        }
    }

    /// Spawn the process for the configured tunnel mode (effect executor for
    /// `TunnelEffect::Start`).
    pub fn tunnel_run(&mut self) {
        let Some(state) = self.remote.as_ref() else {
            return;
        };
        let port = state.config.port;
        match state.config.tunnel_mode.as_str() {
            "domain" => {
                let hostname = state.config.tunnel_hostname.clone();
                if hostname.is_empty() {
                    self.set_flash("custom domain: hostname belum diisi".to_string());
                    return;
                }
                state.tunnel.start_domain(port, hostname.clone());
                self.set_flash(format!("custom domain mulai… ({hostname})"));
            }
            _ => {
                state.tunnel.start_quick(port);
                self.set_flash("quick tunnel mulai…".to_string());
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
        self.emit_approval_resolved(label);
        Some(allow)
    }

    /// Settle the open approval from a web decision string ("once" / "always"
    /// / "reject"). Unknown strings are ignored.
    pub fn settle_remote_approval(&mut self, decision: &str) -> Option<bool> {
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

    pub fn submit_prompt(&mut self) -> Option<String> {
        if self.busy {
            return None;
        }
        let prompt = self.input.text().trim().to_string();
        if prompt.is_empty() && self.attachments.is_empty() {
            return None;
        }
        if prompt.starts_with('/') && self.attachments.is_empty() {
            self.input.take();
            self.slash = None;
            self.slash_dismissed = None;
            self.run_slash_line(&prompt);
            return None;
        }
        self.input.take();
        self.attachments.clear();
        self.push_entry(ChatEntry::User(prompt.clone()));
        self.save_current();
        self.ensure_placeholder_title(&prompt);
        self.busy = true;
        self.iteration = None;
        self.asst_open = false;
        self.scroll_from_bottom = 0;
        if let Some(turn) = self.remote.as_ref().map(|s| s.session_id.clone()) {
            self.emit_remote(evrax_remote::RemoteEvent::User {
                turn: turn.clone(),
                text: prompt.clone(),
            });
            self.emit_remote(evrax_remote::RemoteEvent::TurnStart { turn });
        }
        Some(prompt)
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
        if !arg.is_empty() {
            self.set_flash(format!("/{name} takes no arguments"));
            return;
        }
        self.execute_command(command);
    }

    /// Submit a prompt that originated from the web UI (queued by the remote
    /// server). Mirrors `submit_prompt` but uses the given text instead of the
    /// composer, so web prompts run under the current TUI mode and surface tool
    /// approvals in the terminal as usual.
    pub fn submit_external_prompt(&mut self, text: String) -> Option<String> {
        if self.busy {
            return None;
        }
        let prompt = text.trim().to_string();
        if prompt.is_empty() {
            return None;
        }
        self.push_entry(ChatEntry::User(prompt.clone()));
        self.ensure_placeholder_title(&prompt);
        self.busy = true;
        self.iteration = None;
        self.asst_open = false;
        self.scroll_from_bottom = 0;
        if let Some(turn) = self.remote.as_ref().map(|s| s.session_id.clone()) {
            self.emit_remote(evrax_remote::RemoteEvent::TurnStart { turn });
        }
        Some(prompt)
    }

    pub fn paste_text(&mut self, text: &str) {
        let cleaned: Vec<String> = text
            .split('\n')
            .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
            .collect();
        self.input
            .insert_str(cleaned.join("\n").trim_end_matches('\n'));
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

    pub fn composer_height(&self, width: usize) -> u16 {
        let chips = self.attachments.len().min(7) as u16;
        let text_rows = self.input.visual_rows(width).len().clamp(1, 7) as u16;
        (chips + text_rows).clamp(1, 9) + 2
    }

    pub fn last_assistant_text(&self) -> Option<String> {
        self.messages.iter().rev().find_map(|entry| match entry {
            ChatEntry::Assistant(text) if !text.is_empty() => Some(text.clone()),
            _ => None,
        })
    }

    pub fn cancel_turn(&mut self) {
        self.busy = false;
        self.asst_open = false;
        for entry in self.messages.iter_mut().rev() {
            if let ChatEntry::Tool(tool) = entry
                && matches!(tool.status, ToolStatus::Running)
            {
                tool.status = ToolStatus::Error;
                tool.detail = "cancelled".to_string();
            }
        }
        self.content_rev = self.content_rev.wrapping_add(1);
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
            AgentEvent::ToolStarted { name, summary } => {
                self.asst_open = false;
                self.push_entry(ChatEntry::Tool(ToolCall {
                    name: name.clone(),
                    summary,
                    status: ToolStatus::Running,
                    detail: "running…".to_string(),
                }));
                if let Some(turn) = &remote_turn {
                    self.emit_remote(evrax_remote::RemoteEvent::Tool {
                        turn: turn.clone(),
                        name,
                        status: "running".into(),
                        detail: None,
                    });
                }
            }
            AgentEvent::ToolFinished { name, ok, detail } => {
                self.asst_open = false;
                for entry in self.messages.iter_mut().rev() {
                    if let ChatEntry::Tool(tool) = entry
                        && matches!(tool.status, ToolStatus::Running)
                        && tool.name == name
                    {
                        tool.status = if ok {
                            ToolStatus::Done
                        } else {
                            ToolStatus::Error
                        };
                        tool.detail = detail.clone();
                        self.content_rev = self.content_rev.wrapping_add(1);
                        break;
                    }
                }
                if let Some(turn) = &remote_turn {
                    self.emit_remote(evrax_remote::RemoteEvent::Tool {
                        turn: turn.clone(),
                        name,
                        status: if ok { "done".into() } else { "error".into() },
                        detail: Some(detail),
                    });
                }
            }
            AgentEvent::ApprovalRequested { name, summary, key } => {
                self.asst_open = false;
                if let Some(turn) = &remote_turn {
                    self.emit_remote(evrax_remote::RemoteEvent::Approval {
                        turn: turn.clone(),
                        name: name.clone(),
                        summary: summary.clone(),
                    });
                }
                self.approval = Some(Approval {
                    name,
                    summary,
                    key,
                    selected: 0,
                });
            }
            AgentEvent::Diff { path, lines } => {
                self.asst_open = false;
                self.push_entry(ChatEntry::Diff { path, lines });
            }
            AgentEvent::TurnFinished => {
                self.busy = false;
                self.iteration = None;
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use evrax_core::AgentEvent;

    fn app() -> App {
        App::new()
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
    fn paste_preserves_newlines_and_collapses_spaces() {
        let mut app = app();
        app.paste_text("cargo\r\n  test   --all\r\n");
        assert_eq!(app.input.text(), "cargo\ntest --all");
    }

    #[test]
    fn editor_supports_multiline_navigation() {
        let mut ed = LineEditor::new();
        ed.insert_str("first line");
        ed.insert_newline();
        ed.insert_str("second");
        assert_eq!(ed.visual_rows(80), vec!["first line", "second"]);
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
        assert_eq!(ed.visual_rows(4), vec!["0123", "4567", "89"]);
        ed.insert_str("\nxy");
        assert_eq!(ed.visual_rows(4), vec!["0123", "4567", "89", "xy"]);
        ed.move_left();
        assert_eq!(ed.cursor_visual(4), (3, 1));
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
        assert_eq!(app.filtered_commands().len(), 7);
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
        assert_eq!(
            filtered.len(),
            1,
            "'sw' should fuzzy-match only Switch mode"
        );
        assert_eq!(filtered[0].0, "Switch to plan mode");
    }

    #[test]
    fn palette_selection_wraps_and_backspace_resets() {
        let mut app = app();
        app.toggle_palette();
        app.palette_move(-1);
        assert_eq!(app.palette.as_ref().unwrap().selected, 6);
        app.palette_move(1);
        assert_eq!(app.palette.as_ref().unwrap().selected, 0);
        app.palette_type('z');
        assert_eq!(app.filtered_commands().len(), 0);
        app.palette_backspace();
        assert_eq!(app.filtered_commands().len(), 7);
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
        assert_eq!(app.settle_remote_approval("bogus"), None, "unknown ignored");
        assert!(
            app.approval.is_some(),
            "unknown must not consume the dialog"
        );
        assert_eq!(app.settle_remote_approval("reject"), Some(false));
        assert!(app.approval.is_none(), "decision consumed the dialog");

        app.apply_event(AgentEvent::ApprovalRequested {
            name: "shell".into(),
            summary: "cargo test".into(),
            key: "shell".into(),
        });
        assert_eq!(app.settle_remote_approval("always"), Some(true));
        assert!(app.auto_allow.contains("shell"));
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
            "belum TurnFinished, belum ada job"
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
        // 7 commands; page size 8 â†’ down stays on the last row.
        app.palette_page(1, 8);
        assert_eq!(app.palette.as_ref().unwrap().selected, 6);
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
    #[test]
    fn wizard_presets_fill_provider_defaults() {
        let mut app = App::new();
        app.open_wizard(&evrax_agent::AgentConfig::default());
        app.wizard_preset(1);
        let wizard = app.wizard.as_ref().unwrap();
        assert_eq!(wizard.base_url, "https://openrouter.ai/api/v1");
        assert!(!wizard.model.is_empty());
    }

    #[test]
    fn wizard_config_trims_and_drops_empty_key() {
        let mut app = App::new();
        app.open_wizard(&evrax_agent::AgentConfig::default());
        if let Some(wizard) = app.wizard.as_mut() {
            wizard.base_url = "  http://x/v1  ".into();
            wizard.model = " m ".into();
            wizard.api_key = "   ".into();
        }
        let cfg = app.wizard_config().unwrap();
        assert_eq!(cfg.base_url, "http://x/v1");
        assert_eq!(cfg.model, "m");
        assert!(cfg.api_key.is_none());
    }
}
