//! `evrax-remote` — central HTTP server for the evrax remote web feature.
//!
//! v1 scope: session registry + password gate + a minimal SSE stub + a
//! single-file Vue 3 web UI. Full realtime streaming is wired in a later stage.

use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

pub mod tunnel;

/// Permit level granted to a remote session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Permit {
    Read,
    Write,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteConfig {
    #[serde(default = "default_bind")]
    pub bind: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_password")]
    pub password: String,
    #[serde(default)]
    pub public_url: String,
    /// Chosen tunnel provider: "", "quick", "domain", "tailscale" or "manual".
    #[serde(default)]
    pub tunnel_mode: String,
    /// Hostname for the custom-domain tunnel (e.g. `evrax.example.com`).
    #[serde(default)]
    pub tunnel_hostname: String,
}

fn default_bind() -> String {
    "127.0.0.1".into()
}
fn default_port() -> u16 {
    444
}
fn default_password() -> String {
    "12345678".into()
}

impl Default for RemoteConfig {
    fn default() -> Self {
        Self {
            bind: default_bind(),
            port: default_port(),
            password: default_password(),
            public_url: String::new(),
            tunnel_mode: String::new(),
            tunnel_hostname: String::new(),
        }
    }
}

/// One registered remote session.
#[derive(Debug, Clone, Serialize)]
pub struct Session {
    pub id: String,
    pub name: String,
    pub permit: Permit,
    /// Authoritative agent mode, "plan" | "build". Owned by the TUI (it
    /// confirms via `set_session_mode`); the web only requests changes.
    pub mode: String,
}

/// A realtime event pushed to a session's web subscribers, then serialized to
/// an SSE frame. This is the contract the Vue web UI consumes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RemoteEvent {
    Connected { session: String },
    TurnStart { turn: String },
    User { turn: String, text: String },
    Delta { turn: String, text: String },
    Thinking { turn: String, text: String },
    Tool { turn: String, name: String, status: String, detail: Option<String> },
    Approval { turn: String, name: String, summary: String },
    ApprovalResolved { turn: String, decision: String },
    /// Authoritative session mode changed ("plan" | "build"). Broadcast by the
    /// owning TUI via `set_session_mode`; carries no turn id.
    Mode { mode: String },
    TurnDone { turn: String },
    Error { turn: Option<String>, message: String },
    Done,
}

impl RemoteEvent {
    /// Serialize to a single SSE frame (`event: <name>\ndata: <json>\n\n`).
    fn to_sse(&self) -> String {
        let (name, payload) = match self {
            RemoteEvent::Connected { session } => ("connected", serde_json::json!({"type":"connected","session":session})),
            RemoteEvent::TurnStart { turn } => ("turn_start", serde_json::json!({"type":"turn_start","turn":turn})),
            RemoteEvent::User { turn, text } => ("user", serde_json::json!({"type":"user","turn":turn,"text":text})),
            RemoteEvent::Delta { turn, text } => ("delta", serde_json::json!({"type":"delta","turn":turn,"text":text})),
            RemoteEvent::Thinking { turn, text } => ("thinking", serde_json::json!({"type":"thinking","turn":turn,"text":text})),
            RemoteEvent::Tool { turn, name, status, detail } => ("tool", serde_json::json!({"type":"tool","turn":turn,"name":name,"status":status,"detail":detail})),
            RemoteEvent::Approval { turn, name, summary } => ("approval", serde_json::json!({"type":"approval","turn":turn,"name":name,"summary":summary})),
            RemoteEvent::ApprovalResolved { turn, decision } => ("approval_resolved", serde_json::json!({"type":"approval_resolved","turn":turn,"decision":decision})),
            RemoteEvent::Mode { mode } => ("mode", serde_json::json!({"type":"mode","mode":mode})),
            RemoteEvent::TurnDone { turn } => ("turn_done", serde_json::json!({"type":"turn_done","turn":turn})),
            RemoteEvent::Error { turn, message } => ("error", serde_json::json!({"type":"error","turn":turn,"message":message})),
            RemoteEvent::Done => ("done", serde_json::json!({"type":"done"})),
        };
        let json = serde_json::to_string(&payload).unwrap_or_else(|_| "{}".into());
        format!("event: {name}\ndata: {json}\n\n")
    }
}

/// Internal registry entry: the public session plus its live SSE subscribers.
struct Registered {
    session: Session,
    subs: Arc<Mutex<Vec<Sender<RemoteEvent>>>>,
    pending_prompts: Arc<Mutex<VecDeque<String>>>,
    /// Accept/reject decisions sent from the web while an approval is open.
    pending_approvals: Arc<Mutex<VecDeque<String>>>,
    /// Mode switches requested from the web, awaiting the owning TUI.
    pending_modes: Arc<Mutex<VecDeque<String>>>,
}

/// Password policy: with the default password, only loopback clients may use
/// any endpoint other than `/health`.
pub fn allow_remote(password_default: bool, ip: Option<IpAddr>) -> bool {
    if !password_default {
        return true;
    }
    match ip {
        Some(ip) => ip.is_loopback(),
        // No remote address available cannot be proven loopback.
        None => false,
    }
}

pub fn is_default_password(password: &str) -> bool {
    password == "12345678"
}

#[derive(Deserialize)]
struct RegisterBody {
    id: String,
    #[serde(default)]
    name: String,
    permit: Permit,
    #[serde(default)]
    mode: String,
}

#[derive(Deserialize)]
struct LoginBody {
    password: String,
}

#[derive(Deserialize)]
struct PromptBody {
    #[serde(default)]
    prompt: String,
}

#[derive(Deserialize)]
struct ApprovalBody {
    #[serde(default)]
    decision: String,
}

#[derive(Deserialize)]
struct ModeBody {
    #[serde(default)]
    mode: String,
}

/// Normalize an incoming mode to the closed set the rest of the system assumes.
/// Anything other than `"plan"` (including a missing field) becomes `"build"`.
fn normalize_mode(mode: &str) -> String {
    if mode == "plan" { "plan".to_string() } else { "build".to_string() }
}

