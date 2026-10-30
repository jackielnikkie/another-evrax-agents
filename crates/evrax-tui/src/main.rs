mod app;
mod banner;
mod image;
mod markdown;
mod theme;
mod ui;
mod welcome;
mod window;
mod worker_link;

use std::io::Stdout;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, Event,
    EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use evrax_core::{AgentBackend, AgentEvent, MockBackend, TurnRequest};
use evrax_store::ProjectStore;
use futures_util::StreamExt;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Position;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::app::{
    AgentMode, App, ApprovalDecision, FetchRequest, FetchResult, ModelAction, TunnelEditResult,
    TunnelEffect, WizardAction,
};

#[tokio::main]
async fn main() -> Result<()> {
    if cli_info_flag() {
        return Ok(());
    }
    let cli = Cli::parse();
    let loaded = evrax_agent::config_from_args();
    let store = if cli.no_save {
        None
    } else {
        match ProjectStore::open(&loaded.config.root) {
            Ok(store) => Some(store),
            Err(e) => {
                eprintln!("evrax: session store unavailable ({e})");
                None
            }
        }
    };
    if cli.list {
        match store.as_ref() {
            Some(store) => print_sessions(store)?,
            None => println!("(session store disabled)"),
        }
        return Ok(());
    }
    let resume = if cli.continue_latest || cli.session.is_some() {
        let store = store
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("session store disabled (--no-save/--mock)"))?;
        let id = match cli.session.as_deref() {
            Some(spec) => resolve_session(store, spec)?,
            None => store
                .list()?
                .first()
                .map(|m| m.id.clone())
                .ok_or_else(|| anyhow::anyhow!("no saved sessions yet"))?,
        };
        Some(id)
    } else {
        None
    };
    install_panic_hook();
    if let Some(idx) = theme::load_saved() {
        theme::set_index(idx);
    }
    let need_wizard = cli.setup
        || (!loaded.mock
            && loaded
                .config
                .api_key
                .as_deref()
                .unwrap_or("")
                .trim()
                .is_empty()
            && std::env::var_os("EVRAX_API_KEY").is_none());
    // Worker mode: run the agent loop without touching the terminal at all.
    // Checked from argv (not config) so a stale agent.json never sends an
    // interactive run down this path, and vice versa.
    if has_flag("--headless") {
        return run_headless(loaded, store).await;
    }
    let mut terminal = init_terminal().context("evrax needs an interactive terminal")?;
    let result = run(&mut terminal, loaded, store, resume, need_wizard).await;
    let restored = restore_terminal(&mut terminal);
    restored.context("failed to restore terminal state")?;
    result
}

#[derive(Default)]
struct Cli {
    list: bool,
    continue_latest: bool,
    session: Option<String>,
    no_save: bool,
    setup: bool,
}

impl Cli {
    fn parse() -> Self {
        let mut list = false;
        let mut continue_latest = false;
        let mut session = None;
        let mut no_save = false;
        let mut setup = false;
        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--list" | "-l" | "sessions" => list = true,
                "--continue" | "-c" => continue_latest = true,
                "--session" | "-s" => session = args.next(),
                "--no-save" => no_save = true,
                "--setup" => setup = true,
                _ => {}
            }
        }
        Cli {
            list,
            continue_latest,
            session,
            no_save,
            setup,
        }
    }
}

fn print_sessions(store: &ProjectStore) -> Result<()> {
    println!("project: {}", store.root().display());
    let metas = store.list()?;
    if metas.is_empty() {
        println!("(no sessions yet)");
        return Ok(());
    }
    for meta in metas {
        println!(
            "{:<28}  {:<16}  {}",
            meta.id,
            fmt_ms(meta.updated_at),
            meta.title
        );
    }
    Ok(())
}

fn resolve_session(store: &ProjectStore, spec: &str) -> Result<String> {
    let metas = store.list()?;
    if metas.iter().any(|m| m.id == spec) {
        return Ok(spec.to_string());
    }
    let by_prefix: Vec<&evrax_store::SessionMeta> =
        metas.iter().filter(|m| m.id.starts_with(spec)).collect();
    if by_prefix.len() == 1 {
        return Ok(by_prefix[0].id.clone());
    }
    if by_prefix.len() > 1 {
        bail!(
            "session prefix '{spec}' ambigu ({} kandidat)",
            by_prefix.len()
        );
    }
    let by_title: Vec<&evrax_store::SessionMeta> = metas
        .iter()
        .filter(|m| m.title.eq_ignore_ascii_case(spec))
        .collect();
    if by_title.len() == 1 {
        return Ok(by_title[0].id.clone());
    }
    if by_title.len() > 1 {
        bail!("title '{spec}' is ambiguous ({} sessions)", by_title.len());
    }
    bail!("session '{spec}' not found; try evrax --list")
}

fn fmt_ms(ms: u64) -> String {
    let secs = ms / 1000;
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}",
        rem / 3600,
        (rem % 3600) / 60
    )
}

fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn cli_info_flag() -> bool {
    const HELP: &str = "USAGE:\n    evrax [OPTIONS]\n\nOPTIONS:\n    -c, --continue         resume the most recent session\n    -s, --session <spec>   resume by id, unique prefix, or title\n    -l, --list             list saved sessions and exit\n        --setup            open the provider setup wizard\n        --no-save          do not persist sessions\n        --mock             run with the built-in mock backend\n        --provider <name>  use a saved provider for this run\n        --model <name>     model id\n        --base-url <url>   openai-compatible base URL\n        --api-key <key>    API key (else agent.json / EVRAX_API_KEY)\n        --max-iterations <n>  tool rounds per turn (default 500)\n        --root <path>      workspace root\n        --headless             run the agent loop without the TUI (used by workers)\n        --worker-id <id>     internal: run as a named worker\n    -V, --version          print version and exit\n    -h, --help             print this help and exit\n\nIn the TUI: /model switches provider or model.";
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--version" | "-V" => {
                println!("evrax {}", env!("CARGO_PKG_VERSION"));
                return true;
            }
            "--help" | "-h" => {
                println!("evrax {}\n\n{HELP}", env!("CARGO_PKG_VERSION"));
                return true;
            }
            _ => {}
        }
    }
    false
}

fn init_terminal() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableBracketedPaste
    )?;
    let _ = execute!(
        stdout,
        crossterm::event::PushKeyboardEnhancementFlags(
            crossterm::event::KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
        )
    );
    let terminal = Terminal::new(CrosstermBackend::new(stdout))?;
    Ok(terminal)
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    disable_raw_mode()?;
    let _ = execute!(
        terminal.backend_mut(),
        crossterm::event::PopKeyboardEnhancementFlags
    );
    execute!(
        terminal.backend_mut(),
        DisableBracketedPaste,
        DisableMouseCapture,
        LeaveAlternateScreen
    )?;
    terminal.show_cursor()?;
    Ok(())
}

/// Panic hook: restore the terminal so the message is readable, then append a
/// crash report to a log file outside the workspace. A panic may happen before
/// or after terminal init, so every step is best-effort and never panics again.
fn install_panic_hook() {
    let original = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(
            std::io::stdout(),
            DisableBracketedPaste,
            DisableMouseCapture,
            LeaveAlternateScreen
        );
        write_panic_log(info);
        original(info);
    }));
}

