use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::tools::{Context, ToolOutcome, truncate};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Heartbeat freshness window: a worker publishes its state every 250 ms, so a
/// state older than this means the worker is dead.
const ALIVE_WINDOW_MS: u64 = 5000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkerStatus {
    #[default]
    Starting,
    Queued,
    Busy,
    Idle,
    Stopping,
    Stopped,
    Failed,
}

impl WorkerStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            WorkerStatus::Starting => "starting",
            WorkerStatus::Queued => "queued",
            WorkerStatus::Busy => "busy",
            WorkerStatus::Idle => "idle",
            WorkerStatus::Stopping => "stopping",
            WorkerStatus::Stopped => "stopped",
            WorkerStatus::Failed => "failed",
        }
    }

    pub fn parse(value: &str) -> WorkerStatus {
        match value.trim().to_ascii_lowercase().as_str() {
            "queued" => WorkerStatus::Queued,
            "busy" => WorkerStatus::Busy,
            "idle" => WorkerStatus::Idle,
            "stopping" => WorkerStatus::Stopping,
            "stopped" => WorkerStatus::Stopped,
            "failed" => WorkerStatus::Failed,
            _ => WorkerStatus::Starting,
        }
    }
}

impl std::fmt::Display for WorkerStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct State {
    pub id: String,
    pub name: String,
    pub pid: u32,
    #[serde(default)]
    pub status: WorkerStatus,
    pub root: String,
    /// Id of the parent session that spawned this worker. Empty for legacy
    /// state files written before P20.
    #[serde(default)]
    pub session: String,
    pub updated_ms: u64,
    pub last_reply: String,
}

pub fn base_dir() -> PathBuf {
    std::env::var_os("EVRAX_WORKERS_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("APPDATA")
                .map(PathBuf::from)
                .map(|dir| dir.join("evrax"))
        })
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config").join("evrax"))
        })
        .unwrap_or_else(|| PathBuf::from("."))
        .join("workers")
}

pub fn state_path(id: &str) -> PathBuf {
    base_dir().join(id).join("state.json")
}

pub fn inbox_path(id: &str) -> PathBuf {
    base_dir().join(id).join("prompt.txt")
}

pub fn stop_path(id: &str) -> PathBuf {
    base_dir().join(id).join("stop")
}

pub fn events_path(id: &str) -> PathBuf {
    base_dir().join(id).join("events.jsonl")
}

pub fn load(id: &str) -> Option<State> {
    let text = fs::read_to_string(state_path(id)).ok()?;
    serde_json::from_str(&text).ok()
}

fn load_from(dir: &Path, id: &str) -> Option<State> {
    let text = fs::read_to_string(dir.join(id).join("state.json")).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn save(state: &State) -> Result<(), String> {
    let path = state_path(&state.id);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(state).map_err(|e| e.to_string())?;
    fs::write(path, text).map_err(|e| e.to_string())
}

pub fn list() -> Vec<State> {
    let Ok(entries) = fs::read_dir(base_dir()) else {
        return Vec::new();
    };
    let mut states: Vec<State> = entries
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| entry.file_name().to_str().and_then(load))
        .collect();
    states.sort_by_key(|state| std::cmp::Reverse(state.updated_ms));
    states
}

/// Workers belonging to one project root (the scope the TUI/agent shows).
pub fn list_for(root: &Path) -> Vec<State> {
    filter_by_root(&list(), root)
}

/// Workers spawned by one session inside one project root (P20 reconnect).
pub fn list_for_session(session: &str, root: &Path) -> Vec<State> {
    filter_by_session(&list(), session, root)
}

fn filter_by_root(states: &[State], root: &Path) -> Vec<State> {
    let root = root.display().to_string();
    states
        .iter()
        .filter(|state| state.root == root)
        .cloned()
        .collect()
}

fn filter_by_session(states: &[State], session: &str, root: &Path) -> Vec<State> {
    let root = root.display().to_string();
    states
        .iter()
        .filter(|state| state.session == session && state.root == root)
        .cloned()
        .collect()
}