/// Shared server state.
#[derive(Clone)]
struct State {
    config: Arc<RemoteConfig>,
    /// Authoritative password; interior-mutable so it can change without a
    /// server restart.
    password: Arc<Mutex<String>>,
    public_url: Arc<Mutex<String>>,
    registry: Arc<Mutex<HashMap<String, Registered>>>,
    refcount: Arc<AtomicUsize>,
    token: Arc<Mutex<Option<(String, Instant)>>>,
    login_attempts: Arc<Mutex<HashMap<IpAddr, (u32, Instant)>>>,
    alive: Arc<AtomicBool>,
}

/// Handle to a running server thread. Dropping it asks the server to stop
/// once no sessions remain (refcount == 0 and registry empty).
pub struct RemoteServer {
    state: State,
    shutdown: Sender<()>,
}

impl RemoteServer {
    /// Whether the server thread is still serving requests (i.e. the bound
    /// socket is live). Returns `false` if the bind failed or the thread has
    /// already shut down.
    pub fn is_running(&self) -> bool {
        self.state.alive.load(Ordering::SeqCst)
    }
}

impl Drop for RemoteServer {
    fn drop(&mut self) {
        let _ = self.shutdown.send(());
    }
}

/// Start the HTTP server on a background thread.
///
/// Binds synchronously so a port conflict is detected immediately: on bind
/// failure no thread is spawned and the returned handle reports
/// `is_running() == false`. Otherwise the handle is returned immediately.
pub fn start(config: RemoteConfig) -> RemoteServer {
    let state = State {
        config: Arc::new(config.clone()),
        password: Arc::new(Mutex::new(config.password.clone())),
        public_url: Arc::new(Mutex::new(config.public_url.clone())),
        registry: Arc::new(Mutex::new(HashMap::new())),
        refcount: Arc::new(AtomicUsize::new(0)),
        token: Arc::new(Mutex::new(None)),
        login_attempts: Arc::new(Mutex::new(HashMap::new())),
        alive: Arc::new(AtomicBool::new(false)),
    };
    let (tx, rx) = channel::<()>();
    let addr = format!("{}:{}", state.config.bind, state.config.port);

    match tiny_http::Server::http(&addr) {
        Ok(server) => {
            state.alive.store(true, Ordering::SeqCst);
            let thread_state = state.clone();
            thread::Builder::new()
                .name("evrax-remote".into())
                .spawn(move || run_server(thread_state, server, rx))
                .expect("failed to spawn evrax-remote server thread");
        }
        Err(e) => eprintln!("evrax-remote: failed to bind {addr}: {e}"),
    }

    RemoteServer { state, shutdown: tx }
}

fn run_server(state: State, server: tiny_http::Server, rx: std::sync::mpsc::Receiver<()>) {
    // Clear the alive flag on any exit path, including a panic unwind.
    struct AliveGuard(Arc<AtomicBool>);
    impl Drop for AliveGuard {
        fn drop(&mut self) {
            self.0.store(false, Ordering::SeqCst);
        }
    }
    let _alive = AliveGuard(state.alive.clone());

    loop {
        match server.recv_timeout(Duration::from_millis(200)) {
            Ok(Some(req)) => {
                if let Err(e) = handle(&state, req) {
                    eprintln!("evrax-remote: request error: {e}");
                }
            }
            Ok(None) => {}
            Err(e) => eprintln!("evrax-remote: recv error: {e}"),
        }

        let idle = state.refcount.load(Ordering::SeqCst) == 0
            && state.registry.lock().map(|r| r.is_empty()).unwrap_or(false);
        // Stop only when idle AND the owner handle has been dropped
        // (`Disconnected`) or an explicit stop was sent (`Ok`). A live handle
        // yields `Empty`, which must NOT end the loop, so the server keeps
        // serving even while no session is registered.
        if idle {
            match rx.try_recv() {
                Ok(()) | Err(TryRecvError::Disconnected) => break,
                Err(TryRecvError::Empty) => {}
            }
        }
    }
}

fn read_body(req: &mut tiny_http::Request) -> String {
    let mut buf = String::new();
    let _ = req.as_reader().read_to_string(&mut buf);
    buf
}

fn respond_json<T: Serialize>(req: tiny_http::Request, status: u16, value: &T) -> std::io::Result<()> {
    let body = serde_json::to_string(value).unwrap_or_else(|_| "{}".into());
    let header = tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).expect("static header");
    req.respond(tiny_http::Response::from_string(body).with_status_code(status).with_header(header))
}

/// Push an event to every live subscriber of a session, dropping dead senders.
fn broadcast(subs: &Arc<Mutex<Vec<Sender<RemoteEvent>>>>, event: RemoteEvent) {
    if let Ok(mut list) = subs.lock() {
        list.retain(|tx| tx.send(event.clone()).is_ok());
    }
}