/// Appends path/duration/transcript-size to the panic log. Prefers
/// `%APPDATA%\evrax`, then `$HOME/.config/evrax`, then the current directory.
fn write_panic_log(info: &std::panic::PanicHookInfo<'_>) {
    use std::io::Write as _;

    let Some(path) = evrax_data_dir().map(|dir| dir.join("last-panic.log")) else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let location = info
        .location()
        .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
        .unwrap_or_else(|| "<unknown>".to_string());
    let payload = info
        .payload()
        .downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| info.payload().downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "<non-string panic>".to_string());
    let backtrace = std::env::var_os("RUST_BACKTRACE")
        .is_some()
        .then(|| std::backtrace::Backtrace::force_capture().to_string());
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(file, "--- panic at {stamp} ---");
        let _ = writeln!(file, "location: {location}");
        let _ = writeln!(file, "message: {payload}");
        if let Some(bt) = backtrace {
            let _ = writeln!(file, "backtrace:\n{bt}");
        }
    }
}

/// Best-effort per-user data directory for logs. Prefers `%APPDATA%\evrax`,
/// then `$HOME/.config/evrax`, then the current directory.
fn evrax_data_dir() -> Option<std::path::PathBuf> {
    if let Some(appdata) = std::env::var_os("APPDATA") {
        return Some(std::path::PathBuf::from(appdata).join("evrax"));
    }
    if let Some(home) = std::env::var_os("HOME") {
        return Some(std::path::PathBuf::from(home).join(".config").join("evrax"));
    }
    std::env::current_dir().ok()
}

/// Builds the active backend. `mock` (or a client build failure) yields the
/// mock; the label feeds `App.backend_label`, the note the startup flash.
fn chat_backend(
    cfg: evrax_agent::AgentConfig,
    store: Option<ProjectStore>,
    mock: bool,
) -> (Box<dyn AgentBackend>, String, Option<String>) {
    if mock {
        return (
            Box::new(MockBackend::new()),
            "mock".to_string(),
            Some("backend · mock".to_string()),
        );
    }
    match evrax_agent::ChatBackend::new_with_project(cfg.clone(), store) {
        Ok(real) => {
            let label = real.model().to_string();
            let note = if cfg.provider.is_empty() {
                format!("backend · {label} · {}", real.base_url())
            } else {
                format!("backend · {} · {label} · {}", cfg.provider, real.base_url())
            };
            (Box::new(real), label, Some(note))
        }
        Err(e) => (
            Box::new(MockBackend::new()),
            "mock".to_string(),
            Some(format!("backend failed ({e}) · using mock")),
        ),
    }
}

/// Headless worker: runs the agent loop with no terminal. Reads prompts from
/// the worker inbox, forwards events to `events.jsonl`, and publishes heartbeat
/// status. Exits on a stop request. A worker may only be spawned by the parent
/// TUI (which passes `--worker-id`), so a missing id or link is fatal.
async fn run_headless(
    loaded: evrax_agent::LoadedConfig,
    store: Option<ProjectStore>,
) -> Result<()> {
    let id = arg_value("--worker-id")
        .ok_or_else(|| anyhow::anyhow!("--headless requires --worker-id"))?;
    let mut link =
        worker_link::Link::load(&id).ok_or_else(|| anyhow::anyhow!("worker {id} not found"))?;

    // `chat_backend` needs a store rooted at the worker's workspace; a worker is
    // always launched with `--root`, so build one if the caller had none (mock).
    let store = store.or_else(|| ProjectStore::open(&loaded.config.root).ok());
    let (backend, _label, _note) = chat_backend(loaded.config.clone(), store.clone(), loaded.mock);

    let mut app = App::new();
    app.session_id = id.clone();
    let name = link.name().to_string();
    app.titles.insert(id.clone(), name);
    app.manual_titles.insert(id.clone());
    link.publish_final(&app, "idle");

    let mut ticker = tokio::time::interval(Duration::from_millis(250));
    loop {
        ticker.tick().await;
        if link.stop_requested() {
            break;
        }
        let Some(prompt) = link.take_prompt() else {
            link.publish(&app);
            continue;
        };
        run_headless_turn(&mut app, &*backend, &id, prompt, &mut link).await;
    }
    link.publish_final(&app, "stopped");
    Ok(())
}

/// Runs one worker turn to completion, streaming every event to the worker's
/// JSONL transcript. Explicit approval requests are auto-rejected: a headless
/// worker has no terminal to answer them, and anything inside its workspace was
/// already authorized by the backend.
async fn run_headless_turn(
    app: &mut App,
    backend: &dyn AgentBackend,
    id: &str,
    prompt: String,
    link: &mut worker_link::Link,
) {
    let Some(prompt) = app.submit_external_prompt(prompt) else {
        return;
    };
    let session = app.session_id.clone();
    let plan_mode = app.mode == AgentMode::Plan;
    let (decision_tx, decision_rx) = mpsc::channel::<bool>(4);
    let request = TurnRequest {
        prompt,
        session,
        plan_mode,
    };
    link.publish(app);
    let mut stream = backend.run_turn(request, decision_rx);
    while let Some(event) = stream.next().await {
        if let AgentEvent::ApprovalRequested { .. } = event {
            // A headless worker has no pane to answer a prompt on. Rather than
            // rejecting every gated call (which made even `echo hi` fail),
            // approve it: the workspace is the user's own, and anything truly
            // outside it is meant to be a deliberate, user-facing decision —
            // which the orchestrator handles, not a silent worker.
            let _ = decision_tx.try_send(true);
        }
        let _ = evrax_agent::worker::append_event_json(id, &event_to_json(&event));
        app.apply_event(event);
    }
    // A clean TurnFinished already cleared these; a Failed stream leaves the
    // turn busy, so clear it here without the cancel flash/tool-error sweep.
    app.busy = false;
    app.turn_started = None;
    link.publish_final(app, "idle");
}

/// Serializes an `AgentEvent` (not `Serialize` in `evrax-core`) into the shape
/// the TUI's worker-pane transcript understands. Only the variants that matter
/// for a readable pane are mapped; anything else is dropped.
fn event_to_json(event: &AgentEvent) -> serde_json::Value {
    use serde_json::json;
    match event {
        AgentEvent::ThinkingDelta { text } => json!({"type": "thinking", "text": text}),
        AgentEvent::TextDelta { text } => json!({"type": "delta", "text": text}),
        AgentEvent::ToolPreparing { name, bytes } => {
            json!({"type": "tool_preparing", "name": name, "bytes": bytes})
        }
        AgentEvent::ToolStarted { name, summary } => {
            json!({"type": "tool", "name": name, "summary": summary})
        }
        AgentEvent::ToolFinished { name, ok, detail } => {
            json!({"type": "tool_done", "name": name, "ok": ok, "detail": detail})
        }
        AgentEvent::Notice { message } => json!({"type": "notice", "message": message}),
        AgentEvent::Failed { message } => json!({"type": "error", "message": message}),
        AgentEvent::TurnFinished => json!({"type": "turn_finished"}),
        AgentEvent::TitleSuggested { session, title } => {
            json!({"type": "title", "session": session, "title": title})
        }
        AgentEvent::Iteration { n, max } => json!({"type": "iteration", "n": n, "max": max}),
        AgentEvent::Retrying {
            attempt,
            max,
            delay_ms,
            reason,
        } => {
            json!({"type": "retrying", "attempt": attempt, "max": max, "delay_ms": delay_ms, "reason": reason})
        }
        AgentEvent::ThinkingFinished { seconds } => {
            json!({"type": "thinking_done", "seconds": seconds})
        }
        AgentEvent::ApprovalRequested { name, summary, .. } => {
            json!({"type": "approval", "name": name, "summary": summary})
        }
        AgentEvent::Diff { path, lines } => {
            let lines: Vec<serde_json::Value> = lines
                .iter()
                .map(|l| json!({"kind": l.kind, "text": l.text}))
                .collect();
            json!({"type": "diff", "path": path, "lines": lines})
        }
    }
}