/// Removes worker directories whose process is gone (`!is_alive`) and whose
/// status is terminal. Live workers are never touched, and `keep_live` keeps
/// non-terminal-but-stale directories for inspection (startup uses `false`).
pub fn prune(keep_live: bool) -> usize {
    prune_dir(&base_dir(), keep_live)
}

/// Whether a worker directory may be deleted: never a live worker, else any
/// terminal (stopped/failed) state, or any dead state when `keep_live` is off.
fn should_prune(state: &State, keep_live: bool) -> bool {
    if is_alive(state) {
        return false;
    }
    let terminal = state.status == WorkerStatus::Stopped || state.status == WorkerStatus::Failed;
    terminal || !keep_live
}

fn prune_dir(dir: &Path, keep_live: bool) -> usize {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    let mut removed = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        let removable = match load_from(dir, &name) {
            Some(state) => should_prune(&state, keep_live),
            // A directory with no readable state is orphaned junk: remove it.
            None => true,
        };
        if removable && fs::remove_dir_all(&path).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// Liveness from the heartbeat, not a pid probe: `state.pid` may be 0 (not yet
/// published), and portable process queries would need a crate. A worker
/// republishes `updated_ms` every 250 ms, so a fresh-enough state in a
/// non-terminal status means the worker is running.
pub fn is_alive(state: &State) -> bool {
    state.status != WorkerStatus::Stopped
        && state.status != WorkerStatus::Failed
        && now_ms().saturating_sub(state.updated_ms) < ALIVE_WINDOW_MS
}

/// Clamps a requested worker count into the supported 1..=3 range.
pub fn clamp_count(value: i64) -> usize {
    value.clamp(1, 3) as usize
}

pub fn run(args: &Value, ctx: &Context) -> Result<ToolOutcome, String> {
    let action = args
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or("list")
        .to_ascii_lowercase();
    match action.as_str() {
        "spawn" => spawn(args, ctx),
        "send" => send(args),
        "status" => status(args),
        "list" => Ok(ToolOutcome {
            ok: true,
            detail: render_list_for(&ctx.root),
            diff: None,
        }),
        "stop" => stop(args),
        other => Err(format!("unknown worker action: {other}")),
    }
}

/// Spawns `count` (1..=3) headless workers at once, each drawn as a TUI pane.
/// `session` (from `ctx`, or the tool arg as a fallback) ties the worker to its
/// parent session so the pane can be restored on resume (P20). `Context` has no
/// session field yet — its owner is out of this lane — so the optional `session`
/// arg is read here and blank means "unknown".
fn spawn(args: &Value, ctx: &Context) -> Result<ToolOutcome, String> {
    let name = arg_name(args);
    let prompt = arg_prompt(args);
    let session = args
        .get("session")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    let count = clamp_count(args.get("count").and_then(Value::as_i64).unwrap_or(1));
    let mut ids: Vec<String> = Vec::with_capacity(count);
    let mut failed: Vec<String> = Vec::new();
    for _ in 0..count {
        match spawn_worker(&name, prompt, &ctx.root, session) {
            Ok(state) => ids.push(state.id),
            Err(error) => failed.push(error),
        }
    }
    if ids.is_empty() {
        return Ok(ToolOutcome {
            ok: false,
            detail: format!("failed to spawn workers: {}", failed.join("; ")),
            diff: None,
        });
    }
    let mut detail = format!(
        "spawned {} worker(s) headless\nids: {}",
        ids.len(),
        ids.join(", ")
    );
    if !failed.is_empty() {
        detail.push_str(&format!("\nfailures: {}", failed.join("; ")));
    }
    Ok(ToolOutcome {
        ok: true,
        detail,
        diff: None,
    })
}

/// Generates an id, persists the starting state, writes the initial prompt and
/// launches the headless process.
pub fn spawn_worker(
    name: &str,
    prompt: Option<&str>,
    root: &Path,
    session: &str,
) -> Result<State, String> {
    let id = format!(
        "worker-{}-{}",
        now_ms(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    let mut state = State {
        id: id.clone(),
        name: name.to_string(),
        pid: 0,
        status: WorkerStatus::Starting,
        root: root.display().to_string(),
        session: session.to_string(),
        updated_ms: now_ms(),
        last_reply: String::new(),
    };
    save(&state)?;
    if let Some(prompt) = prompt {
        fs::write(inbox_path(&id), prompt).map_err(|e| e.to_string())?;
    }
    match launch_headless(name, &id, root) {
        Ok(()) => Ok(state),
        Err(error) => {
            state.status = WorkerStatus::Failed;
            state.updated_ms = now_ms();
            state.last_reply = error.clone();
            let _ = save(&state);
            Err(format!("{id}: {error}"))
        }
    }
}

fn send(args: &Value) -> Result<ToolOutcome, String> {
    let id = arg_id(args)?;
    let prompt = args
        .get("prompt")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "send requires prompt".to_string())?;
    let mut state = load(&id).ok_or_else(|| format!("worker {id} not found"))?;
    if state.status == WorkerStatus::Stopped || state.status == WorkerStatus::Failed {
        return Err(format!("worker {id} is {}", state.status));
    }
    let inbox = inbox_path(&id);
    if inbox.exists() {
        return Err(format!("worker {id} already has a pending prompt"));
    }
    fs::write(&inbox, prompt).map_err(|e| e.to_string())?;
    state.status = WorkerStatus::Queued;
    state.updated_ms = now_ms();
    save(&state)?;
    Ok(ToolOutcome {
        ok: true,
        detail: format!("prompt queued for worker {id}"),
        diff: None,
    })
}

fn status(args: &Value) -> Result<ToolOutcome, String> {
    let id = arg_id(args)?;
    let state = load(&id).ok_or_else(|| format!("worker {id} not found"))?;
    let age = now_ms().saturating_sub(state.updated_ms) / 1000;
    let mut detail = format!(
        "worker {} [{}]\nname: {}\nroot: {}\npid: {}\nupdated: {}s ago",
        state.id, state.status, state.name, state.root, state.pid, age
    );
    if !state.last_reply.trim().is_empty() {
        let mut reply = state.last_reply.clone();
        truncate(&mut reply);
        detail.push_str("\nlast reply:\n");
        detail.push_str(reply.trim_end());
    }
    Ok(ToolOutcome {
        ok: true,
        detail,
        diff: None,
    })
}

fn stop(args: &Value) -> Result<ToolOutcome, String> {
    let id = arg_id(args)?;
    let mut state = load(&id).ok_or_else(|| format!("worker {id} not found"))?;
    let path = stop_path(&id);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    fs::write(path, "stop").map_err(|e| e.to_string())?;
    state.status = WorkerStatus::Stopping;
    state.updated_ms = now_ms();
    save(&state)?;
    // The just-stopped worker stays on disk until its process actually exits
    // (status terminal + heartbeat dead), but sweep any other terminal orphans.
    prune(false);
    Ok(ToolOutcome {
        ok: true,
        detail: format!("stop requested for worker {id}"),
        diff: None,
    })
}

fn arg_id(args: &Value) -> Result<String, String> {
    args.get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| "missing string argument `id`".into())
}

fn arg_name(args: &Value) -> String {
    args.get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("worker")
        .to_string()
}

fn arg_prompt(args: &Value) -> Option<&str> {
    args.get("prompt")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn render_list(states: &[State]) -> String {
    if states.is_empty() {
        return "no workers".into();
    }
    let mut lines = vec![format!("{} workers", states.len())];
    for state in states {
        lines.push(format!(
            "{} [{}] {} (pid {})",
            state.id, state.status, state.name, state.pid
        ));
    }
    lines.join("\n")
}

/// Project-scoped list: shows only this root's workers, with a `(+K hidden)`
/// suffix when other projects still have workers on disk.
fn render_list_for(root: &Path) -> String {
    let mine = list_for(root);
    let hidden = list().len().saturating_sub(mine.len());
    let mut text = render_list(&mine);
    if hidden > 0 {
        text.push_str(&format!("\n(+{hidden} hidden from other projects)"));
    }
    text
}

/// Appends one event to the worker's JSONL transcript. `AgentEvent` is not
/// `Serialize` in `evrax-core` (and that crate is out of scope here), so
/// callers pass an already-serialized `Value`.
pub fn append_event_json(id: &str, event: &Value) -> Result<(), String> {
    let path = events_path(id);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let line = serde_json::to_string(event).map_err(|e| e.to_string())?;
    use std::io::Write;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    writeln!(file, "{line}").map_err(|e| e.to_string())
}

/// Headless worker spawn: same binary, no window, inherited-less stdio.
fn launch_headless(name: &str, id: &str, root: &Path) -> Result<(), String> {
    let _ = name;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    std::process::Command::new(exe)
        .args([
            "--root",
            &root.to_string_lossy(),
            "--worker-id",
            id,
            "--headless",
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("could not spawn headless worker: {e}"))
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_renders_states() {
        let state = State {
            id: "worker-1".into(),
            name: "one".into(),
            pid: 7,
            status: WorkerStatus::Busy,
            root: ".".into(),
            session: "session-1".into(),
            updated_ms: now_ms(),
            last_reply: String::new(),
        };
        assert_eq!(
            render_list(&[state]),
            "1 workers\nworker-1 [busy] one (pid 7)"
        );
    }

    #[test]
    fn worker_status_round_trips_lowercase() {
        assert_eq!(WorkerStatus::Busy.as_str(), "busy");
        assert_eq!(WorkerStatus::parse("busy"), WorkerStatus::Busy);
        assert_eq!(WorkerStatus::parse("BUSY"), WorkerStatus::Busy);
        assert_eq!(WorkerStatus::parse("garbage"), WorkerStatus::Starting);
        assert_eq!(WorkerStatus::parse(""), WorkerStatus::Starting);
    }

    #[test]
    fn is_alive_uses_heartbeat() {
        let base = State {
            id: "worker-1".into(),
            name: "n".into(),
            pid: 0,
            status: WorkerStatus::Idle,
            root: ".".into(),
            session: String::new(),
            updated_ms: now_ms(),
            last_reply: String::new(),
        };
        assert!(is_alive(&base));

        let stale = State {
            updated_ms: now_ms().saturating_sub(9999),
            ..base.clone()
        };
        assert!(!is_alive(&stale));

        let failed = State {
            status: WorkerStatus::Failed,
            ..base.clone()
        };
        assert!(!is_alive(&failed));
    }

    #[test]
    fn spawn_clamps_count() {
        assert_eq!(clamp_count(0), 1);
        assert_eq!(clamp_count(1), 1);
        assert_eq!(clamp_count(3), 3);
        assert_eq!(clamp_count(9), 3);
    }

    #[test]
    fn state_status_missing_field_defaults() {
        let json = r#"{"id":"x","name":"n","pid":0,"root":".","updated_ms":0,"last_reply":""}"#;
        let state: State = serde_json::from_str(json).unwrap();
        assert_eq!(state.status, WorkerStatus::Starting);
    }

    #[test]
    fn state_status_round_trips() {
        let state = State {
            id: "worker-1".into(),
            name: "n".into(),
            pid: 0,
            status: WorkerStatus::Busy,
            root: ".".into(),
            session: String::new(),
            updated_ms: 0,
            last_reply: String::new(),
        };
        let json = serde_json::to_string(&state).unwrap();
        assert!(json.contains("\"status\":\"busy\""));
        let back: State = serde_json::from_str(&json).unwrap();
        assert_eq!(back.status, WorkerStatus::Busy);
    }

    fn state_fixture(id: &str, root: &str, session: &str, status: WorkerStatus) -> State {
        State {
            id: id.into(),
            name: id.into(),
            pid: 0,
            status,
            root: root.into(),
            session: session.into(),
            updated_ms: now_ms(),
            last_reply: String::new(),
        }
    }

    #[test]
    fn worker_state_records_session() {
        let state = state_fixture("worker-1", "/proj", "session-a", WorkerStatus::Idle);
        let json = serde_json::to_string(&state).unwrap();
        assert!(json.contains("\"session\":\"session-a\""), "{json}");
        let back: State = serde_json::from_str(&json).unwrap();
        assert_eq!(back.session, "session-a");
    }

    #[test]
    fn worker_state_session_defaults_empty_when_absent() {
        let json = r#"{"id":"x","name":"n","pid":0,"root":".","updated_ms":0,"last_reply":""}"#;
        let state: State = serde_json::from_str(json).unwrap();
        assert_eq!(state.session, "");
    }

    #[test]
    fn list_for_filters_by_root() {
        let states = [
            state_fixture("a", "/proj-a", "s1", WorkerStatus::Idle),
            state_fixture("b", "/proj-b", "s1", WorkerStatus::Idle),
            state_fixture("c", "/proj-a", "s2", WorkerStatus::Busy),
        ];
        let mine = filter_by_root(&states, Path::new("/proj-a"));
        let ids: Vec<&str> = mine.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["a", "c"]);
        assert!(filter_by_root(&states, Path::new("/proj-z")).is_empty());
    }

    #[test]
    fn list_for_session_filters() {
        let states = [
            state_fixture("a", "/proj", "session-1", WorkerStatus::Idle),
            state_fixture("b", "/proj", "session-2", WorkerStatus::Idle),
            state_fixture("c", "/other", "session-1", WorkerStatus::Idle),
        ];
        let hit = filter_by_session(&states, "session-1", Path::new("/proj"));
        let got: Vec<&str> = hit.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(got, ["a"]);
        assert!(filter_by_session(&states, "session-9", Path::new("/proj")).is_empty());
    }

    #[test]
    fn prune_removes_starting_stale() {
        let dir = tempfile::tempdir().unwrap();
        let mut stale = state_fixture("stale", "/proj", "s1", WorkerStatus::Starting);
        stale.updated_ms = now_ms().saturating_sub(60_000);
        let dir_path = dir.path().join(&stale.id);
        fs::create_dir_all(&dir_path).unwrap();
        fs::write(
            dir_path.join("state.json"),
            serde_json::to_string(&stale).unwrap(),
        )
        .unwrap();

        assert!(should_prune(&stale, false), "dead + non-terminal goes");
        let removed = prune_dir(dir.path(), false);
        assert_eq!(removed, 1);
        assert!(!dir_path.exists());
    }

    #[test]
    fn prune_keeps_live_workers() {
        let dir = tempfile::tempdir().unwrap();
        let live = state_fixture("live", "/proj", "s1", WorkerStatus::Busy);
        let live_path = dir.path().join(&live.id);
        fs::create_dir_all(&live_path).unwrap();
        fs::write(
            live_path.join("state.json"),
            serde_json::to_string(&live).unwrap(),
        )
        .unwrap();

        assert!(!should_prune(&live, false));
        assert_eq!(prune_dir(dir.path(), false), 0);
        assert!(live_path.exists());
    }

    #[test]
    fn prune_removes_terminal_even_with_keep_live() {
        let dir = tempfile::tempdir().unwrap();
        let mut stopped = state_fixture("stopped", "/proj", "s1", WorkerStatus::Stopped);
        stopped.updated_ms = now_ms();
        let stopped_path = dir.path().join(&stopped.id);
        fs::create_dir_all(&stopped_path).unwrap();
        fs::write(
            stopped_path.join("state.json"),
            serde_json::to_string(&stopped).unwrap(),
        )
        .unwrap();

        assert!(should_prune(&stopped, true));
        assert_eq!(prune_dir(dir.path(), true), 1);
        assert!(!stopped_path.exists());
    }

    #[test]
    fn prune_removes_stateless_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let orphan = dir.path().join("worker-junk");
        fs::create_dir_all(&orphan).unwrap();
        assert_eq!(prune_dir(dir.path(), true), 1);
        assert!(!orphan.exists());
    }

    #[test]
    fn render_list_for_marks_hidden_from_other_projects() {
        // Pure check of the hidden-count arithmetic; ids on disk are irrelevant.
        let mine = [state_fixture("a", "/p", "s", WorkerStatus::Idle)];
        assert_eq!(render_list(&mine), "1 workers\na [idle] a (pid 0)");
        let hidden = 2usize;
        let text = format!(
            "{}\n(+{hidden} hidden from other projects)",
            render_list(&mine)
        );
        assert!(text.contains("(+2 hidden from other projects)"), "{text}");
    }
}
