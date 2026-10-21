use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::tools::{Context, ToolOutcome, truncate};

static COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct State {
    pub id: String,
    pub name: String,
    pub pid: u32,
    pub status: String,
    pub root: String,
    pub updated_ms: u64,
    pub last_reply: String,
}

pub fn base_dir() -> PathBuf {
    std::env::var_os("EVRAX_SUBAGENTS_DIR")
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
        .join("subagents")
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

pub fn load(id: &str) -> Option<State> {
    let text = fs::read_to_string(state_path(id)).ok()?;
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

pub fn run(args: &Value, ctx: &Context) -> Result<ToolOutcome, String> {
    let action = args
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or("list")
        .to_ascii_lowercase();
    match action.as_str() {
        "start" => start(args, ctx),
        "send" => send(args),
        "status" => status(args),
        "list" => Ok(ToolOutcome {
            ok: true,
            detail: render_list(&list()),
            diff: None,
        }),
        "stop" => stop(args),
        other => Err(format!("unknown subagent action: {other}")),
    }
}

fn start(args: &Value, ctx: &Context) -> Result<ToolOutcome, String> {
    let name = args
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("subagent")
        .to_string();
    let prompt = args
        .get("prompt")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let id = format!(
        "sub-{}-{}",
        now_ms(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    let mut state = State {
        id: id.clone(),
        name: name.clone(),
        pid: 0,
        status: "starting".into(),
        root: ctx.root.display().to_string(),
        updated_ms: now_ms(),
        last_reply: String::new(),
    };
    save(&state)?;
    if let Some(prompt) = prompt {
        fs::write(inbox_path(&id), prompt).map_err(|e| e.to_string())?;
    }
    match launch(&name, &id, &ctx.root) {
        Ok(()) => Ok(ToolOutcome {
            ok: true,
            detail: format!(
                "subagent {id} launched in a new terminal\nname: {name}\nroot: {}\nUse action=status/send/stop with id={id}.",
                ctx.root.display()
            ),
            diff: None,
        }),
        Err(error) => {
            state.status = "failed".into();
            state.updated_ms = now_ms();
            state.last_reply = error.clone();
            let _ = save(&state);
            Ok(ToolOutcome {
                ok: false,
                detail: format!("failed to launch subagent {id}: {error}"),
                diff: None,
            })
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
    let mut state = load(&id).ok_or_else(|| format!("subagent {id} not found"))?;
    if state.status == "stopped" || state.status == "failed" {
        return Err(format!("subagent {id} is {}", state.status));
    }
    let inbox = inbox_path(&id);
    if inbox.exists() {
        return Err(format!("subagent {id} already has a pending prompt"));
    }
    fs::write(&inbox, prompt).map_err(|e| e.to_string())?;
    state.status = "queued".into();
    state.updated_ms = now_ms();
    save(&state)?;
    Ok(ToolOutcome {
        ok: true,
        detail: format!("prompt queued for subagent {id}"),
        diff: None,
    })
}

fn status(args: &Value) -> Result<ToolOutcome, String> {
    let id = arg_id(args)?;
    let state = load(&id).ok_or_else(|| format!("subagent {id} not found"))?;
    let age = now_ms().saturating_sub(state.updated_ms) / 1000;
    let mut detail = format!(
        "subagent {} [{}]\nname: {}\nroot: {}\npid: {}\nupdated: {}s ago",
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
    let mut state = load(&id).ok_or_else(|| format!("subagent {id} not found"))?;
    let path = stop_path(&id);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    fs::write(path, "stop").map_err(|e| e.to_string())?;
    state.status = "stopping".into();
    state.updated_ms = now_ms();
    save(&state)?;
    Ok(ToolOutcome {
        ok: true,
        detail: format!("stop requested for subagent {id}"),
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

fn render_list(states: &[State]) -> String {
    if states.is_empty() {
        return "no subagents".into();
    }
    let mut lines = vec![format!("{} subagents", states.len())];
    for state in states {
        lines.push(format!(
            "{} [{}] {} (pid {})",
            state.id, state.status, state.name, state.pid
        ));
    }
    lines.join("\n")
}

fn launch(name: &str, id: &str, root: &Path) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let title = format!("evrax · {name}");
    let mut terminal = std::process::Command::new("wt.exe");
    terminal.args([
        "-w",
        "0",
        "new-tab",
        "--title",
        &title,
        &exe.to_string_lossy(),
        "--root",
        &root.to_string_lossy(),
        "--subagent-id",
        id,
    ]);
    if terminal.spawn().is_ok() {
        return Ok(());
    }

    let script = format!(
        "Start-Process -FilePath {} -ArgumentList @('--root', {}, '--subagent-id', {})",
        quote_powershell(&exe.to_string_lossy()),
        quote_powershell(&root.to_string_lossy()),
        quote_powershell(id)
    );
    std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-Command", &script])
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("could not open a terminal: {e}"))
}

fn quote_powershell(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
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
    fn powershell_quotes_are_escaped() {
        assert_eq!(quote_powershell("a'b"), "'a''b'");
    }

    #[test]
    fn list_renders_states() {
        let state = State {
            id: "sub-1".into(),
            name: "one".into(),
            pid: 7,
            status: "busy".into(),
            root: ".".into(),
            updated_ms: now_ms(),
            last_reply: String::new(),
        };
        assert_eq!(
            render_list(&[state]),
            "1 subagents\nsub-1 [busy] one (pid 7)"
        );
    }
}