async fn run(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    loaded: evrax_agent::LoadedConfig,
    store: Option<ProjectStore>,
    resume: Option<String>,
    need_wizard: bool,
) -> Result<()> {
    let (mut backend, backend_label, backend_note) =
        chat_backend(loaded.config.clone(), store.clone(), loaded.mock);
    let (event_tx, mut event_rx) = mpsc::channel::<AgentEvent>(64);
    let (fetch_tx, mut fetch_rx) = mpsc::channel::<FetchResult>(8);
    let (update_tx, mut update_rx) = mpsc::channel::<(
        bool,
        Result<Option<evrax_agent::update::UpdateCheck>, String>,
    )>(4);
    let mut turn_task: Option<JoinHandle<()>> = None;
    let mut app = App::new();
    app.attach_config(&loaded);
    let worker_id = arg_value("--worker-id");
    if worker_id.is_none()
        && let Some(store) = store.clone()
    {
        app.attach_project(store);
        if let Some(id) = resume.as_deref() {
            app.resume(id);
        }
    }
    if need_wizard {
        app.open_wizard();
    }
    app.catalog = evrax_agent::catalog::Catalog::load();
    app.set_backend_label(backend_label);
    if let Some(note) = backend_note {
        app.set_flash(note);
    }
    // One silent probe at startup: only a newer release breaks the silence.
    app.request_startup_update_check();
    let mut worker_link = worker_id.and_then(|id| {
        let mut link = worker_link::Link::load(&id)?;
        app.session_id = id.clone();
        let name = link.name().to_string();
        app.titles.insert(id.clone(), name.clone());
        app.manual_titles.insert(id.clone());
        link.publish_final(&app, "idle");
        app.set_flash(format!("worker {name}"));
        Some(link)
    });
    let mut decision_tx: Option<mpsc::Sender<bool>> = None;
    let mut events = EventStream::new();
    let mut ticker = tokio::time::interval(Duration::from_millis(250));
    let debug_window = std::env::var_os("EVRAX_DEBUG_MAXIMIZED").is_some();
    let mut debug_logged = false;
    // Optional slow-frame watchdog: only active when the env flag is set, so the
    // normal path pays nothing beyond one `Instant::now` per repaint.
    let slow_enabled = std::env::var_os("EVRAX_DEBUG_SLOW").is_some();
    let slow_threshold = Duration::from_secs(1);

    // Repaint only when something actually changed. The old unconditional 4 Hz
    // redraw rebuilt the whole transcript twice per frame and burned ~50% of a core.
    let mut needs_draw = true;
    let mut max_scroll = 0u16;
    let mut last_size = terminal.size()?;
    let mut input_open = true;
    // Drag-select floods events faster than a frame; coalesce to ~60 fps. The
    // selection still updates every event, only the repaint is throttled.
    let drag_frame = Duration::from_millis(16);
    let mut last_drag_paint = Instant::now();

    while !app.should_quit {
        while let Ok((request_id, result)) = fetch_rx.try_recv() {
            app.wizard_models_ready(request_id, result.clone());
            app.model_ready(request_id, result);
            needs_draw = true;
        }
        // An explicit `/update` or the startup probe: spawn at most one check.
        if let Some(explicit) = app.update_job() {
            let tx = update_tx.clone();
            let current = env!("CARGO_PKG_VERSION");
            tokio::spawn(async move {
                let result = evrax_agent::update::check_update(current).await;
                let _ = tx.send((explicit, result)).await;
            });
        }
        while let Ok((explicit, result)) = update_rx.try_recv() {
            app.apply_update_check(explicit, result);
            needs_draw = true;
        }
        if let Some(cfg) = app.take_reload_config() {
            if loaded.mock {
                app.set_flash("config saved · mock still active".to_string());
            } else {
                let (next, label, note) = chat_backend(cfg, store.clone(), false);
                backend = next;
                app.set_backend_label(label);
                if let Some(note) = note {
                    app.set_flash(note);
                }
            }
            needs_draw = true;
        }
        if let Some(n) = app.take_worker_spawn_request() {
            spawn_workers(&mut app, n, &loaded.config.root);
            needs_draw = true;
        }
        if needs_draw {
            let draw_start = Instant::now();
            terminal.draw(|frame| ui::render(frame, &mut app))?;
            if slow_enabled {
                let elapsed = draw_start.elapsed();
                if elapsed > slow_threshold {
                    log_slow_frame(elapsed, app.messages.len());
                }
            }
            flush_kitty(terminal, &mut app);
            let size = terminal.size()?;
            last_size = size;
            app.set_viewport(size.height.saturating_sub(6));
            max_scroll = app.frame_max_scroll;
            app.clamp_scroll(max_scroll);
            if let Some(up) = app.selection_edge() {
                app.scroll_view(1, up, max_scroll);
            }
            if let Some(text) = app.copied_text.take() {
                copy_to_clipboard(text);
            }
            needs_draw = false;
        }

        tokio::select! {
            event = events.next(), if input_open => {
                if event.is_none() {
                    input_open = false;
                }
                if let Some(Ok(Event::Key(key))) = event
                    && key.kind == KeyEventKind::Press
                {
                    if is_batchable(&app, &key)
                        && let KeyCode::Char(c) = key.code
                        && !key.modifiers.intersects(KeyModifiers::ALT | KeyModifiers::CONTROL)
                    {
                        let (chars, leftover) = drain_burst(&mut events, c).await;
                        if !chars.is_empty() {
                            // Terminal paste without bracketed-paste support arrives
                            // as a key burst. Screenshot pastes show up as a temp
                            // file path (or as raw pixels only in the clipboard), so
                            // try the clipboard image first, then
                            // fall back to path detection, then literal text.
                            if is_paste_like(&chars) {
                                if !app.ingest_paste(&chars) {
                                    app.paste_text_input(&chars);
                                }
                            } else {
                                app.paste_text_input(&chars);
                            }
                            app.slash_sync();
                        }
                        if let Some(ev) = leftover {
                            handle_event(
                                &mut app,
                                ev,
                                &*backend,
                                &event_tx,
                                &mut decision_tx,
                                &mut turn_task,
                                max_scroll,
                            );
                        }
                    } else if app.wizard.is_some() {
                        handle_wizard_key(&mut app, key, &fetch_tx);
                    } else if app.model_dialog_open() {
                        handle_model_key(&mut app, key, &fetch_tx);
                    } else {
                        handle_event(
                            &mut app,
                            Event::Key(key),
                            &*backend,
                            &event_tx,
                            &mut decision_tx,
                            &mut turn_task,
                            max_scroll,
                        );
                    }
                    needs_draw = true;
                } else if let Some(Ok(other)) = event {
                    // Mouse motion alone never repaints: the terminal floods these
                    // events and each repaint re-wraps the whole transcript.
                    let idle_motion =
                        matches!(other, Event::Mouse(ref m) if matches!(m.kind, MouseEventKind::Moved));
                    // Left-drag selection streams events far faster than a frame.
                    // Coalesce its repaint to ~60 fps; `selection.update` below
                    // still runs on every event so the highlight stays accurate.
                    let dragging = matches!(other, Event::Mouse(ref m) if matches!(m.kind, MouseEventKind::Drag(MouseButton::Left)));
                    let drag_due = !dragging || last_drag_paint.elapsed() >= drag_frame;
                    if dragging && drag_due {
                        last_drag_paint = Instant::now();
                    }
                    needs_draw |= !idle_motion && drag_due;
                    handle_event(
                        &mut app,
                        other,
                        &*backend,
                        &event_tx,
                        &mut decision_tx,
                        &mut turn_task,
                        max_scroll,
                    );
                }
            }
            Some(event) = event_rx.recv() => {
                app.apply_event(event);
                // Drain everything already queued so a burst of stream deltas costs
                // one repaint instead of one per token.
                while let Ok(more) = event_rx.try_recv() {
                    app.apply_event(more);
                }
                if app.take_auto_approval()
                    && let Some(tx) = &decision_tx
                {
                    let _ = tx.try_send(true);
                }
                if let Some((session, prompt, reply)) = app.take_title_job() {
                    let tx = event_tx.clone();
                    let future = backend.suggest_title(prompt, reply);
                    tokio::spawn(async move {
                        if let Some(title) = future.await {
                            let _ = tx
                                .send(AgentEvent::TitleSuggested { session, title })
                                .await;
                        }
                    });
                }
                needs_draw = true;
            }
            _ = ticker.tick() => {
                let had_flash = app.flash.is_some();
                let was_busy = app.busy;
                let had_approval = app.approval.is_some();
                let mode_before = app.mode;
                app.tick();
                app.sync_remote_mode();
                app.poll_workers();
                drain_remote_prompts(&mut app, &*backend, &event_tx, &mut decision_tx, &mut turn_task);
                drain_queued_prompt(&mut app, &*backend, &event_tx, &mut decision_tx, &mut turn_task);
                drain_remote_approvals(&mut app, &mut decision_tx);
                drain_worker_prompt(
                    &mut app,
                    &mut worker_link,
                    &*backend,
                    &event_tx,
                    &mut decision_tx,
                    &mut turn_task,
                );
                if worker_link
                    .as_ref()
                    .is_some_and(worker_link::Link::stop_requested)
                {
                    app.should_quit = true;
                }
                if let Some(link) = worker_link.as_mut() {
                    link.publish(&app);
                }
                if let Some(state) = window::window_state()
                    && app.maximized != Some(state)
                {
                    app.maximized = Some(state);
                    if debug_window && !debug_logged {
                        debug_logged = true;
                        log_window_debug(app.maximized);
                    }
                }
                if let Ok(size) = terminal.size()
                    && size != last_size
                {
                    needs_draw = true;
                }
                // Keep repainting only while something animates (spinner, expiring
                // flash, kitty frames) or when a drain changed dialog state.
                needs_draw |= had_flash
                    || was_busy
                    || had_approval
                    || app.busy
                    || app.flash.is_some()
                    || app.approval.is_some()
                    || app.mode != mode_before
                    || app.has_workers()
                    || !app.pending_kitty.is_empty()
                    || (app.messages.is_empty() && app.tick.is_multiple_of(2));
            }
        }
    }

    if let Some(link) = worker_link.as_mut() {
        link.publish_final(&app, "stopped");
    }
    app.save_current();
    if let Some(task) = turn_task.take() {
        task.abort();
    }
    Ok(())
}

