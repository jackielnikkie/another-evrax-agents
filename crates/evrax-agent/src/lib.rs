//! Real evrax backend: OpenAI-compatible SSE chat (9router) + agent loop.

pub mod diff;
pub mod sse;
pub mod tools;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use evrax_core::{AgentBackend, AgentEvent, DiffLine, TurnRequest};
use futures_util::StreamExt;
use futures_util::stream::{self, BoxStream};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::sse::{Frame, SseDecoder, ToolCall, ToolCallAccumulator, parse_frame};

pub const DEFAULT_BASE_URL: &str = "http://127.0.0.1:20128/v1";
pub const DEFAULT_MODEL: &str = "bebas";
const MAX_HISTORY_CHARS: usize = 100_000;

#[derive(Clone, Debug, PartialEq)]
pub struct AgentConfig {
    pub base_url: String,
    pub model: String,
    pub api_key: Option<String>,
    pub max_iterations: usize,
    pub root: PathBuf,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            base_url: DEFAULT_BASE_URL.to_string(),
            model: DEFAULT_MODEL.to_string(),
            api_key: None,
            max_iterations: 12,
            root: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct ConfigFile {
    base_url: Option<String>,
    model: Option<String>,
    api_key: Option<String>,
    max_iterations: Option<usize>,
    mock: Option<bool>,
}

/// Defaults < `%APPDATA%\evrax\agent.json` < env < CLI args.
#[derive(Debug)]
pub struct LoadedConfig {
    pub mock: bool,
    pub config: AgentConfig,
}

pub fn load_config() -> LoadedConfig {
    let mut config = AgentConfig::default();
    let mut mock = false;
    if let Some(path) = config_file_path()
        && let Ok(text) = std::fs::read_to_string(path)
        && let Ok(file) = serde_json::from_str::<ConfigFile>(&text)
    {
        mock = file.mock.unwrap_or(false);
        apply(&mut config, file);
    }
    if let Ok(v) = std::env::var("EVRAX_BASE_URL") {
        config.base_url = v;
    }
    if let Ok(v) = std::env::var("EVRAX_MODEL") {
        config.model = v;
    }
    if let Ok(v) = std::env::var("EVRAX_API_KEY") {
        config.api_key = Some(v);
    }
    if let Ok(v) = std::env::var("EVRAX_MAX_ITERATIONS")
        && let Ok(v) = v.parse()
    {
        config.max_iterations = v;
    }
    if std::env::var("EVRAX_MOCK").is_ok_and(|v| v == "1" || v == "true") {
        mock = true;
    }
    LoadedConfig { mock, config }
}

fn apply(config: &mut AgentConfig, file: ConfigFile) {
    if let Some(v) = file.base_url {
        config.base_url = v;
    }
    if let Some(v) = file.model {
        config.model = v;
    }
    if let Some(v) = file.api_key {
        config.api_key = Some(v);
    }
    if let Some(v) = file.max_iterations {
        config.max_iterations = v;
    }
}

/// Parse `--mock`, `--model <v>`, `--base-url <v>`, `--api-key <v>`,
/// `--max-iterations <v>`, `--root <v>` on top of env/file config.
pub fn config_from_args() -> LoadedConfig {
    let mut loaded = load_config();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--mock" => loaded.mock = true,
            "--model" => {
                if let Some(v) = args.next() {
                    loaded.config.model = v;
                }
            }
            "--base-url" | "--base_url" => {
                if let Some(v) = args.next() {
                    loaded.config.base_url = v;
                }
            }
            "--api-key" | "--api_key" => {
                if let Some(v) = args.next() {
                    loaded.config.api_key = Some(v);
                }
            }
            "--max-iterations" => {
                if let Some(v) = args.next()
                    && let Ok(v) = v.parse()
                {
                    loaded.config.max_iterations = v;
                }
            }
            "--root" => {
                if let Some(v) = args.next() {
                    loaded.config.root = PathBuf::from(v);
                }
            }
            _ => {}
        }
    }
    loaded
}

fn config_file_path() -> Option<PathBuf> {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .map(|dir| dir.join("evrax").join("agent.json"))
}

struct Inner {
    cfg: AgentConfig,
    client: reqwest::Client,
    conversations: Mutex<HashMap<String, Vec<Vec<Value>>>>,
}

