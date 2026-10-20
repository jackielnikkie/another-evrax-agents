mod app;
mod image;
mod markdown;
mod theme;
mod ui;
mod window;

use std::io::Stdout;
use std::time::Duration;

use anyhow::{Context, Result};
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, Event,
    EventStream, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use evrax_core::{AgentBackend, AgentEvent, MockBackend, TurnRequest};
use futures_util::StreamExt;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Position;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::app::{AgentMode, App, ApprovalDecision, TunnelEditResult, TunnelEffect};

#[tokio::main]
async fn main() -> Result<()> {
    setup_panic_hook();
    if let Some(idx) = theme::load_saved() {
        theme::set_index(idx);
    }
    let mut terminal = init_terminal().context("evrax needs an interactive terminal")?;
    let result = run(&mut terminal).await;
    let restored = restore_terminal(&mut terminal);
    restored.context("failed to restore terminal state")?;
    result
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

fn setup_panic_hook() {
    let original = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(
            std::io::stdout(),
            DisableBracketedPaste,
            DisableMouseCapture,
            LeaveAlternateScreen
        );
        original(info);
    }));
}

async fn run(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    let loaded = evrax_agent::config_from_args();
    let (backend, backend_note): (Box<dyn AgentBackend>, String) = if loaded.mock {
        (Box::new(MockBackend::new()), "backend · mock".to_string())
    } else {
        match evrax_agent::ChatBackend::new(loaded.config.clone()) {
            Ok(real) => {
                let note = format!("backend · {} · {}", real.model(), real.base_url());
                (Box::new(real), note)
            }
            Err(e) => (
                Box::new(MockBackend::new()),
                format!("backend gagal ({e}) · pakai mock"),
            ),
        }
    };
    let (event_tx, mut event_rx) = mpsc::channel::<AgentEvent>(64);
    let mut turn_task: Option<JoinHandle<()>> = None;
    let mut app = App::new();
    if !backend_note.is_empty() {
        app.set_flash(backend_note);
    }
    let mut decision_tx: Option<mpsc::Sender<bool>> = None;
    let mut events = EventStream::new();
    let mut ticker = tokio::time::interval(Duration::from_millis(250));
    let debug_window = std::env::var_os("EVRAX_DEBUG_MAXIMIZED").is_some();
    let mut last_maximized: Option<Option<bool>> = None;

    while !app.should_quit {
        terminal.draw(|frame| ui::render(frame, &mut app))?;
        flush_kitty(terminal, &mut app);
        let size = terminal.size()?;
        app.set_viewport(size.height.saturating_sub(6));
        if let Some(state) = window::window_state() {
            app.maximized = Some(state);
        }
        if debug_window && last_maximized != Some(app.maximized) {
            last_maximized = Some(app.maximized);
            log_window_debug(app.maximized);
        }
        let max_scroll = ui::max_scroll_for(&app, size.width, size.height);
        app.clamp_scroll(max_scroll);
        if let Some(up) = app.selection_edge() {
            app.scroll_view(1, up, max_scroll);
        }
        if let Some(text) = app.copied_text.take() {
            copy_to_clipboard(text);
        }

        tokio::select! {
            event = events.next() => {
                if let Some(Ok(Event::Key(key))) = event {
                    if is_batchable(&app, &key)
                        && let KeyCode::Char(c) = key.code
                        && !key.modifiers.intersects(KeyModifiers::ALT | KeyModifiers::CONTROL)
                    {
                        let (chars, leftover) = drain_burst(&mut events, c).await;
                        if !chars.is_empty() {
                            // Terminal paste without bracketed-paste support arrives
                            // as a key burst. Screenshot pastes show up as a temp
                            // file path (or as raw pixels only in the clipboard), so
                            // try the clipboard image first — opencode-style — then
                            // fall back to path detection, then literal text.
                            if is_paste_like(&chars) {
                                if !app.ingest_paste(&chars) {
                                    app.input.insert_str(&chars);
                                }
                            } else {
                                app.input.insert_str(&chars);
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
                } else if let Some(Ok(event)) = event {
                    handle_event(
                        &mut app,
                        event,
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
                if app.take_auto_approval()
                    && let Some(tx) = &decision_tx
                {
                    let _ = tx.try_send(true);
                }
            }
            _ = ticker.tick() => {
                app.tick();
                drain_remote_prompts(&mut app, &*backend, &event_tx, &mut decision_tx, &mut turn_task);
                drain_remote_approvals(&mut app, &mut decision_tx);
            }
        }
    }

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
        && !app.busy
}

async fn drain_burst(
    events: &mut EventStream,
    first: char,
) -> (String, Option<Event>) {
    let mut chars = String::from(first);
    let deadline = Duration::from_millis(3);
    loop {
        match tokio::time::timeout(deadline, events.next()).await {
            Ok(Some(Ok(Event::Key(key)))) => {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                if key.modifiers.intersects(KeyModifiers::ALT | KeyModifiers::CONTROL) {
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
    if let Some(prompt) = app.submit_external_prompt(text) {
        let (session, plan_mode) = (app.session_name.clone(), app.mode == AgentMode::Plan);
        start_turn(backend, event_tx, decision_tx, turn_task, prompt, session, plan_mode);
    }
}

/// Applies an accept/reject decision sent from the web while the approval
/// dialog is open (the dialog may also be settled from the terminal).
fn drain_remote_approvals(app: &mut App, decision_tx: &mut Option<mpsc::Sender<bool>>) {
    if app.approval.is_none() {
        return;
    }
    let decision = {
        let Some(session_id) = app.remote.as_ref().map(|r| r.session_id.clone()) else {
            return;
        };
        let Some(server) = app.remote_server() else {
            return;
        };
        evrax_remote::take_pending_approval(server, &session_id)
    };
    let Some(decision) = decision else { return };
    if let Some(allow) = app.settle_remote_approval(&decision)
        && let Some(tx) = decision_tx
    {
        let _ = tx.try_send(allow);
    }
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
                        && app.tunnel_url_rect.map(|r| r.contains(pos)).unwrap_or(false)
                    {
                        match app.tunnel_current_url() {
                            Some(url) => {
                                copy_to_clipboard(url);
                                app.set_flash("tunnel url copied".to_string());
                            }
                            None => app.set_flash("tunnel: belum ada url".to_string()),
                        }
                        return;
                    }
                    if let Some(idx) = app.chip_x_rects.iter().position(|r| r.contains(pos)) {
                        app.remove_attachment(idx);
                        return;
                    }
                    if let Some(idx) = app
                        .chip_rects
                        .iter()
                        .position(|r| r.contains(pos))
                    {
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
            if is_paste_like(&text) && app.ingest_paste(&text) {
                app.slash_sync();
            } else {
                app.paste_text(&text);
                app.slash_sync();
            }
        }
        Event::Resize(_, _) => {}
        Event::Key(key) if key.kind == KeyEventKind::Press => {
            handle_key(app, key, backend, event_tx, decision_tx, turn_task, max_scroll);
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
        if key.code == KeyCode::Char('o') {
            app.open_sessions();
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
                if !key.modifiers.intersects(KeyModifiers::ALT | KeyModifiers::CONTROL) =>
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
        if app.tunnel_dialog.as_ref().and_then(|d| d.editing.as_ref()).is_some() {
            match key.code {
                KeyCode::Esc => app.tunnel_edit_cancel(),
                KeyCode::Enter => commit_tunnel_url(app),
                KeyCode::Backspace => app.tunnel_edit_backspace(),
                KeyCode::Char(c)
                    if !key.modifiers.intersects(KeyModifiers::ALT | KeyModifiers::CONTROL) =>
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
        if app.remote_dialog.as_ref().and_then(|d| d.editing.as_ref()).is_some() {
            match key.code {
                KeyCode::Esc => app.remote_edit_cancel(),
                KeyCode::Enter => commit_remote_password(app),
                KeyCode::Backspace => app.remote_edit_backspace(),
                KeyCode::Char(c)
                    if !key.modifiers.intersects(KeyModifiers::ALT | KeyModifiers::CONTROL) =>
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
            if !key.modifiers.intersects(KeyModifiers::ALT | KeyModifiers::CONTROL) {
                app.input.insert_char(c);
            }
        }
        KeyCode::Backspace => app.input.backspace(),
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
                let (session, plan_mode) = (app.session_name.clone(), app.mode == AgentMode::Plan);
                start_turn(backend, event_tx, decision_tx, turn_task, prompt, session, plan_mode);
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