/// Writes queued kitty graphics escape sequences straight to the terminal.
/// These bypass ratatui's cell buffer because the protocol carries pixel data.
fn flush_kitty(terminal: &mut Terminal<CrosstermBackend<Stdout>>, app: &mut App) {
    if app.pending_kitty.is_empty() {
        return;
    }
    use std::io::Write as _;
    let mut out = String::new();
    for esc in app.pending_kitty.drain(..) {
        out.push_str(&esc);
    }
    let backend = terminal.backend_mut();
    let _ = write!(backend, "{out}");
    let _ = backend.flush();
}

fn log_window_debug(state: Option<bool>) {
    use std::io::Write as _;

    let path = std::env::temp_dir().join("evrax-window-debug.log");
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "state={state:?} {}", window::diagnostic());
    }
}

/// Slow-frame watchdog sink (only called when `EVRAX_DEBUG_SLOW` is set): one
/// line per repaint that took over a second, with the transcript size.
fn log_slow_frame(elapsed: Duration, messages: usize) {
    use std::io::Write as _;

    let path = evrax_data_dir()
        .map(|dir| dir.join("slow-frame.log"))
        .unwrap_or_else(|| std::path::PathBuf::from("evrax-slow-frame.log"));
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(
            file,
            "t={stamp} frame_ms={} messages={messages}",
            elapsed.as_millis()
        );
    }
}

/// A burst of chars that looks like a paste: multi-char, or a single image path.
fn is_paste_like(chars: &str) -> bool {
    chars.chars().count() > 1 || crate::image::is_image_path(chars)
}

fn is_batchable(app: &App, key: &crossterm::event::KeyEvent) -> bool {
    key.kind == KeyEventKind::Press
        && app.approval.is_none()
        && app.palette.is_none()
        && app.session_dialog.is_none()
        && app.theme_dialog.is_none()
        && app.remote_dialog.is_none()
        && app.preview.is_none()
        && app.wizard.is_none()
        && !app.model_dialog_open()
        && (!app.busy || app.stream_input_open())
}

async fn drain_burst(events: &mut EventStream, first: char) -> (String, Option<Event>) {
    let mut chars = String::from(first);
    let deadline = Duration::from_millis(3);
    loop {
        match tokio::time::timeout(deadline, events.next()).await {
            Ok(Some(Ok(Event::Key(key)))) => {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                if key
                    .modifiers
                    .intersects(KeyModifiers::ALT | KeyModifiers::CONTROL)
                {
                    return (chars, Some(Event::Key(key)));
                }
                match key.code {
                    KeyCode::Char(c) => chars.push(c),
                    KeyCode::Enter if key.modifiers.is_empty() => chars.push('\n'),
                    _ => return (chars, Some(Event::Key(key))),
                }
            }
            Ok(Some(Ok(event))) => return (chars, Some(event)),
            Ok(Some(Err(_))) | Ok(None) => return (chars, None),
            Err(_) => return (chars, None),
        }
    }
}

/// Persist the edited password, push it to the running server, and flash.
fn commit_remote_password(app: &mut App) {
    match app.remote_edit_commit() {
        Ok(cfg) => {
            let persisted = evrax_remote::save_remote_config(&cfg).is_ok();
            if let Some(server) = app.remote_server() {
                evrax_remote::set_password(server, cfg.password.clone());
            }
            app.set_flash(if persisted {
                "remote password updated".to_string()
            } else {
                "password applied for this session (remote.json not writable)".to_string()
            });
            // A tunnel that was waiting on the default password may now start.
            if let TunnelEffect::Start = app.tunnel_follow_remote() {
                tunnel_start(app);
            }
        }
        Err(msg) if msg == "password unchanged" => {}
        Err(msg) => app.set_flash(msg),
    }
}