impl Inner {
    fn replay(&self, session: &str) -> Vec<Value> {
        let conversations = self.conversations.lock().unwrap();
        conversations
            .get(session)
            .into_iter()
            .flatten()
            .flatten()
            .cloned()
            .collect()
    }

    fn commit(&self, session: &str, turn: Vec<Value>) {
        let mut conversations = self.conversations.lock().unwrap();
        let turns = conversations.entry(session.to_string()).or_default();
        turns.push(turn);
        let size = |messages: &[Value]| -> usize {
            messages.iter().map(message_chars).sum()
        };
        let mut total: usize = turns.iter().map(|t| size(t)).sum();
        while total > MAX_HISTORY_CHARS && turns.len() > 1 {
            total = total.saturating_sub(size(&turns[0]));
            turns.remove(0);
        }
    }
}

fn message_chars(message: &Value) -> usize {
    let content = message.get("content").and_then(Value::as_str).unwrap_or("");
    let calls = message
        .get("tool_calls")
        .and_then(Value::as_array)
        .map(|calls| {
            calls
                .iter()
                .map(|c| {
                    c.pointer("/function/name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .len()
                        + c.pointer("/function/arguments")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .len()
                })
                .sum::<usize>()
        })
        .unwrap_or(0);
    content.len() + calls
}

pub struct ChatBackend {
    inner: Arc<Inner>,
}

impl ChatBackend {
    pub fn new(cfg: AgentConfig) -> anyhow::Result<Self> {
        let mut builder = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .read_timeout(Duration::from_secs(180));
        let lower = cfg.base_url.to_lowercase();
        if lower.contains("127.0.0.1") || lower.contains("localhost") || lower.contains("[::1]") {
            builder = builder.no_proxy();
        }
        Ok(Self {
            inner: Arc::new(Inner {
                cfg,
                client: builder.build()?,
                conversations: Mutex::new(HashMap::new()),
            }),
        })
    }

    pub fn from_config() -> anyhow::Result<Self> {
        Self::new(load_config().config)
    }

    pub fn model(&self) -> &str {
        &self.inner.cfg.model
    }

    pub fn base_url(&self) -> &str {
        &self.inner.cfg.base_url
    }

    pub fn root(&self) -> &std::path::Path {
        &self.inner.cfg.root
    }
}

impl AgentBackend for ChatBackend {
    fn run_turn(
        &self,
        request: TurnRequest,
        decisions: mpsc::Receiver<bool>,
    ) -> BoxStream<'static, AgentEvent> {
        let inner = self.inner.clone();
        let (tx, rx) = mpsc::channel::<AgentEvent>(64);
        tokio::spawn(async move {
            if let Err(message) = run_turn(inner, request, decisions, tx.clone()).await {
                let _ = tx.send(AgentEvent::Failed { message }).await;
            }
        });
        stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|event| (event, rx))
        })
        .boxed()
    }
}

struct Exec {
    ok: bool,
    detail: String,
    diff: Option<(String, Vec<DiffLine>)>,
}

impl Exec {
    fn error(detail: String) -> Self {
        Self { ok: false, detail, diff: None }
    }

    fn result_text(&self) -> String {
        if self.ok {
            self.detail.clone()
        } else {
            format!("error: {}", self.detail)
        }
    }
}

async fn emit(tx: &mpsc::Sender<AgentEvent>, event: AgentEvent) -> Result<(), String> {
    tx.send(event).await.map_err(|_| "turn dibatalkan".to_string())
}