/// Serve a long-lived SSE stream for one session on a dedicated thread so the
/// single-threaded request loop keeps polling. The connection stays open until
/// the client disconnects or the session is deregistered (senders dropped).
///
/// Uses `Request::into_writer` rather than `Response` streaming: tiny_http's
/// `respond` buffers the body in a `BufWriter` and only flushes once the body
/// `Read` returns EOF, which would hold every SSE frame until the idle timeout.
/// Taking the raw writer lets us emit the HTTP preamble ourselves and flush
/// after each frame, so events arrive in real time.
fn spawn_stream(req: tiny_http::Request, id: String, subs: Arc<Mutex<Vec<Sender<RemoteEvent>>>>) -> std::io::Result<()> {
    let (tx, rx) = channel::<RemoteEvent>();
    if let Ok(mut list) = subs.lock() {
        list.push(tx);
    }

    let mut stream = req.into_writer();

    // Manual HTTP preamble: no Content-Length, body ends when the socket closes.
    const HEAD: &str = "HTTP/1.1 200 OK\r\n\
                        Content-Type: text/event-stream\r\n\
                        Cache-Control: no-cache\r\n\
                        Connection: keep-alive\r\n\
                        X-Accel-Buffering: no\r\n\r\n";
    if stream.write_all(HEAD.as_bytes()).is_err() {
        return Ok(());
    }

    let connected = RemoteEvent::Connected { session: id }.to_sse();
    if stream.write_all(connected.as_bytes()).is_err() {
        return Ok(());
    }
    if stream.flush().is_err() {
        return Ok(());
    }

    // 15s keep-alive comment: also detects a dead client and keeps proxies open.
    loop {
        match rx.recv_timeout(Duration::from_secs(15)) {
            Ok(ev) => {
                if stream.write_all(ev.to_sse().as_bytes()).is_err() {
                    break;
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                if stream.write_all(b": ping\n\n").is_err() {
                    break;
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
        if stream.flush().is_err() {
            break;
        }
    }
    Ok(())
}

fn handle(state: &State, mut req: tiny_http::Request) -> std::io::Result<()> {
    let method = req.method().clone();
    let path = req.url().split('?').next().unwrap_or("").to_string();

    let remote_ip = req.remote_addr().map(|a| a.ip());
    let current_password = state.password.lock().map(|p| p.clone()).unwrap_or_default();
    let password_default = is_default_password(&current_password);

    // Password policy gate (health is always reachable).
    if path != "/health" && !allow_remote(password_default, remote_ip) {
        return respond_json(req, 403, &serde_json::json!({"error": "tunnel requires non-default password"}));
    }

    // Public routes need no token. Everything else does: a tunneled request
    // from the same host looks like loopback, so the password gate alone cannot
    // protect it — the bearer token is the real authority.
    let is_public =
        path == "/health" || path == "/login" || (matches!(method, tiny_http::Method::Get) && path == "/");
    if !is_public && !auth_token_valid(state, &req) {
        return respond_json(req, 401, &serde_json::json!({"error": "unauthorized"}));
    }

    use tiny_http::Method;

    match (method, path.as_str()) {
        (Method::Get, "/") => {
            let header =
                tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..]).expect("static header");
            req.respond(tiny_http::Response::from_string(WEBUI).with_header(header))
        }

        (Method::Get, "/health") => health_ok(state, req),

        (Method::Get, "/api/sessions") => {
            let sessions: Vec<Session> = state
                .registry
                .lock()
                .map(|r| r.values().map(|reg| reg.session.clone()).collect())
                .unwrap_or_default();
            respond_json(req, 200, &sessions)
        }

        (Method::Post, "/login") => {
            if !login_allowed(state, remote_ip) {
                return respond_json(req, 429, &serde_json::json!({"error": "too many attempts, try again later"}));
            }
            let body = read_body(&mut req);
            let parsed: Option<LoginBody> = serde_json::from_str(&body).ok();
            match parsed {
                Some(b) if ct_eq(&b.password, &current_password) => {
                    note_login_success(state, remote_ip);
                    let token = generate_token();
                    if let Ok(mut t) = state.token.lock() {
                        *t = Some((token.clone(), Instant::now()));
                    }
                    respond_json(req, 200, &serde_json::json!({ "token": token }))
                }
                _ => {
                    note_login_failure(state, remote_ip);
                    respond_json(req, 401, &serde_json::json!({"error": "invalid password"}))
                }
            }
        }

        (Method::Post, "/logout") => {
            if let Ok(mut t) = state.token.lock() {
                *t = None;
            }
            respond_json(req, 200, &serde_json::json!({"ok": true}))
        }

        (Method::Post, "/register") => {
            let body = read_body(&mut req);
            match serde_json::from_str::<RegisterBody>(&body) {
                Ok(b) if !b.id.trim().is_empty() => {
                    let name = if b.name.trim().is_empty() { b.id.clone() } else { b.name };
                    let mode = normalize_mode(&b.mode);
                    let session = Session { id: b.id.clone(), name, permit: b.permit, mode };
                    if let Ok(mut reg) = state.registry.lock() {
                        let existed = reg.contains_key(&session.id);
                        if existed {
                            if let Some(r) = reg.get_mut(&session.id) {
                                r.session = session;
                            }
                        } else {
                            let registered = Registered {
                                session,
                                subs: Arc::new(Mutex::new(Vec::new())),
                                pending_prompts: Arc::new(Mutex::new(VecDeque::new())),
                                pending_approvals: Arc::new(Mutex::new(VecDeque::new())),
                                pending_modes: Arc::new(Mutex::new(VecDeque::new())),
                            };
                            reg.insert(b.id.clone(), registered);
                            state.refcount.fetch_add(1, Ordering::SeqCst);
                        }
                    }
                    respond_json(req, 200, &serde_json::json!({"ok": true}))
                }
                _ => respond_json(req, 400, &serde_json::json!({"error": "invalid register body"})),
            }
        }

        (Method::Delete, _) if path.starts_with("/register/") => {
            let id = path.trim_start_matches("/register/").to_string();
            let removed = state.registry.lock().map(|mut reg| reg.remove(&id).is_some()).unwrap_or(false);
            if removed {
                state.refcount.fetch_sub(1, Ordering::SeqCst);
                respond_json(req, 200, &serde_json::json!({"ok": true}))
            } else {
                respond_json(req, 404, &serde_json::json!({"error": "unknown session"}))
            }
        }

        (Method::Get, _) if path.starts_with("/api/session/") && path.ends_with("/stream") => {
            let id = path
                .trim_start_matches("/api/session/")
                .trim_end_matches("/stream")
                .to_string();
            let subs = state
                .registry
                .lock()
                .ok()
                .and_then(|r| r.get(&id).map(|reg| reg.subs.clone()));
            let Some(subs) = subs else {
                return respond_json(req, 404, &serde_json::json!({"error": "unknown session"}));
            };
            // Serve on a dedicated thread so the request loop keeps polling.
            thread::Builder::new()
                .name(format!("evrax-sse-{id}"))
                .spawn(move || {
                    if let Err(e) = spawn_stream(req, id, subs) {
                        eprintln!("evrax-remote: stream error: {e}");
                    }
                })
                .expect("failed to spawn stream thread");
            Ok(())
        }

        (Method::Post, _) if path.starts_with("/api/session/") && path.ends_with("/prompt") => {
            let id = path
                .trim_start_matches("/api/session/")
                .trim_end_matches("/prompt")
                .to_string();
            let body = read_body(&mut req);
            let parsed = serde_json::from_str::<PromptBody>(&body).ok();
            let permit = state
                .registry
                .lock()
                .ok()
                .and_then(|r| r.get(&id).map(|reg| reg.session.permit));
            match (permit, parsed) {
                (None, _) => respond_json(req, 404, &serde_json::json!({"error": "unknown session"})),
                (Some(Permit::Read), _) => {
                    respond_json(req, 403, &serde_json::json!({"error": "session is read-only"}))
                }
                (Some(Permit::Write), Some(p)) if !p.prompt.trim().is_empty() => {
                    // Queue the prompt for the owning TUI and echo it as a
                    // `user` event so every open tab renders the same bubble.
                    let turn = format!("remote_{}", REMOTE_TURN_SEQ.fetch_add(1, Ordering::SeqCst));
                    let subs = state.registry.lock().ok().and_then(|r| {
                        r.get(&id).map(|reg| {
                            if let Ok(mut q) = reg.pending_prompts.lock() {
                                q.push_back(p.prompt.clone());
                            }
                            reg.subs.clone()
                        })
                    });
                    if let Some(subs) = subs {
                        // Only echo the user bubble; the TUI emits its own
                        // TurnStart/Delta/... stream when it runs the prompt.
                        broadcast(&subs, RemoteEvent::User { turn: turn.clone(), text: p.prompt });
                    }
                    respond_json(req, 200, &serde_json::json!({"ok": true, "turn": turn}))
                }
                (Some(Permit::Write), _) => {
                    respond_json(req, 400, &serde_json::json!({"error": "empty prompt"}))
                }
            }
        }

        (Method::Post, _) if path.starts_with("/api/session/") && path.ends_with("/approval") => {
            let id = path
                .trim_start_matches("/api/session/")
                .trim_end_matches("/approval")
                .to_string();
            let body = read_body(&mut req);
            let parsed = serde_json::from_str::<ApprovalBody>(&body).ok();
            let queue = state
                .registry
                .lock()
                .ok()
                .and_then(|r| r.get(&id).map(|reg| reg.pending_approvals.clone()));
            match (queue, parsed) {
                (None, _) => respond_json(req, 404, &serde_json::json!({"error": "unknown session"})),
                (Some(q), Some(p))
                    if matches!(p.decision.as_str(), "once" | "always" | "reject") =>
                {
                    // The owning TUI drains this on its next tick and settles
                    // the open approval dialog; it then broadcasts
                    // `approval_resolved` so every tab updates.
                    if let Ok(mut q) = q.lock() {
                        q.push_back(p.decision);
                    }
                    respond_json(req, 200, &serde_json::json!({"ok": true}))
                }
                (Some(_), _) => {
                    respond_json(req, 400, &serde_json::json!({"error": "invalid decision"}))
                }
            }
        }

        (Method::Post, _) if path.starts_with("/api/session/") && path.ends_with("/mode") => {
            let id = path
                .trim_start_matches("/api/session/")
                .trim_end_matches("/mode")
                .to_string();
            let body = read_body(&mut req);
            let parsed = serde_json::from_str::<ModeBody>(&body).ok();
            let entry = state
                .registry
                .lock()
                .ok()
                .and_then(|r| r.get(&id).map(|reg| (reg.session.permit, reg.pending_modes.clone())));
            match (entry, parsed) {
                (None, _) => respond_json(req, 404, &serde_json::json!({"error": "unknown session"})),
                (Some((Permit::Read, _)), _) => {
                    respond_json(req, 403, &serde_json::json!({"error": "session is read-only"}))
                }
                (Some((_, q)), Some(m)) if m.mode == "plan" || m.mode == "build" => {
                    // Only queue the request; the owning TUI applies it and
                    // confirms via `set_session_mode`, which broadcasts the
                    // final state to every tab.
                    if let Ok(mut q) = q.lock() {
                        q.push_back(m.mode);
                    }
                    respond_json(req, 202, &serde_json::json!({"ok": true}))
                }
                (Some(_), _) => respond_json(req, 400, &serde_json::json!({"error": "invalid mode"})),
            }
        }

        _ => respond_json(req, 404, &serde_json::json!({"error": "not found"})),
    }
}

fn health_ok(state: &State, req: tiny_http::Request) -> std::io::Result<()> {
    let n = state.registry.lock().map(|r| r.len()).unwrap_or(0);
    let header = tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).expect("static header");
    let body = format!("{{\"ok\":true,\"sessions\":{n}}}");
    req.respond(tiny_http::Response::from_string(body).with_header(header))
}

/// Bearer/session token TTL. After this the client must log in again.
const TOKEN_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// Max failed logins within `LOGIN_WINDOW` before a temporary lockout.
const LOGIN_MAX: u32 = 5;
const LOGIN_WINDOW: Duration = Duration::from_secs(300);

/// 128-bit token from OS-seeded hasher keys (`RandomState`) + time + counter.
/// Not a CSPRNG call, but unguessable without the per-process random seed.
fn generate_token() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    use std::time::SystemTime;

    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let a = {
        let mut h = RandomState::new().build_hasher();
        h.write_u128(nanos);
        h.write_usize(TOKEN_SEQ.fetch_add(1, Ordering::SeqCst));
        h.finish()
    };
    let b = {
        let mut h = RandomState::new().build_hasher();
        h.write_u64(a);
        h.write_usize(TOKEN_SEQ.fetch_add(1, Ordering::SeqCst));
        h.finish()
    };
    format!("{a:016x}{b:016x}")
}