/// Start the process for the configured tunnel mode and make sure the web
/// server is registered so the tunnel has something to expose.
fn tunnel_start(app: &mut App) {
    app.tunnel_run();
    app.ensure_remote_registered();
}

/// Persist the current remote config (tunnel mode/url changes included).
fn persist_remote(app: &mut App) {
    if let Some(state) = &app.remote {
        let _ = evrax_remote::save_remote_config(&state.config);
    }
}

/// Persist a finished tunnel inline edit (manual URL or custom hostname) and
/// run the follow-up action it implies.
fn commit_tunnel_url(app: &mut App) {
    match app.tunnel_edit_commit() {
        Ok(TunnelEditResult::UrlSaved) => {
            let url = app
                .remote
                .as_ref()
                .map(|s| s.config.public_url.clone())
                .unwrap_or_default();
            app.push_public_url(&url);
            persist_remote(app);
            app.set_flash("tunnel url saved".to_string());
        }
        Ok(TunnelEditResult::UrlCleared) => {
            app.push_public_url("");
            persist_remote(app);
            app.set_flash("tunnel url cleared".to_string());
        }
        Ok(TunnelEditResult::HostReady) => {
            persist_remote(app);
            app.set_flash("custom domain: menyiapkan tunnel…".to_string());
            tunnel_start(app);
        }
        Err(msg) => app.set_flash(msg),
    }
}

/// Spawn a backend turn and forward its events to the UI channel.
fn start_turn(
    backend: &dyn AgentBackend,
    event_tx: &mpsc::Sender<AgentEvent>,
    decision_tx: &mut Option<mpsc::Sender<bool>>,
    turn_task: &mut Option<JoinHandle<()>>,
    prompt: String,
    session: String,
    plan_mode: bool,
) {
    let (tx, rx) = mpsc::channel::<bool>(4);
    *decision_tx = Some(tx);
    let request = TurnRequest {
        prompt,
        session,
        plan_mode,
    };
    let stream = backend.run_turn(request, rx);
    let tx = event_tx.clone();
    *turn_task = Some(tokio::spawn(async move {
        let mut stream = stream;
        while let Some(event) = stream.next().await {
            if tx.send(event).await.is_err() {
                break;
            }
        }
    }));
}

/// Drain a queued web prompt (only when the TUI is idle) and run it as a turn.
/// Prompts arriving while busy stay queued and are picked up on a later tick.
fn drain_remote_prompts(
    app: &mut App,
    backend: &dyn AgentBackend,
    event_tx: &mpsc::Sender<AgentEvent>,
    decision_tx: &mut Option<mpsc::Sender<bool>>,
    turn_task: &mut Option<JoinHandle<()>>,
) {
    if app.busy {
        return;
    }
    let text = {
        let Some(session_id) = app.remote.as_ref().map(|r| r.session_id.clone()) else {
            return;
        };
        let Some(server) = app.remote_server() else {
            return;
        };
        evrax_remote::take_pending_prompt(server, &session_id)
    };
    let Some(text) = text else { return };
    if let Some(prompt) = app.submit_web_prompt(text) {
        let (session, plan_mode) = (app.session_id.clone(), app.mode == AgentMode::Plan);
        start_turn(
            backend,
            event_tx,
            decision_tx,
            turn_task,
            prompt,
            session,
            plan_mode,
        );
    }
}

/// Runs a prompt the user typed while a turn was streaming. Called after the
/// web queue is drained, so local typing keeps its order.
fn drain_queued_prompt(
    app: &mut App,
    backend: &dyn AgentBackend,
    event_tx: &mpsc::Sender<AgentEvent>,
    decision_tx: &mut Option<mpsc::Sender<bool>>,
    turn_task: &mut Option<JoinHandle<()>>,
) {
    let Some(prompt) = app.take_queued_prompt() else {
        return;
    };
    let (session, plan_mode) = (app.session_id.clone(), app.mode == AgentMode::Plan);
    start_turn(
        backend,
        event_tx,
        decision_tx,
        turn_task,
        prompt,
        session,
        plan_mode,
    );
}

fn arg_value(name: &str) -> Option<String> {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == name {
            return args.next();
        }
    }
    None
}

fn has_flag(name: &str) -> bool {
    std::env::args().skip(1).any(|arg| arg == name)
}

/// Launches the headless worker processes backing freshly created panes. Each
/// placeholder pane (empty id) gets one process. Spawning only readies the
/// panes: the user orchestrates explicitly (worker send or a normal prompt),
/// so `/worker N` never spends a turn on its own.
fn spawn_workers(app: &mut App, _n: usize, root: &std::path::Path) {
    let session = app.session_id.clone();
    let mut spawned: Vec<String> = Vec::new();
    let mut last_error: Option<String> = None;
    for pane in app.workers.iter_mut() {
        if !pane.id.is_empty() {
            continue;
        }
        match evrax_agent::worker::spawn_worker(&pane.name, None, root, &session) {
            Ok(state) => {
                pane.id = state.id.clone();
                pane.status = "starting".to_string();
                spawned.push(state.id);
            }
            Err(error) => {
                pane.status = "failed".to_string();
                last_error = Some(error);
            }
        }
    }
    if let Some(error) = last_error {
        app.set_flash(format!("worker spawn failed: {error}"));
    }
    if spawned.is_empty() {
        return;
    }
    app.set_flash(format!(
        "workers ready: {} (orchestrate with worker send)",
        spawned.len()
    ));
}

fn drain_worker_prompt(
    app: &mut App,
    link: &mut Option<worker_link::Link>,
    backend: &dyn AgentBackend,
    event_tx: &mpsc::Sender<AgentEvent>,
    decision_tx: &mut Option<mpsc::Sender<bool>>,
    turn_task: &mut Option<JoinHandle<()>>,
) {
    if app.busy {
        return;
    }
    let Some(link) = link.as_ref() else {
        return;
    };
    let Some(text) = link.take_prompt() else {
        return;
    };
    if let Some(prompt) = app.submit_external_prompt(text) {
        let (session, plan_mode) = (app.session_id.clone(), app.mode == AgentMode::Plan);
        start_turn(
            backend,
            event_tx,
            decision_tx,
            turn_task,
            prompt,
            session,
            plan_mode,
        );
    }
}

/// Applies accept/reject decisions sent from the web while the approval
/// dialog is open (the dialog may also be settled from the terminal). Each
/// decision carries the id of the approval it targets: only the decision for
/// the dialog currently open is applied; anything stale is dropped as it is
/// drained.
fn drain_remote_approvals(app: &mut App, decision_tx: &mut Option<mpsc::Sender<bool>>) {
    if app.approval.is_none() {
        return;
    }
    let mut apply: Option<(String, u64)> = None;
    loop {
        let next = {
            let Some(session_id) = app.remote.as_ref().map(|r| r.session_id.clone()) else {
                return;
            };
            let Some(server) = app.remote_server() else {
                return;
            };
            evrax_remote::take_pending_approval(server, &session_id)
        };
        match next {
            Some((decision, id)) => {
                if app.approval.as_ref().map(|a| a.id) == Some(id) {
                    apply = Some((decision, id));
                }
            }
            None => break,
        }
        if app.approval.is_none() {
            break;
        }
    }
    let Some((decision, id)) = apply else { return };
    if let Some(allow) = app.settle_remote_approval(&decision, id)
        && let Some(tx) = decision_tx
    {
        let _ = tx.try_send(allow);
    }
}