async fn run_turn(
    inner: Arc<Inner>,
    request: TurnRequest,
    mut decisions: mpsc::Receiver<bool>,
    tx: mpsc::Sender<AgentEvent>,
) -> Result<(), String> {
    let system = json!({ "role": "system", "content": system_prompt(&inner.cfg, request.plan_mode) });
    let history = inner.replay(&request.session);
    let specs = tool_specs(request.plan_mode);
    let ctx = tools::Context::new(inner.cfg.root.clone());
    let mut committed: Vec<Value> = vec![json!({ "role": "user", "content": request.prompt })];
    let mut rounds = 0usize;

    loop {
        rounds += 1;
        if rounds > inner.cfg.max_iterations {
            return Err(format!(
                "berhenti: melebihi batas {} iterasi tool",
                inner.cfg.max_iterations
            ));
        }
        let mut messages = Vec::with_capacity(history.len() + committed.len() + 1);
        messages.push(system.clone());
        messages.extend(history.iter().cloned());
        messages.extend(committed.iter().cloned());

        let round = stream_round(&inner, &messages, &specs, &tx).await?;
        if round.calls.is_empty() {
            committed.push(json!({ "role": "assistant", "content": round.text }));
            inner.commit(&request.session, committed);
            return emit(&tx, AgentEvent::TurnFinished).await;
        }

        let calls = round.calls;
        committed.push(json!({
            "role": "assistant",
            "content": round.text,
            "tool_calls": calls.iter().map(|call| json!({
                "id": call.id,
                "type": "function",
                "function": { "name": call.name, "arguments": call.arguments }
            })).collect::<Vec<_>>()
        }));

        for call in &calls {
            let args = parse_args(&call.arguments);
            let summary = args
                .as_ref()
                .map(|args| tools::summary(&call.name, args))
                .unwrap_or_else(|_| call.arguments.chars().take(80).collect());
            let summary = trunc(&summary, 120);
            emit(&tx, AgentEvent::ToolStarted { name: call.name.clone(), summary: summary.clone() })
                .await?;
            let outcome =
                authorize_and_run(&call.name, &args, &summary, &ctx, request.plan_mode, &mut decisions, &tx)
                    .await?;
            if let Some((path, lines)) = &outcome.diff {
                emit(&tx, AgentEvent::Diff { path: path.clone(), lines: lines.clone() }).await?;
            }
            emit(&tx, AgentEvent::ToolFinished {
                name: call.name.clone(),
                ok: outcome.ok,
                detail: trunc(&outcome.detail, 2000),
            })
            .await?;
            committed.push(json!({
                "role": "tool",
                "tool_call_id": call.id,
                "content": outcome.result_text()
            }));
        }
    }
}

fn parse_args(raw: &str) -> Result<Value, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(trimmed).map_err(|e| format!("invalid tool arguments: {e}"))
}

#[allow(clippy::too_many_arguments)]
async fn authorize_and_run(
    name: &str,
    args: &Result<Value, String>,
    summary: &str,
    ctx: &tools::Context,
    plan_mode: bool,
    decisions: &mut mpsc::Receiver<bool>,
    tx: &mpsc::Sender<AgentEvent>,
) -> Result<Exec, String> {
    let mutating = tools::risk(name) == tools::Risk::Mutate;
    if mutating && plan_mode {
        return Ok(Exec::error("blocked: plan mode is read-only".into()));
    }
    if mutating {
        emit(tx, AgentEvent::ApprovalRequested {
            name: name.to_string(),
            summary: summary.to_string(),
        })
        .await?;
        if !decisions.recv().await.unwrap_or(false) {
            return Ok(Exec::error("rejected by user".into()));
        }
    }
    let args = match args {
        Ok(args) => args.clone(),
        Err(message) => return Ok(Exec::error(message.clone())),
    };
    Ok(match tools::run(name, &args, ctx).await {
        Ok(outcome) => Exec { ok: outcome.ok, detail: outcome.detail, diff: outcome.diff },
        Err(message) => Exec::error(message),
    })
}

fn trunc(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let kept: String = text.chars().take(max).collect();
    format!("{kept}…")
}

struct Round {
    text: String,
    calls: Vec<ToolCall>,
}

struct StreamState {
    text: String,
    thinking: Option<(String, Instant)>,
    acc: ToolCallAccumulator,
}

async fn handle_payload(
    payload: &str,
    state: &mut StreamState,
    tx: &mpsc::Sender<AgentEvent>,
) -> Result<bool, String> {
    match parse_frame(payload) {
        Frame::Done => Ok(true),
        Frame::Error(message) => Err(format!("error provider: {message}")),
        Frame::Ignore => Ok(false),
        Frame::Delta(delta) => {
            if !delta.reasoning.is_empty() {
                let entry = state.thinking.get_or_insert_with(|| (String::new(), Instant::now()));
                entry.0.push_str(&delta.reasoning);
                emit(tx, AgentEvent::ThinkingDelta { text: delta.reasoning }).await?;
            }
            if !delta.text.is_empty() {
                state.text.push_str(&delta.text);
                emit(tx, AgentEvent::TextDelta { text: delta.text }).await?;
            }
            for call in delta.tool_calls {
                state.acc.apply(call);
            }
            Ok(false)
        }
    }
}