static TOKEN_SEQ: AtomicUsize = AtomicUsize::new(0);

/// Constant-time string compare (avoids leaking length/prefix via timing).
fn ct_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for i in 0..a.len() {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

/// Token from the `Authorization: Bearer` header, else the `?token=` query
/// (EventSource cannot set headers, so the stream carries it in the URL).
fn request_token(req: &tiny_http::Request) -> Option<String> {
    if let Some(h) = req.headers().iter().find(|h| h.field.equiv("Authorization"))
        && let Some(t) = h.value.as_str().strip_prefix("Bearer ")
    {
        return Some(t.to_string());
    }
    let query = req.url().split_once('?').map(|(_, q)| q).unwrap_or("");
    query
        .split('&')
        .find_map(|kv| kv.strip_prefix("token=").map(|v| v.to_string()))
}

fn auth_token_valid(state: &State, req: &tiny_http::Request) -> bool {
    let Some(tok) = request_token(req) else {
        return false;
    };
    match state.token.lock() {
        Ok(guard) => match guard.as_ref() {
            Some((t, at)) => ct_eq(t, &tok) && at.elapsed() < TOKEN_TTL,
            None => false,
        },
        Err(_) => false,
    }
}

fn login_key(ip: Option<IpAddr>) -> IpAddr {
    ip.unwrap_or(IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED))
}