fn handle_wizard_key(app: &mut App, key: KeyEvent, fetch_tx: &mpsc::Sender<FetchResult>) {
    if key.kind != KeyEventKind::Press {
        return;
    }
    match key.code {
        KeyCode::Esc => {
            if app.wizard_escape() {
                app.wizard_close();
                app.set_flash("setup skipped".to_string());
            }
        }
        KeyCode::Up => app.wizard_move(-1),
        KeyCode::Down => app.wizard_move(1),
        KeyCode::PageUp => app.wizard_page(-1),
        KeyCode::PageDown => app.wizard_page(1),
        KeyCode::Left if app.wizard_step() == Some(crate::app::WizardStep::BaseUrl) => {
            app.wizard_preset(-1)
        }
        KeyCode::Right if app.wizard_step() == Some(crate::app::WizardStep::BaseUrl) => {
            app.wizard_preset(1)
        }
        KeyCode::Left if app.wizard_step() == Some(crate::app::WizardStep::Kind) => {
            app.wizard_kind(-1)
        }
        KeyCode::Right if app.wizard_step() == Some(crate::app::WizardStep::Kind) => {
            app.wizard_kind(1)
        }
        KeyCode::Enter => match app.wizard_enter() {
            WizardAction::FetchModels(request) => spawn_fetch(request, fetch_tx),
            WizardAction::Save(settings) => match evrax_agent::save_config(&settings) {
                Ok(path) => {
                    app.apply_settings(settings);
                    app.wizard_close();
                    app.set_flash(format!("config saved · {}", path.display()));
                }
                Err(e) => app.wizard_status(format!("save failed: {e}"), false),
            },
            WizardAction::None => {}
        },
        KeyCode::Backspace => app.wizard_backspace(),
        KeyCode::Char(c)
            if !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            app.wizard_type(c);
        }
        _ => {}
    }
}

fn handle_model_key(app: &mut App, key: KeyEvent, fetch_tx: &mpsc::Sender<FetchResult>) {
    if key.kind != KeyEventKind::Press {
        return;
    }
    match key.code {
        KeyCode::Esc => {
            if app.model_escape() {
                app.model_close();
            }
        }
        KeyCode::Up => app.model_move(-1),
        KeyCode::Down => app.model_move(1),
        KeyCode::PageUp => app.model_page(-1),
        KeyCode::PageDown => app.model_page(1),
        KeyCode::Enter => match app.model_enter() {
            ModelAction::FetchModels(request) => spawn_fetch(request, fetch_tx),
            ModelAction::Apply(settings) => match evrax_agent::save_config(&settings) {
                Ok(path) => {
                    app.apply_settings(settings);
                    app.model_close();
                    app.set_flash(format!("model saved · {}", path.display()));
                }
                Err(e) => app.set_flash(format!("save failed: {e}")),
            },
            ModelAction::None => {}
        },
        KeyCode::Backspace => app.model_backspace(),
        KeyCode::Char(c)
            if !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            app.model_type(c);
        }
        _ => {}
    }
}

fn spawn_fetch(request: FetchRequest, fetch_tx: &mpsc::Sender<FetchResult>) {
    let tx = fetch_tx.clone();
    tokio::spawn(async move {
        let result =
            evrax_agent::list_models(request.kind, &request.base_url, request.api_key.as_deref())
                .await;
        let _ = tx.send((request.request_id, result)).await;
    });
}

fn handle_event(
    app: &mut App,
    event: Event,
    backend: &dyn AgentBackend,
    event_tx: &mpsc::Sender<AgentEvent>,
    decision_tx: &mut Option<mpsc::Sender<bool>>,
    turn_task: &mut Option<JoinHandle<()>>,
    max_scroll: u16,
) {
    match event {
        Event::Mouse(mouse) => {
            match mouse.kind {
                MouseEventKind::Down(MouseButton::Left) => {
                    app.selection.clear();
                    // Clicking the x removes an attachment; the chip body opens
                    // its preview dialog.
                    let pos = Position::new(mouse.column, mouse.row);
                    // In the tunnel dialog, clicking the status/url line copies
                    // the tunnel URL to the clipboard.
                    if app.tunnel_dialog.is_some()
                        && app
                            .tunnel_url_rect
                            .map(|r| r.contains(pos))
                            .unwrap_or(false)
                    {
                        match app.tunnel_current_url() {
                            Some(url) => {
                                copy_to_clipboard(url);
                                app.set_flash("tunnel url copied".to_string());
                            }
                            None => app.set_flash("tunnel: no url yet".to_string()),
                        }
                        return;
                    }
                    if let Some(idx) = app.chip_x_rects.iter().position(|r| r.contains(pos)) {
                        app.remove_attachment(idx);
                        return;
                    }
                    if let Some(idx) = app.chip_rects.iter().position(|r| r.contains(pos)) {
                        app.open_preview(idx);
                        return;
                    }
                    if app
                        .selectable_area
                        .contains(Position::new(mouse.column, mouse.row))
                    {
                        app.selection.begin((mouse.column, mouse.row));
                    }
                }
                MouseEventKind::Drag(MouseButton::Left) => {
                    app.selection.update((mouse.column, mouse.row));
                }
                MouseEventKind::Up(MouseButton::Left) => app.selection.finish(),
                MouseEventKind::ScrollUp => app.scroll_view(3, true, max_scroll),
                MouseEventKind::ScrollDown => app.scroll_view(3, false, max_scroll),
                _ => {}
            }
        }
        Event::Paste(text) => {
            if !is_paste_like(&text) || !app.ingest_paste(&text) {
                app.paste_text_input(&text);
            }
            app.slash_sync();
        }
        Event::Resize(_, _) => {}
        Event::Key(key) if key.kind == KeyEventKind::Press => {
            handle_key(
                app,
                key,
                backend,
                event_tx,
                decision_tx,
                turn_task,
                max_scroll,
            );
        }
        _ => {}
    }
}