async fn stream_round(
    inner: &Inner,
    messages: &[Value],
    specs: &[Value],
    tx: &mpsc::Sender<AgentEvent>,
) -> Result<Round, String> {
    let url = format!("{}/chat/completions", inner.cfg.base_url.trim_end_matches('/'));
    let mut body = json!({ "model": inner.cfg.model, "messages": messages, "stream": true });
    if !specs.is_empty() {
        body["tools"] = Value::Array(specs.to_vec());
    }
    let mut attempt = 0u64;
    let response = loop {
        attempt += 1;
        let mut request = inner.client.post(&url).json(&body);
        if let Some(key) = &inner.cfg.api_key {
            request = request.bearer_auth(key);
        }
        match request.send().await {
            Ok(response) => {
                let status = response.status();
                if (status.as_u16() == 429 || status.is_server_error()) && attempt < 3 {
                    tokio::time::sleep(Duration::from_millis(600 * attempt)).await;
                    continue;
                }
                if !status.is_success() {
                    let text = response.text().await.unwrap_or_default();
                    return Err(format!("HTTP {status}{}", trunc(&format!(": {text}"), 400)));
                }
                break response;
            }
            Err(error) => {
                if attempt < 3 {
                    tokio::time::sleep(Duration::from_millis(600 * attempt)).await;
                    continue;
                }
                return Err(format!("gagal menghubungi {}: {error}", inner.cfg.base_url));
            }
        }
    };

    let mut stream = response.bytes_stream();
    let mut decoder = SseDecoder::new();
    let mut state = StreamState {
        text: String::new(),
        thinking: None,
        acc: ToolCallAccumulator::default(),
    };

    let mut finished = false;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("stream terputus: {e}"))?;
        for payload in decoder.push(&chunk) {
            if handle_payload(&payload, &mut state, tx).await? {
                finished = true;
                break;
            }
        }
        if finished {
            break;
        }
    }
    if !finished {
        for payload in decoder.finish() {
            if handle_payload(&payload, &mut state, tx).await? {
                break;
            }
        }
    }
    finish_round(state, tx).await
}

async fn finish_round(
    mut state: StreamState,
    tx: &mpsc::Sender<AgentEvent>,
) -> Result<Round, String> {
    if let Some((_, started)) = &state.thinking {
        emit(tx, AgentEvent::ThinkingFinished { seconds: started.elapsed().as_secs_f32() })
            .await?;
    }
    let calls = state.acc.take();
    if calls.is_empty() && state.text.trim().is_empty() {
        return Err("model mengembalikan respons kosong".into());
    }
    Ok(Round { text: state.text, calls })
}

fn tool_specs(plan_mode: bool) -> Vec<Value> {
    if !plan_mode {
        return tools::specs();
    }
    let read_only = ["read", "list", "grep"];
    tools::specs()
        .into_iter()
        .filter(|spec| {
            spec.pointer("/function/name")
                .and_then(Value::as_str)
                .is_some_and(|name| read_only.contains(&name))
        })
        .collect()
}

fn system_prompt(cfg: &AgentConfig, plan_mode: bool) -> String {
    let mut prompt = format!(
        "You are evrax, a coding agent running in a terminal TUI.\n\
         Workspace: {} · Platform: {} — the shell tool runs PowerShell.\n\
         Inspect code with read/list/grep before changing anything. Make minimal, precise edits.\n\
         After code changes, verify with `cargo test` (and `cargo clippy` when asked).\n\
         Keep replies concise. When the task is done, answer with plain text and no tool calls.",
        cfg.root.display(),
        std::env::consts::OS
    );
    if plan_mode {
        prompt.push_str("\nPLAN MODE: read-only. Never attempt write/edit/shell; investigate and present a plan.");
    }
    prompt
}

#[cfg(test)]
mod tests {
    use super::*;
    use tiny_http::{Response, Server};

    fn sse(events: &[&str]) -> String {
        let mut out = String::new();
        for event in events {
            out.push_str("data: ");
            out.push_str(event);
            out.push_str("\n\n");
        }
        out
    }