fn login_allowed(state: &State, ip: Option<IpAddr>) -> bool {
    let key = login_key(ip);
    match state.login_attempts.lock() {
        Ok(map) => match map.get(&key) {
            Some((n, at)) if at.elapsed() < LOGIN_WINDOW => *n < LOGIN_MAX,
            _ => true,
        },
        Err(_) => true,
    }
}

fn note_login_failure(state: &State, ip: Option<IpAddr>) {
    let key = login_key(ip);
    if let Ok(mut map) = state.login_attempts.lock() {
        let entry = map.entry(key).or_insert((0, Instant::now()));
        if entry.1.elapsed() >= LOGIN_WINDOW {
            *entry = (0, Instant::now());
        }
        entry.0 += 1;
    }
}

fn note_login_success(state: &State, ip: Option<IpAddr>) {
    let key = login_key(ip);
    if let Ok(mut map) = state.login_attempts.lock() {
        map.remove(&key);
    }
}

/// Register a session in the shared registry.
pub fn register(server: &RemoteServer, id: String, name: String, permit: Permit) {
    let session = Session { id: id.clone(), name, permit, mode: "build".into() };
    if let Ok(mut reg) = server.state.registry.lock() {
        if let Some(existing) = reg.get_mut(&id) {
            existing.session = session;
            return;
        }
        reg.insert(
            id,
            Registered {
                session,
                subs: Arc::new(Mutex::new(Vec::new())),
                pending_prompts: Arc::new(Mutex::new(VecDeque::new())),
                pending_approvals: Arc::new(Mutex::new(VecDeque::new())),
                pending_modes: Arc::new(Mutex::new(VecDeque::new())),
            },
        );
        server.state.refcount.fetch_add(1, Ordering::SeqCst);
    }
}

/// Remove a session from the shared registry.
pub fn deregister(server: &RemoteServer, id: &str) {
    let removed = server
        .state
        .registry
        .lock()
        .map(|mut reg| reg.remove(id).is_some())
        .unwrap_or(false);
    if removed {
        server.state.refcount.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Push a realtime event to every web subscriber of one session. No-op when the
/// session is not registered (remote disabled or closed).
pub fn emit(server: &RemoteServer, id: &str, event: RemoteEvent) {
    let subs = server
        .state
        .registry
        .lock()
        .ok()
        .and_then(|r| r.get(id).map(|reg| reg.subs.clone()));
    if let Some(subs) = subs {
        broadcast(&subs, event);
    }
}

/// Pop the next queued web prompt for one session, if any.
pub fn take_pending_prompt(server: &RemoteServer, id: &str) -> Option<String> {
    server
        .state
        .registry
        .lock()
        .ok()
        .and_then(|r| r.get(id).map(|reg| reg.pending_prompts.clone()))
        .and_then(|q| q.lock().ok().map(|mut q| q.pop_front()))
        .flatten()
}

/// Pop the next web accept/reject decision for one session, if any.
pub fn take_pending_approval(server: &RemoteServer, id: &str) -> Option<String> {
    server
        .state
        .registry
        .lock()
        .ok()
        .and_then(|r| r.get(id).map(|reg| reg.pending_approvals.clone()))
        .and_then(|q| q.lock().ok().map(|mut q| q.pop_front()))
        .flatten()
}

/// Record + broadcast the session's authoritative mode ("plan"|"build").
/// No-op when unchanged or unknown. Broadcasts `RemoteEvent::Mode` to all subscribers.
pub fn set_session_mode(server: &RemoteServer, id: &str, mode: &str) {
    if mode != "plan" && mode != "build" {
        return;
    }
    // Clone the subscriber list and drop the registry lock before broadcasting.
    let subs = {
        let Ok(mut reg) = server.state.registry.lock() else { return };
        let Some(entry) = reg.get_mut(id) else { return };
        if entry.session.mode == mode {
            return;
        }
        entry.session.mode = mode.to_string();
        entry.subs.clone()
    };
    broadcast(&subs, RemoteEvent::Mode { mode: mode.to_string() });
}

/// The session's authoritative mode, if it is registered.
pub fn session_mode(server: &RemoteServer, id: &str) -> Option<String> {
    server
        .state
        .registry
        .lock()
        .ok()
        .and_then(|r| r.get(id).map(|reg| reg.session.mode.clone()))
}

/// Pop the next mode requested from the web that the owning TUI has not applied yet.
pub fn take_pending_mode(server: &RemoteServer, id: &str) -> Option<String> {
    server
        .state
        .registry
        .lock()
        .ok()
        .and_then(|r| r.get(id).map(|reg| reg.pending_modes.clone()))
        .and_then(|q| q.lock().ok().map(|mut q| q.pop_front()))
        .flatten()
}

static REMOTE_TURN_SEQ: AtomicUsize = AtomicUsize::new(0);

/// Snapshot of the live runtime config (bind/port are fixed at start; password
/// and public_url reflect any runtime change).
pub fn config_snapshot(server: &RemoteServer) -> RemoteConfig {
    let mut cfg = (*server.state.config).clone();
    if let Ok(p) = server.state.password.lock() {
        cfg.password = p.clone();
    }
    if let Ok(u) = server.state.public_url.lock() {
        cfg.public_url = u.clone();
    }
    cfg
}

/// Change the login password on a running server. Takes effect immediately and
/// invalidates the current token (clients must log in again).
pub fn set_password(server: &RemoteServer, password: String) {
    if let Ok(mut p) = server.state.password.lock() {
        *p = password;
    }
    if let Ok(mut t) = server.state.token.lock() {
        *t = None;
    }
}

/// Record the public/tunnel URL on a running server (informational only).
pub fn set_public_url(server: &RemoteServer, url: String) {
    if let Ok(mut u) = server.state.public_url.lock() {
        *u = url;
    }
}

/// Location of the remote config file: `%APPDATA%\evrax\remote.json`.
pub fn remote_config_path() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|dir| PathBuf::from(dir).join("evrax").join("remote.json"))
}

/// Parse a config from JSON text (missing/invalid → defaults).
pub fn parse_remote_config(text: &str) -> RemoteConfig {
    serde_json::from_str(text).unwrap_or_default()
}