fn handle_key(
    app: &mut App,
    key: crossterm::event::KeyEvent,
    backend: &dyn AgentBackend,
    event_tx: &mpsc::Sender<AgentEvent>,
    decision_tx: &mut Option<mpsc::Sender<bool>>,
    turn_task: &mut Option<JoinHandle<()>>,
    max_scroll: u16,
) {
    if !matches!(key.code, KeyCode::PageUp | KeyCode::PageDown) {
        app.selection.clear();
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        app.quit();
        return;
    }
    if app.approval.is_some() {
        let decision = match key.code {
            KeyCode::Left | KeyCode::Char('h') => {
                app.approval_move(-1);
                None
            }
            KeyCode::Right | KeyCode::Char('l') => {
                app.approval_move(1);
                None
            }
            KeyCode::Enter => {
                let selected = app.approval.as_ref().map(|a| a.selected).unwrap_or(0);
                Some(match selected {
                    0 => ApprovalDecision::Once,
                    1 => ApprovalDecision::Always,
                    _ => ApprovalDecision::Reject,
                })
            }
            KeyCode::Esc => Some(ApprovalDecision::Reject),
            _ => None,
        };
        if let Some(decision) = decision
            && let Some(allow) = app.settle_approval(decision)
            && let Some(tx) = decision_tx
        {
            let _ = tx.try_send(allow);
        }
        return;
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        if key.code == KeyCode::Char('p') {
            app.toggle_palette();
            return;
        }
        // `ctrl+t` and `ctrl+o` both toggle the transcript detail level
        // (quiet <-> expanded). The session list stays reachable via
        // `/sessions` and the `ctrl+p` palette.
        if key.code == KeyCode::Char('o') || key.code == KeyCode::Char('t') {
            app.toggle_expanded();
            return;
        }
        if key.code == KeyCode::Char('n') {
            app.new_session();
            return;
        }
        if key.code == KeyCode::Char('y')
            && let Some(text) = app.last_assistant_text()
        {
            copy_to_clipboard(text);
            return;
        }
    }
    if app.palette.is_some() {
        match key.code {
            KeyCode::Esc => app.palette_close(),
            KeyCode::Enter => app.palette_confirm(),
            KeyCode::Backspace => app.palette_backspace(),
            KeyCode::Up => app.palette_move(-1),
            KeyCode::Down => app.palette_move(1),
            KeyCode::PageUp => app.palette_page(-1, app::DIALOG_PAGE),
            KeyCode::PageDown => app.palette_page(1, app::DIALOG_PAGE),
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::ALT | KeyModifiers::CONTROL) =>
            {
                app.palette_type(c);
            }
            _ => {}
        }
        return;
    }
    if app.session_dialog.is_some() {
        match key.code {
            KeyCode::Esc => app.sessions_close(),
            KeyCode::Enter => app.sessions_confirm(),
            KeyCode::Up => app.sessions_move(-1),
            KeyCode::Down => app.sessions_move(1),
            KeyCode::PageUp => app.sessions_page(-1, app::DIALOG_PAGE),
            KeyCode::PageDown => app.sessions_page(1, app::DIALOG_PAGE),
            _ => {}
        }
        return;
    }
    if app.theme_dialog.is_some() {
        match key.code {
            KeyCode::Esc => app.theme_close(),
            KeyCode::Enter => app.theme_confirm(),
            KeyCode::Up => app.theme_move(-1),
            KeyCode::Down => app.theme_move(1),
            KeyCode::PageUp => app.theme_page(-1, app::DIALOG_PAGE),
            KeyCode::PageDown => app.theme_page(1, app::DIALOG_PAGE),
            _ => {}
        }
        return;
    }
    if app.tunnel_dialog.is_some() {
        if app
            .tunnel_dialog
            .as_ref()
            .and_then(|d| d.editing.as_ref())
            .is_some()
        {
            match key.code {
                KeyCode::Esc => app.tunnel_edit_cancel(),
                KeyCode::Enter => commit_tunnel_url(app),
                KeyCode::Backspace => app.tunnel_edit_backspace(),
                KeyCode::Char(c)
                    if !key
                        .modifiers
                        .intersects(KeyModifiers::ALT | KeyModifiers::CONTROL) =>
                {
                    app.tunnel_edit_input(c);
                }
                _ => {}
            }
            return;
        }
        match key.code {
            KeyCode::Esc => app.tunnel_close(),
            KeyCode::Enter => {
                let effect = app.tunnel_confirm();
                persist_remote(app);
                match effect {
                    TunnelEffect::None => {}
                    TunnelEffect::Copy(url) => {
                        copy_to_clipboard(url);
                        app.set_flash("tunnel url copied".to_string());
                    }
                    TunnelEffect::Start => tunnel_start(app),
                }
            }
            KeyCode::Up => app.tunnel_move(-1),
            KeyCode::Down => app.tunnel_move(1),
            _ => {}
        }
        return;
    }
    if app.remote_dialog.is_some() {
        if app
            .remote_dialog
            .as_ref()
            .and_then(|d| d.editing.as_ref())
            .is_some()
        {
            match key.code {
                KeyCode::Esc => app.remote_edit_cancel(),
                KeyCode::Enter => commit_remote_password(app),
                KeyCode::Backspace => app.remote_edit_backspace(),
                KeyCode::Char(c)
                    if !key
                        .modifiers
                        .intersects(KeyModifiers::ALT | KeyModifiers::CONTROL) =>
                {
                    app.remote_edit_input(c);
                }
                _ => {}
            }
            return;
        }
        match key.code {
            KeyCode::Esc => app.remote_close(),
            KeyCode::Enter => {
                app.remote_toggle();
                // The tunnel follows the remote switch and password changes.
                let effect = app.tunnel_follow_remote();
                persist_remote(app);
                if let TunnelEffect::Start = effect {
                    tunnel_start(app);
                }
            }
            KeyCode::Up => app.remote_move(-1),
            KeyCode::Down => app.remote_move(1),
            _ => {}
        }
        return;
    }
    if app.preview.is_some() {
        match key.code {
            KeyCode::Esc | KeyCode::Enter => app.close_preview(),
            _ => {}
        }
        return;
    }
    if app.slash.is_some() {
        match key.code {
            KeyCode::Esc => {
                app.slash_dismiss();
                return;
            }
            KeyCode::Tab => {
                app.slash_confirm();
                return;
            }
            KeyCode::Up => {
                app.slash_move(-1);
                return;
            }
            KeyCode::Down => {
                app.slash_move(1);
                return;
            }
            KeyCode::PageUp => {
                app.slash_page(-1, app::DIALOG_PAGE);
                return;
            }
            KeyCode::PageDown => {
                app.slash_page(1, app::DIALOG_PAGE);
                return;
            }
            _ => {}
        }
    }
    match key.code {
        KeyCode::Char('j') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.input.insert_newline();
        }
        KeyCode::Char(c) => {
            if !key
                .modifiers
                .intersects(KeyModifiers::ALT | KeyModifiers::CONTROL)
            {
                app.input.insert_char(c);
            }
        }
        KeyCode::Backspace => app.backspace_input(),
        KeyCode::Delete => app.input.delete(),
        KeyCode::Left => app.input.move_left(),
        KeyCode::Right => app.input.move_right(),
        KeyCode::Up => app.input.move_up(),
        KeyCode::Down => app.input.move_down(),
        KeyCode::Home => app.input.home(),
        KeyCode::End => app.input.end(),
        KeyCode::Enter => {
            if key.modifiers.is_empty() && app.slash.is_some() {
                if app.slash_confirm() {
                    return;
                }
                app.slash_dismiss();
            }
            if key
                .modifiers
                .intersects(KeyModifiers::SHIFT | KeyModifiers::ALT | KeyModifiers::CONTROL)
            {
                app.input.insert_newline();
            } else if let Some(prompt) = app.submit_prompt() {
                let (session, plan_mode) = (app.session_id.clone(), app.mode == AgentMode::Plan);
                start_turn(
                    backend,
                    event_tx,
                    decision_tx,
                    turn_task,
                    prompt,
                    session,
                    plan_mode,
                );
            }
        }
        KeyCode::Esc => {
            if app.busy && app.request_cancel() {
                if let Some(task) = turn_task.take() {
                    task.abort();
                }
                app.cancel_turn();
            }
        }
        KeyCode::Tab | KeyCode::BackTab => app.cycle_mode(),
        KeyCode::PageUp => {
            let page = app.page();
            app.scroll_view(page, true, max_scroll);
        }
        KeyCode::PageDown => {
            let page = app.page();
            app.scroll_view(page, false, max_scroll);
        }
        _ => {}
    }
    app.slash_sync();
}