    /// Serves one scripted reply per request; records each request body.
    fn fake_server(replies: Vec<String>) -> (Arc<Mutex<Vec<String>>>, String) {
        let server = Arc::new(Server::http("127.0.0.1:0").unwrap());
        let addr = server.server_addr().to_ip().unwrap();
        let bodies = Arc::new(Mutex::new(Vec::new()));
        {
            let server = server.clone();
            let thread_bodies = bodies.clone();
            std::thread::spawn(move || {
                for reply in replies {
                    let mut request = match server.recv() {
                        Ok(request) => request,
                        Err(_) => return,
                    };
                    let mut body = String::new();
                    request.as_reader().read_to_string(&mut body).unwrap();
                    thread_bodies.lock().unwrap().push(body);
                    let _ = request.respond(Response::from_data(reply));
                }
            });
        }
        (bodies, format!("http://{addr}/v1"))
    }

    fn backend(base_url: String, root: PathBuf) -> ChatBackend {
        ChatBackend::new(AgentConfig {
            base_url,
            model: "test-model".into(),
            api_key: None,
            max_iterations: 4,
            root,
        })
        .unwrap()
    }

    async fn collect(backend: &ChatBackend, prompt: &str) -> Vec<AgentEvent> {
        let (tx, rx) = mpsc::channel(8);
        drop(tx);
        backend
            .run_turn(
                TurnRequest {
                    prompt: prompt.into(),
                    session: "unit-test".into(),
                    plan_mode: false,
                },
                rx,
            )
            .collect()
            .await
    }

    #[tokio::test]
    async fn streams_text_and_finishes() {
        let (_fake, url) = fake_server(vec![sse(&[
            r#"{"choices":[{"delta":{"reasoning_content":"hmm"}}]}"#,
            r#"{"choices":[{"delta":{"content":"halo "}}]}"#,
            r#"{"choices":[{"delta":{"content":"bos"}}]}"#,
            "[DONE]",
        ])]);
        let backend = backend(url, std::env::temp_dir());
        let events = collect(&backend, "sapa").await;
        assert!(events.iter().any(|e| matches!(
            e,
            AgentEvent::TextDelta { text } if text == "halo "
        )));
        assert!(events.iter().any(|e| matches!(e, AgentEvent::ThinkingDelta { .. })));
        assert!(events.iter().any(|e| matches!(e, AgentEvent::ThinkingFinished { .. })));
        assert_eq!(events.last(), Some(&AgentEvent::TurnFinished));
    }