/// Load a config from a specific path (missing file → defaults).
pub fn load_remote_config_at(path: &std::path::Path) -> RemoteConfig {
    std::fs::read_to_string(path)
        .map(|s| parse_remote_config(&s))
        .unwrap_or_default()
}

/// Load the user's remote config (missing file → defaults).
pub fn load_remote_config() -> RemoteConfig {
    remote_config_path()
        .map(|p| load_remote_config_at(&p))
        .unwrap_or_default()
}

/// Persist the config to `%APPDATA%\evrax\remote.json`, creating the directory.
pub fn save_remote_config(config: &RemoteConfig) -> std::io::Result<()> {
    let path = remote_config_path().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "APPDATA is not set")
    })?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_string_pretty(config).unwrap_or_default();
    std::fs::write(path, json)
}

const WEBUI: &str = include_str!("web/index.html");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allow_remote_default_password_only_loopback() {
        let lo4: IpAddr = "127.0.0.1".parse().unwrap();
        let lo6: IpAddr = "::1".parse().unwrap();
        let wan4: IpAddr = "203.0.113.5".parse().unwrap();
        let wan6: IpAddr = "2001:db8::1".parse().unwrap();

        assert!(allow_remote(true, Some(lo4)));
        assert!(allow_remote(true, Some(lo6)));
        assert!(!allow_remote(true, Some(wan4)));
        assert!(!allow_remote(true, Some(wan6)));
        assert!(!allow_remote(true, None));
    }

    #[test]
    fn allow_remote_custom_password_allows_any() {
        let wan4: IpAddr = "203.0.113.5".parse().unwrap();
        assert!(allow_remote(false, Some(wan4)));
        assert!(allow_remote(false, None));
    }

    #[test]
    fn parse_register_body() {
        let body = r#"{"id":"abc","name":"Shell","permit":"write"}"#;
        let parsed: RegisterBody = serde_json::from_str(body).unwrap();
        assert_eq!(parsed.id, "abc");
        assert_eq!(parsed.name, "Shell");
        assert_eq!(parsed.permit, Permit::Write);
    }

    #[test]
    fn parse_register_body_name_defaults() {
        let body = r#"{"id":"abc","permit":"read"}"#;
        let parsed: RegisterBody = serde_json::from_str(body).unwrap();
        assert_eq!(parsed.name, "");
        assert_eq!(parsed.permit, Permit::Read);
    }

    #[test]
    fn register_deregister_refcount() {
        let server = start(RemoteConfig { port: 0, ..Default::default() });
        register(&server, "s1".into(), "one".into(), Permit::Read);
        register(&server, "s2".into(), "two".into(), Permit::Write);
        assert_eq!(server.state.refcount.load(Ordering::SeqCst), 2);
        deregister(&server, "s1");
        assert_eq!(server.state.refcount.load(Ordering::SeqCst), 1);
        deregister(&server, "missing");
        assert_eq!(server.state.refcount.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn session_serializes_lowercase_permit() {
        let s = Session { id: "x".into(), name: "n".into(), permit: Permit::Write, mode: "build".into() };
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"permit\":\"write\""));
    }

    #[test]
    fn default_config_values() {
        let c = RemoteConfig::default();
        assert_eq!(c.bind, "127.0.0.1");
        assert_eq!(c.port, 444);
        assert_eq!(c.password, "12345678");
        assert!(is_default_password(&c.password));
    }

    #[test]
    fn stays_alive_after_deregister() {
        let server = start(RemoteConfig { port: 0, ..Default::default() });
        assert!(server.is_running());
        register(&server, "s1".into(), "one".into(), Permit::Read);
        deregister(&server, "s1");
        // Idle with the handle still held: the server must NOT shut down.
        std::thread::sleep(Duration::from_millis(600));
        assert!(server.is_running(), "live handle must keep the server running");
    }

    #[test]
    fn stops_after_handle_drop_when_idle() {
        let server = start(RemoteConfig { port: 0, ..Default::default() });
        let alive = server.state.alive.clone();
        assert!(alive.load(Ordering::SeqCst));
        drop(server);
        let mut stopped = false;
        for _ in 0..40 {
            if !alive.load(Ordering::SeqCst) {
                stopped = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(stopped, "dropped idle handle must stop the server");
    }

    #[test]
    fn bind_failure_returns_dead_handle() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = start(RemoteConfig { bind: "127.0.0.1".into(), port, ..Default::default() });
        assert!(!server.is_running(), "bind conflict must yield a dead handle");
        drop(listener);
        drop(server);
    }

    #[test]
    fn sse_stream_sends_preamble_and_events_promptly() {
        use std::io::{Read, Write};
        use std::net::TcpStream;

        let free = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let server = start(RemoteConfig { bind: "127.0.0.1".into(), port: free, ..Default::default() });
        assert!(server.is_running());
        register(&server, "s1".into(), "one".into(), Permit::Write);
        // Authenticate up-front: the stream route now requires a token.
        if let Ok(mut t) = server.state.token.lock() {
            *t = Some(("tok".into(), Instant::now()));
        }

        let mut c = TcpStream::connect(("127.0.0.1", free)).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        c.write_all(b"GET /api/session/s1/stream?token=tok HTTP/1.1\r\nHost: x\r\nAccept: text/event-stream\r\n\r\n")
            .unwrap();

        // The preamble + connected frame must arrive without waiting for EOF.
        let mut buf = [0u8; 1024];
        let n = c.read(&mut buf).unwrap();
        let first = String::from_utf8_lossy(&buf[..n]);
        assert!(first.contains("200 OK"), "no HTTP status line: {first:?}");
        assert!(first.contains("text/event-stream"), "wrong content type: {first:?}");
        assert!(first.contains("event: connected"), "no connected frame: {first:?}");

        // A broadcast event must be flushed promptly, not held until close.
        let subs = server.state.registry.lock().unwrap().get("s1").unwrap().subs.clone();
        broadcast(&subs, RemoteEvent::Delta { turn: "t".into(), text: "hello-prompt".into() });

        let mut got = String::new();
        for _ in 0..5 {
            let mut b = [0u8; 1024];
            let n = c.read(&mut b).unwrap();
            if n == 0 {
                break;
            }
            got.push_str(&String::from_utf8_lossy(&b[..n]));
            if got.contains("hello-prompt") {
                break;
            }
        }
        assert!(got.contains("event: delta"), "no delta frame: {got:?}");
        assert!(got.contains("hello-prompt"), "delta payload not delivered: {got:?}");
    }

    #[test]
    fn pending_prompt_roundtrip() {
        let server = start(RemoteConfig { port: 0, ..Default::default() });
        register(&server, "s1".into(), "one".into(), Permit::Write);
        let q = server
            .state
            .registry
            .lock()
            .unwrap()
            .get("s1")
            .unwrap()
            .pending_prompts
            .clone();
        q.lock().unwrap().push_back("hello".into());
        assert_eq!(take_pending_prompt(&server, "s1").as_deref(), Some("hello"));
        assert_eq!(take_pending_prompt(&server, "s1"), None);
        assert_eq!(take_pending_prompt(&server, "missing"), None);
    }

    #[test]
    fn pending_approval_roundtrip() {
        let server = start(RemoteConfig { port: 0, ..Default::default() });
        register(&server, "s1".into(), "one".into(), Permit::Write);
        let q = server
            .state
            .registry
            .lock()
            .unwrap()
            .get("s1")
            .unwrap()
            .pending_approvals
            .clone();
        q.lock().unwrap().push_back("once".into());
        assert_eq!(take_pending_approval(&server, "s1").as_deref(), Some("once"));
        assert_eq!(take_pending_approval(&server, "s1"), None);
        assert_eq!(take_pending_approval(&server, "missing"), None);
    }

    #[test]
    fn approval_events_serialize_to_sse() {
        let req = RemoteEvent::Approval {
            turn: "t1".into(),
            name: "shell".into(),
            summary: "rm -rf build".into(),
        }
        .to_sse();
        assert!(req.contains("event: approval\n"), "frame: {req:?}");
        assert!(req.contains("\"name\":\"shell\""), "frame: {req:?}");
        assert!(req.contains("\"summary\":\"rm -rf build\""), "frame: {req:?}");

        let res = RemoteEvent::ApprovalResolved {
            turn: "t1".into(),
            decision: "reject".into(),
        }
        .to_sse();
        assert!(res.contains("event: approval_resolved\n"), "frame: {res:?}");
        assert!(res.contains("\"decision\":\"reject\""), "frame: {res:?}");
    }

    #[test]
    fn approval_endpoint_queues_decision() {
        use std::io::{Read, Write};
        use std::net::TcpStream;

        let free = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let server = start(RemoteConfig { bind: "127.0.0.1".into(), port: free, ..Default::default() });
        assert!(server.is_running());
        register(&server, "s1".into(), "one".into(), Permit::Write);
        if let Ok(mut t) = server.state.token.lock() {
            *t = Some(("tok".into(), Instant::now()));
        }

        let post = |path: &str, body: &str| -> String {
            let mut c = TcpStream::connect(("127.0.0.1", free)).unwrap();
            c.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            let req = format!(
                "POST {path} HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer tok\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            c.write_all(req.as_bytes()).unwrap();
            let mut buf = [0u8; 512];
            let n = c.read(&mut buf).unwrap();
            String::from_utf8_lossy(&buf[..n]).to_string()
        };

        let ok = post("/api/session/s1/approval", r#"{"decision":"always"}"#);
        assert!(ok.contains("200 OK"), "response: {ok:?}");
        assert_eq!(take_pending_approval(&server, "s1").as_deref(), Some("always"));

        let bad = post("/api/session/s1/approval", r#"{"decision":"maybe"}"#);
        assert!(bad.contains("400"), "response: {bad:?}");
        assert_eq!(take_pending_approval(&server, "s1"), None);

        let missing = post("/api/session/nope/approval", r#"{"decision":"once"}"#);
        assert!(missing.contains("404"), "response: {missing:?}");
    }

    #[test]
    fn parse_remote_config_roundtrip_and_defaults() {
        let cfg = RemoteConfig {
            bind: "0.0.0.0".into(),
            port: 5000,
            password: "supersecret".into(),
            public_url: "https://x.trycloudflare.com".into(),
            tunnel_mode: "quick".into(),
            tunnel_hostname: String::new(),
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let back = parse_remote_config(&json);
        assert_eq!(back.bind, "0.0.0.0");
        assert_eq!(back.port, 5000);
        assert_eq!(back.password, "supersecret");
        assert_eq!(back.public_url, "https://x.trycloudflare.com");
        assert_eq!(back.tunnel_mode, "quick");
        // Missing fields / invalid text fall back to defaults.
        assert_eq!(parse_remote_config("{}").bind, "127.0.0.1");
        assert_eq!(parse_remote_config("not json").port, 444);
        assert_eq!(parse_remote_config("").password, "12345678");
        assert_eq!(parse_remote_config("{}").tunnel_mode, "");
    }

    #[test]
    fn ct_eq_is_exact_and_length_safe() {
        assert!(ct_eq("abc", "abc"));
        assert!(!ct_eq("abc", "abd"));
        assert!(!ct_eq("abc", "abcd"));
        assert!(ct_eq("", ""));
    }

    #[test]
    fn login_rate_limit_locks_then_resets() {
        let server = start(RemoteConfig { port: 0, ..Default::default() });
        let ip: IpAddr = "203.0.113.9".parse().unwrap();
        for _ in 0..LOGIN_MAX {
            assert!(login_allowed(&server.state, Some(ip)));
            note_login_failure(&server.state, Some(ip));
        }
        assert!(!login_allowed(&server.state, Some(ip)), "locked after max failures");
        note_login_success(&server.state, Some(ip));
        assert!(login_allowed(&server.state, Some(ip)), "success resets the counter");
    }

    #[test]
    fn set_password_updates_snapshot_and_invalidates_token() {
        let server = start(RemoteConfig::default());
        if let Ok(mut t) = server.state.token.lock() {
            *t = Some(("tok".into(), Instant::now()));
        }
        set_password(&server, "newpass123".into());
        assert!(server.state.token.lock().unwrap().is_none(), "token invalidated");
        assert_eq!(config_snapshot(&server).password, "newpass123");
    }

    #[test]
    fn api_requires_token_but_health_is_open() {
        use std::io::{Read, Write};
        use std::net::TcpStream;

        let free = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let server = start(RemoteConfig { bind: "127.0.0.1".into(), port: free, ..Default::default() });

        let request = |raw: &[u8]| {
            let mut c = TcpStream::connect(("127.0.0.1", free)).unwrap();
            c.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            c.write_all(raw).unwrap();
            let mut buf = [0u8; 512];
            let n = c.read(&mut buf).unwrap();
            String::from_utf8_lossy(&buf[..n]).to_string()
        };

        let unauth = request(b"GET /api/sessions HTTP/1.1\r\nHost: x\r\n\r\n");
        assert!(unauth.contains("401"), "expected 401 without token: {unauth:?}");

        let health = request(b"GET /health HTTP/1.1\r\nHost: x\r\n\r\n");
        assert!(health.contains("200"), "health must stay open: {health:?}");

        if let Ok(mut t) = server.state.token.lock() {
            *t = Some(("tok".into(), Instant::now()));
        }
        let authed = request(b"GET /api/sessions HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer tok\r\n\r\n");
        assert!(authed.contains("200"), "expected 200 with token: {authed:?}");
    }

    #[test]
    fn mode_event_serializes_to_sse() {
        let frame = RemoteEvent::Mode { mode: "plan".into() }.to_sse();
        assert!(frame.contains("event: mode\n"), "frame: {frame:?}");
        assert!(frame.contains("\"type\":\"mode\""), "frame: {frame:?}");
        assert!(frame.contains("\"mode\":\"plan\""), "frame: {frame:?}");
    }

    #[test]
    fn register_body_mode_defaults_to_empty() {
        let without: RegisterBody = serde_json::from_str(r#"{"id":"a","permit":"write"}"#).unwrap();
        assert_eq!(without.mode, "");
        assert_eq!(normalize_mode(&without.mode), "build");

        let with: RegisterBody =
            serde_json::from_str(r#"{"id":"a","permit":"read","mode":"plan"}"#).unwrap();
        assert_eq!(with.mode, "plan");
        assert_eq!(normalize_mode(&with.mode), "plan");
        // Junk collapses to the safe default rather than being stored verbatim.
        assert_eq!(normalize_mode("turbo"), "build");
    }

    #[test]
    fn api_sessions_include_default_mode() {
        use std::io::{Read, Write};
        use std::net::TcpStream;

        let free = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let server = start(RemoteConfig { bind: "127.0.0.1".into(), port: free, ..Default::default() });
        assert!(server.is_running());
        register(&server, "s1".into(), "one".into(), Permit::Write);
        if let Ok(mut t) = server.state.token.lock() {
            *t = Some(("tok".into(), Instant::now()));
        }

        let mut c = TcpStream::connect(("127.0.0.1", free)).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        c.write_all(b"GET /api/sessions HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer tok\r\n\r\n")
            .unwrap();
        let mut buf = [0u8; 1024];
        let n = c.read(&mut buf).unwrap();
        let body = String::from_utf8_lossy(&buf[..n]);
        assert!(body.contains("200 OK"), "response: {body:?}");
        assert!(body.contains("\"mode\":\"build\""), "default mode missing: {body:?}");
    }

    #[test]
    fn mode_endpoint_queues_and_validates() {
        use std::io::{Read, Write};
        use std::net::TcpStream;

        let free = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let server = start(RemoteConfig { bind: "127.0.0.1".into(), port: free, ..Default::default() });
        assert!(server.is_running());
        register(&server, "s1".into(), "one".into(), Permit::Write);
        register(&server, "ro".into(), "ro".into(), Permit::Read);
        if let Ok(mut t) = server.state.token.lock() {
            *t = Some(("tok".into(), Instant::now()));
        }

        let post = |path: &str, body: &str| -> String {
            let mut c = TcpStream::connect(("127.0.0.1", free)).unwrap();
            c.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            let req = format!(
                "POST {path} HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer tok\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            c.write_all(req.as_bytes()).unwrap();
            let mut buf = [0u8; 512];
            let n = c.read(&mut buf).unwrap();
            String::from_utf8_lossy(&buf[..n]).to_string()
        };

        let ok = post("/api/session/s1/mode", r#"{"mode":"plan"}"#);
        assert!(ok.contains("202"), "response: {ok:?}");
        assert_eq!(take_pending_mode(&server, "s1").as_deref(), Some("plan"));

        let bad = post("/api/session/s1/mode", r#"{"mode":"turbo"}"#);
        assert!(bad.contains("400"), "response: {bad:?}");
        assert_eq!(take_pending_mode(&server, "s1"), None, "invalid mode must not enqueue");

        let missing = post("/api/session/nope/mode", r#"{"mode":"plan"}"#);
        assert!(missing.contains("404"), "response: {missing:?}");

        let readonly = post("/api/session/ro/mode", r#"{"mode":"plan"}"#);
        assert!(readonly.contains("403"), "response: {readonly:?}");
        assert_eq!(take_pending_mode(&server, "ro"), None);
    }

    #[test]
    fn set_session_mode_broadcasts_and_ignores_noops() {
        let server = start(RemoteConfig { port: 0, ..Default::default() });
        register(&server, "s1".into(), "one".into(), Permit::Write);
        let subs = server.state.registry.lock().unwrap().get("s1").unwrap().subs.clone();
        let (tx, rx) = channel::<RemoteEvent>();
        subs.lock().unwrap().push(tx);

        assert_eq!(session_mode(&server, "s1").as_deref(), Some("build"));
        set_session_mode(&server, "s1", "plan");
        assert_eq!(session_mode(&server, "s1").as_deref(), Some("plan"));
        match rx.try_recv() {
            Ok(RemoteEvent::Mode { mode }) => assert_eq!(mode, "plan"),
            other => panic!("expected mode broadcast, got {other:?}"),
        }

        // Same value again: no state change, no broadcast.
        set_session_mode(&server, "s1", "plan");
        assert!(rx.try_recv().is_err(), "no-op must not broadcast");

        // Unknown session and junk mode are silently ignored.
        set_session_mode(&server, "missing", "build");
        set_session_mode(&server, "s1", "turbo");
        assert_eq!(session_mode(&server, "s1").as_deref(), Some("plan"));
        assert_eq!(session_mode(&server, "missing"), None);
        assert!(rx.try_recv().is_err());
    }
}