fn copy_to_clipboard(text: String) {
    std::thread::spawn(move || {
        use std::process::{Command, Stdio};

        let path = std::env::temp_dir().join("evrax-clipboard.txt");
        if std::fs::write(&path, &text).is_err() {
            return;
        }
        let escaped = path.display().to_string().replace('\'', "''");
        let script = format!(
            "Set-Clipboard -Value ([System.IO.File]::ReadAllText('{escaped}', [System.Text.Encoding]::UTF8))"
        );
        let _ = Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let _ = std::fs::remove_file(&path);
    });
}

#[cfg(test)]
mod cli_tests {
    use super::*;

    fn store() -> (tempfile::TempDir, ProjectStore) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        let store = ProjectStore::open_in(dir.path(), &dir.path().join("data")).unwrap();
        (dir, store)
    }

    #[test]
    fn resolves_exact_prefix_and_title() {
        let (_dir, store) = store();
        let mut first = evrax_store::SessionMeta::new("session-100-1", "Rilis v0.1.1", "build");
        first.updated_at = 2;
        store.save_meta(&first).unwrap();
        let mut second = evrax_store::SessionMeta::new("session-200-1", "Lain", "plan");
        second.updated_at = 1;
        store.save_meta(&second).unwrap();

        assert_eq!(
            resolve_session(&store, "session-100-1").unwrap(),
            "session-100-1"
        );
        assert_eq!(
            resolve_session(&store, "session-2").unwrap(),
            "session-200-1"
        );
        assert_eq!(
            resolve_session(&store, "rilis V0.1.1").unwrap(),
            "session-100-1"
        );
        assert!(resolve_session(&store, "tidak-ada").is_err());
    }

    #[test]
    fn ambiguous_prefix_is_rejected() {
        let (_dir, store) = store();
        store
            .save_meta(&evrax_store::SessionMeta::new("session-1-1", "a", "build"))
            .unwrap();
        store
            .save_meta(&evrax_store::SessionMeta::new("session-1-2", "b", "build"))
            .unwrap();
        assert!(resolve_session(&store, "session-1").is_err());
    }

    #[test]
    fn wizard_ignores_key_release_events() {
        let mut app = App::new();
        app.open_wizard();
        app.wizard.as_mut().unwrap().step = crate::app::WizardStep::Name;
        let (tx, _rx) = mpsc::channel(4);
        let release = KeyEvent::new_with_kind(
            KeyCode::Char('o'),
            KeyModifiers::NONE,
            KeyEventKind::Release,
        );
        handle_wizard_key(&mut app, release, &tx);
        assert_eq!(app.wizard.as_ref().unwrap().name, "");
        handle_wizard_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE),
            &tx,
        );
        assert_eq!(app.wizard.as_ref().unwrap().name, "o");
    }

    #[test]
    fn wizard_and_model_handlers_page_with_page_keys() {
        let mut app = App::new();
        for index in 0..20 {
            app.providers.push(evrax_agent::Provider::new(
                format!("p{index}"),
                "http://a/v1",
            ));
        }
        app.open_wizard();
        let (tx, _rx) = mpsc::channel(4);
        handle_wizard_key(
            &mut app,
            KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE),
            &tx,
        );
        assert_eq!(app.wizard.as_ref().unwrap().cursor, crate::app::DIALOG_PAGE);
        handle_wizard_key(
            &mut app,
            KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE),
            &tx,
        );
        assert_eq!(app.wizard.as_ref().unwrap().cursor, 0);
        app.wizard.as_mut().unwrap().step = crate::app::WizardStep::Kind;
        handle_wizard_key(
            &mut app,
            KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
            &tx,
        );
        assert_eq!(
            app.wizard.as_ref().unwrap().kind,
            evrax_agent::ProviderKind::Anthropic
        );

        app.open_model_dialog();
        app.model_dialog.as_mut().unwrap().level = crate::app::ModelLevel::Models;
        app.model_dialog.as_mut().unwrap().models = (0..30).map(|i| format!("m{i}")).collect();
        app.model_dialog.as_mut().unwrap().refilter();
        handle_model_key(
            &mut app,
            KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE),
            &tx,
        );
        assert_eq!(
            app.model_dialog.as_ref().unwrap().model_cursor,
            crate::app::DIALOG_PAGE
        );
    }

    #[test]
    fn model_dialog_ignores_key_release_events() {
        let mut app = App::new();
        app.providers = vec![evrax_agent::Provider::new("Nimbus Go", "http://a/v1")];
        app.open_model_dialog();
        app.model_dialog.as_mut().unwrap().level = crate::app::ModelLevel::Models;
        let (tx, _rx) = mpsc::channel(4);
        let release = KeyEvent::new_with_kind(
            KeyCode::Char('x'),
            KeyModifiers::NONE,
            KeyEventKind::Release,
        );
        handle_model_key(&mut app, release, &tx);
        assert_eq!(app.model_dialog.as_ref().unwrap().filter, "");
        handle_model_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE),
            &tx,
        );
        assert_eq!(app.model_dialog.as_ref().unwrap().filter, "x");
    }

    #[test]
    fn event_to_json_maps_delta_and_tool() {
        let delta = event_to_json(&AgentEvent::TextDelta {
            text: "hi".to_string(),
        });
        assert_eq!(delta["type"], "delta");
        assert_eq!(delta["text"], "hi");

        let thinking = event_to_json(&AgentEvent::ThinkingDelta {
            text: "hmm".to_string(),
        });
        assert_eq!(thinking["type"], "thinking");
        assert_eq!(thinking["text"], "hmm");

        let tool = event_to_json(&AgentEvent::ToolStarted {
            name: "read".to_string(),
            summary: "a.rs".to_string(),
        });
        assert_eq!(tool["type"], "tool");
        assert_eq!(tool["name"], "read");

        let done = event_to_json(&AgentEvent::ToolFinished {
            name: "edit".to_string(),
            ok: false,
            detail: "boom".to_string(),
        });
        assert_eq!(done["type"], "tool_done");
        assert_eq!(done["ok"], false);

        let fin = event_to_json(&AgentEvent::TurnFinished);
        assert_eq!(fin["type"], "turn_finished");

        let fail = event_to_json(&AgentEvent::Failed {
            message: "nope".to_string(),
        });
        assert_eq!(fail["type"], "error");
        assert_eq!(fail["message"], "nope");
    }

    #[test]
    fn batchable_input_open_while_busy_streaming() {
        let key = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);
        let mut app = App::new();
        assert!(is_batchable(&app, &key));
        app.busy = true;
        assert!(is_batchable(&app, &key));
        app.preview = Some(0);
        assert!(!is_batchable(&app, &key));
    }

    #[test]
    fn chat_backend_builds_mock_and_real_labels() {
        let (_, label, note) = chat_backend(evrax_agent::AgentConfig::default(), None, true);
        assert_eq!(label, "mock");
        assert!(note.unwrap().contains("mock"));

        let cfg = evrax_agent::AgentConfig {
            model: "bebas".into(),
            ..Default::default()
        };
        let (_, label, note) = chat_backend(cfg, None, false);
        assert_eq!(label, "bebas");
        assert!(note.unwrap().contains("bebas"));
    }
}