    #[tokio::test]
    async fn executes_read_tool_then_finishes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "isi rahasia\n").unwrap();
        let bodies = Arc::new(Mutex::new(Vec::new()));
        let reply1 = sse(&[
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"read","arguments":"{\"path\":\"a.txt\"}"}}]}}]}"#,
            "[DONE]",
        ]);
        let reply2 = sse(&[r#"{"choices":[{"delta":{"content":"isinya: rahasia"}}]}"#, "[DONE]"]);
        let server = Server::http("127.0.0.1:0").unwrap();
        let addr = server.server_addr().to_ip().unwrap();
        let captured = bodies.clone();
        std::thread::spawn(move || {
            for reply in [reply1, reply2] {
                let mut request = server.recv().unwrap();
                let mut body = String::new();
                request.as_reader().read_to_string(&mut body).unwrap();
                captured.lock().unwrap().push(body);
                let _ = request.respond(Response::from_data(reply));
            }
        });
        let backend = backend(format!("http://{addr}/v1"), dir.path().to_path_buf());
        let events = collect(&backend, "baca a.txt").await;
        assert!(events.iter().any(|e| matches!(
            e,
            AgentEvent::ToolFinished { name, ok: true, detail }
                if name == "read" && detail.contains("rahasia")
        )));
        assert_eq!(events.last(), Some(&AgentEvent::TurnFinished));
        let second = bodies.lock().unwrap()[1].clone();
        assert!(second.contains("\"role\":\"tool\""), "second request: {second}");
    }

    #[tokio::test]
    async fn mutating_tool_requires_approval_and_reject_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let call = sse(&[
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"write","arguments":"{\"path\":\"x.txt\",\"content\":\"boom\"}"}}]}}]}"#,
            "[DONE]",
        ]);
        let final_text = sse(&[r#"{"choices":[{"delta":{"content":"ya sudah, tidak jadi"}}]}"#, "[DONE]"]);
        let server = Server::http("127.0.0.1:0").unwrap();
        let addr = server.server_addr().to_ip().unwrap();
        std::thread::spawn(move || {
            for reply in [call.clone(), final_text] {
                let mut request = server.recv().unwrap();
                let mut body = String::new();
                request.as_reader().read_to_string(&mut body).unwrap();
                let _ = request.respond(Response::from_data(reply));
            }
        });
        let backend = backend(format!("http://{addr}/v1"), dir.path().to_path_buf());
        let (decide_tx, decide_rx) = mpsc::channel(4);
        let mut stream = backend.run_turn(
            TurnRequest { prompt: "tulis".into(), session: "s".into(), plan_mode: false },
            decide_rx,
        );
        let mut rejected_seen = false;
        while let Some(event) = stream.next().await {
            if matches!(event, AgentEvent::ApprovalRequested { .. }) {
                decide_tx.send(false).await.unwrap();
            }
            if matches!(event, AgentEvent::ToolFinished { ok: false, detail, .. } if detail == "rejected by user") {
                rejected_seen = true;
            }
        }
        assert!(rejected_seen);
        assert!(!dir.path().join("x.txt").exists());
    }

    #[tokio::test]
    async fn plan_mode_blocks_mutating_tools_without_approval() {
        let dir = tempfile::tempdir().unwrap();
        let call = sse(&[
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"shell","arguments":"{\"command\":\"echo hi\"}"}}]}}]}"#,
            "[DONE]",
        ]);
        let final_text = sse(&[r#"{"choices":[{"delta":{"content":"encana matang"}}]}"#, "[DONE]"]);
        let server = Server::http("127.0.0.1:0").unwrap();
        let addr = server.server_addr().to_ip().unwrap();
        std::thread::spawn(move || {
            for reply in [call.clone(), final_text] {
                let mut request = server.recv().unwrap();
                let mut body = String::new();
                request.as_reader().read_to_string(&mut body).unwrap();
                let _ = request.respond(Response::from_data(reply));
            }
        });
        let backend = backend(format!("http://{addr}/v1"), dir.path().to_path_buf());
        let (decide_tx, decide_rx) = mpsc::channel(2);
        let mut stream = backend.run_turn(
            TurnRequest { prompt: "jalankan".into(), session: "p".into(), plan_mode: true },
            decide_rx,
        );
        let mut saw_block = false;
        let mut saw_approval = false;
        while let Some(event) = stream.next().await {
            if matches!(event, AgentEvent::ApprovalRequested { .. }) {
                saw_approval = true;
            }
            if matches!(event, AgentEvent::ToolFinished { detail, ok: false, .. } if detail.contains("plan mode")) {
                saw_block = true;
            }
        }
        assert!(saw_block);
        assert!(!saw_approval);
        drop(decide_tx);
    }

    #[tokio::test]
    async fn http_error_surfaces_as_failed_event() {
        let server = Server::http("127.0.0.1:0").unwrap();
        let addr = server.server_addr().to_ip().unwrap();
        std::thread::spawn(move || {
            for _ in 0..3 {
                let mut request = server.recv().unwrap();
                let mut body = String::new();
                request.as_reader().read_to_string(&mut body).unwrap();
                let _ = request.respond(Response::from_string("kaboom").with_status_code(400));
            }
        });
        let backend = backend(format!("http://{addr}/v1"), std::env::temp_dir());
        let events = collect(&backend, "halo").await;
        let Some(AgentEvent::Failed { message }) = events.last() else {
            panic!("expected Failed, got {events:?}");
        };
        assert!(message.contains("400"));
    }

    #[tokio::test]
    async fn history_is_replayed_for_the_same_session() {
        let reply = sse(&[r#"{"choices":[{"delta":{"content":"oke"}}]}"#, "[DONE]"]);
        let server = Server::http("127.0.0.1:0").unwrap();
        let addr = server.server_addr().to_ip().unwrap();
        let bodies = Arc::new(Mutex::new(Vec::new()));
        let captured = bodies.clone();
        std::thread::spawn(move || {
            for _ in 0..2 {
                let mut request = server.recv().unwrap();
                let mut body = String::new();
                request.as_reader().read_to_string(&mut body).unwrap();
                captured.lock().unwrap().push(body);
                let _ = request.respond(Response::from_data(reply.clone()));
            }
        });
        let backend = backend(format!("http://{addr}/v1"), std::env::temp_dir());
        let (tx, rx) = mpsc::channel(1);
        drop(tx);
        let mut stream = backend.run_turn(
            TurnRequest { prompt: "pertanyaan pertama".into(), session: "same".into(), plan_mode: false },
            rx,
        );
        while stream.next().await.is_some() {}
        let (tx, rx) = mpsc::channel(1);
        drop(tx);
        let mut stream = backend.run_turn(
            TurnRequest { prompt: "lanjutan".into(), session: "same".into(), plan_mode: false },
            rx,
        );
        while stream.next().await.is_some() {}
        let second = bodies.lock().unwrap()[1].clone();
        assert!(second.contains("pertanyaan pertama"), "second body: {second}");
        assert!(second.contains("oke"));
    }

    #[tokio::test]
    #[ignore = "butuh 9router live + %APPDATA%\\evrax\\agent.json"]
    async fn live_9router_text_roundtrip() {
        let backend = ChatBackend::from_config().unwrap();
        let (tx, rx) = mpsc::channel(1);
        drop(tx);
        let events = backend
            .run_turn(
                TurnRequest {
                    prompt: "Balas tepat: OK".into(),
                    session: "live-smoke".into(),
                    plan_mode: false,
                },
                rx,
            )
            .collect::<Vec<_>>()
            .await;
        let text: String = events
            .iter()
            .filter_map(|e| {
                if let AgentEvent::TextDelta { text } = e {
                    Some(text.as_str())
                } else {
                    None
                }
            })
            .collect();
        println!("live reply: {text:?}");
        assert!(
            matches!(events.last(), Some(AgentEvent::TurnFinished)),
            "events: {events:?}"
        );
        assert!(!text.trim().is_empty(), "model balas kosong");
    }

    #[tokio::test]
    #[ignore = "live 9router + tools (%APPDATA%\\evrax\\agent.json)"]
    async fn live_9router_uses_a_read_tool() {
        let backend = ChatBackend::from_config().unwrap();
        let (tx, rx) = mpsc::channel(1);
        drop(tx);
        let events = backend
            .run_turn(
                TurnRequest {
                    prompt: "Pakai tool read untuk membaca file Cargo.toml, lalu balas satu baris ringkas isinya."
                        .into(),
                    session: "live-tools".into(),
                    plan_mode: false,
                },
                rx,
            )
            .collect::<Vec<_>>()
            .await;
        for event in &events {
            println!("{event:?}");
        }
        assert!(
            events
                .iter()
                .any(|e| matches!(e, AgentEvent::ToolStarted { name, .. } if name == "read")),
            "no read tool call: {events:?}"
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, AgentEvent::ToolFinished { ok: true, .. })),
            "tool failed: {events:?}"
        );
        assert!(matches!(events.last(), Some(AgentEvent::TurnFinished)));
    }

    #[test]
    fn config_file_overrides_defaults_and_env_ignores_unknown() {
        let mut config = AgentConfig::default();
        apply(&mut config, ConfigFile {
            base_url: Some("http://x:1/v1".into()),
            model: Some("m".into()),
            api_key: None,
            max_iterations: Some(3),
            mock: None,
        });
        assert_eq!(config.base_url, "http://x:1/v1");
        assert_eq!(config.max_iterations, 3);
    }

    #[test]
    fn plan_mode_specs_are_read_only() {        let specs = tool_specs(true);
        assert_eq!(specs.len(), 3);
        let names: Vec<&str> = specs
            .iter()
            .map(|s| s.pointer("/function/name").and_then(Value::as_str).unwrap())
            .collect();
        assert_eq!(names, ["read", "list", "grep"]);
    }

    #[test]
    fn history_commit_trims_oldest_turns() {
        let inner = Inner {
            cfg: AgentConfig {
                base_url: DEFAULT_BASE_URL.into(),
                model: DEFAULT_MODEL.into(),
                api_key: None,
                max_iterations: 12,
                root: std::env::temp_dir(),
            },
            client: reqwest::Client::new(),
            conversations: Mutex::new(HashMap::new()),
        };
        let big = "x".repeat(MAX_HISTORY_CHARS / 2 + 10);
        for i in 0..3 {
            inner.commit("s", vec![json!({"role":"user","content":format!("q{i} {big}")})]);
        }
        let replay = inner.replay("s");
        assert_eq!(replay.len(), 1, "only the newest turn survives");
        assert!(replay[0]["content"].as_str().unwrap().contains("q2"));
    }
}
