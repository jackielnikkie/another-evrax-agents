//! Real evrax backend: OpenAI-compatible SSE chat (9router) + agent loop.

pub mod catalog;
pub mod diff;
mod guardrails;
mod retry;
pub mod sse;
pub mod subagent;
pub mod tools;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use evrax_core::{AgentBackend, AgentEvent, DiffLine, TurnRequest};
use futures_util::StreamExt;
use futures_util::future::BoxFuture;
use futures_util::stream::{self, BoxStream};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::sse::{Frame, SseDecoder, ToolCall, ToolCallAccumulator, parse_frame};

pub const DEFAULT_BASE_URL: &str = "http://127.0.0.1:20128/v1";
pub const DEFAULT_MODEL: &str = "bebas";
const MAX_HISTORY_CHARS: usize = 100_000;
const MAX_HISTORY_SESSIONS: usize = 24;

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
            max_iterations: 500,
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

pub fn config_file_path() -> Option<PathBuf> {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .map(|dir| dir.join("evrax").join("agent.json"))
}

/// Persists provider settings, preserving unrelated fields and backing up the
/// previous file as `agent.json.bak-<ms>`.
pub fn save_config(cfg: &AgentConfig, mock: bool) -> anyhow::Result<PathBuf> {
    let path = config_file_path().ok_or_else(|| anyhow::anyhow!("no config directory"))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut value = if path.is_file() {
        let text = std::fs::read_to_string(&path)?;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let _ = std::fs::copy(&path, path.with_extension(format!("json.bak-{stamp}")));
        serde_json::from_str::<Value>(&text).unwrap_or_else(|_| Value::Object(Default::default()))
    } else {
        Value::Object(Default::default())
    };
    let object = value
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("agent.json is not a JSON object"))?;
    object.insert("base_url".into(), Value::String(cfg.base_url.clone()));
    object.insert("model".into(), Value::String(cfg.model.clone()));
    match cfg.api_key.as_deref() {
        Some(key) if !key.is_empty() => {
            object.insert("api_key".into(), Value::String(key.to_string()));
        }
        _ => {
            object.remove("api_key");
        }
    }
    object.insert(
        "max_iterations".into(),
        Value::Number(cfg.max_iterations.into()),
    );
    object.insert("mock".into(), Value::Bool(mock));
    std::fs::write(&path, serde_json::to_string_pretty(&value)?)?;
    Ok(path)
}

/// One-shot provider check used by the setup wizard: a tiny chat request (never
/// `/models`, which some gateways answer even when the key is rejected).
pub async fn test_provider(cfg: &AgentConfig) -> Result<String, String> {
    let mut builder = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .read_timeout(Duration::from_secs(30));
    let lower = cfg.base_url.to_lowercase();
    if lower.contains("127.0.0.1") || lower.contains("localhost") || lower.contains("[::1]") {
        builder = builder.no_proxy();
    }
    let client = builder.build().map_err(|e| e.to_string())?;
    let url = format!("{}/chat/completions", cfg.base_url.trim_end_matches('/'));
    let body = json!({
        "model": cfg.model,
        "messages": [{"role": "user", "content": "Reply with the single word OK."}],
        "max_tokens": 16,
        "temperature": 0,
        "stream": false,
    });
    let mut request = client.post(url).json(&body);
    if let Some(key) = cfg.api_key.as_deref().filter(|k| !k.is_empty()) {
        request = request.bearer_auth(key);
    }
    let response = request
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;
    let status = response.status();
    let text = response.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!(
            "HTTP {}: {}",
            status.as_u16(),
            trunc(&text, 240).replace('\n', " ")
        ));
    }
    let value: Value = serde_json::from_str(&text).map_err(|e| format!("bad JSON: {e}"))?;
    let reply = value["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("")
        .trim()
        .to_string();
    if reply.is_empty() {
        Ok(format!("HTTP {} OK", status.as_u16()))
    } else {
        Ok(reply)
    }
}

struct Inner {
    cfg: AgentConfig,
    client: reqwest::Client,
    store: Option<evrax_store::ProjectStore>,
    conversations: Mutex<HashMap<String, Vec<Vec<Value>>>>,
    /// Recency clock per session, used to evict cold conversations.
    session_use: Mutex<HashMap<String, u64>>,
    clock: AtomicU64,
}

impl Inner {
    #[cfg(test)]
    fn for_test() -> Self {
        Self {
            cfg: AgentConfig {
                base_url: DEFAULT_BASE_URL.into(),
                model: DEFAULT_MODEL.into(),
                api_key: None,
                max_iterations: 500,
                root: std::env::temp_dir(),
            },
            client: reqwest::Client::new(),
            store: None,
            conversations: Mutex::new(HashMap::new()),
            session_use: Mutex::new(HashMap::new()),
            clock: AtomicU64::new(0),
        }
    }

    fn bump(&self, session: &str) {
        let tick = self.clock.fetch_add(1, Ordering::Relaxed) + 1;
        self.session_use
            .lock()
            .unwrap()
            .insert(session.to_string(), tick);
    }

    fn replay(&self, session: &str) -> Vec<Value> {
        self.bump(session);
        self.load_history(session);
        let conversations = self.conversations.lock().unwrap();
        conversations
            .get(session)
            .into_iter()
            .flatten()
            .flatten()
            .cloned()
            .collect()
    }

    fn load_history(&self, session: &str) {
        if self.store.is_none() {
            return;
        }
        if self.conversations.lock().unwrap().contains_key(session) {
            return;
        }
        let Some(store) = self.store.as_ref() else {
            return;
        };
        if let Ok(Some(history)) = store.read_json::<Vec<Vec<Value>>>(session, "history.json") {
            self.conversations
                .lock()
                .unwrap()
                .insert(session.to_string(), history);
        }
    }

    fn commit(&self, session: &str, turn: Vec<Value>) {
        self.bump(session);
        {
            let mut conversations = self.conversations.lock().unwrap();
            let turns = conversations.entry(session.to_string()).or_default();
            turns.push(turn);
            let size = |messages: &[Value]| -> usize { messages.iter().map(message_chars).sum() };
            let mut total: usize = turns.iter().map(|t| size(t)).sum();
            while total > MAX_HISTORY_CHARS && turns.len() > 1 {
                total = total.saturating_sub(size(&turns[0]));
                turns.remove(0);
            }
            if conversations.len() > MAX_HISTORY_SESSIONS {
                let mut use_map = self.session_use.lock().unwrap();
                let current = session.to_string();
                let victim = conversations
                    .keys()
                    .filter(|key| **key != current)
                    .min_by_key(|key| use_map.get(*key).copied().unwrap_or(0))
                    .cloned();
                if let Some(victim) = victim {
                    conversations.remove(&victim);
                    use_map.remove(&victim);
                }
            }
        }
        if let Some(store) = self.store.as_ref() {
            let snapshot = self
                .conversations
                .lock()
                .unwrap()
                .get(session)
                .cloned()
                .unwrap_or_default();
            let _ = store.write_json(session, "history.json", &snapshot);
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
        Self::new_with_project(cfg, None)
    }

    pub fn new_with_project(
        cfg: AgentConfig,
        store: Option<evrax_store::ProjectStore>,
    ) -> anyhow::Result<Self> {
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
                store,
                conversations: Mutex::new(HashMap::new()),
                session_use: Mutex::new(HashMap::new()),
                clock: AtomicU64::new(0),
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
    fn suggest_title(&self, prompt: String, reply: String) -> BoxFuture<'static, Option<String>> {
        let inner = self.inner.clone();
        Box::pin(async move {
            let url = format!(
                "{}/chat/completions",
                inner.cfg.base_url.trim_end_matches('/')
            );
            let body = json!({
                "model": inner.cfg.model,
                "messages": [
                    {"role": "system", "content": TITLE_SYSTEM},
                    {"role": "user", "content": format!(
                        "PROMPT:\n{}\n\nREPLY:\n{}",
                        trunc(&prompt, 1200),
                        trunc(&reply, 1200)
                    )}
                ],
                "stream": false,
                "max_tokens": 16,
                "temperature": 0.2
            });
            let call = async {
                let mut request = inner.client.post(&url).json(&body);
                if let Some(key) = &inner.cfg.api_key {
                    request = request.bearer_auth(key);
                }
                let response = request.send().await.ok()?;
                if !response.status().is_success() {
                    return None;
                }
                let value: Value = response.json().await.ok()?;
                let text = value.pointer("/choices/0/message/content")?.as_str()?;
                sanitize_title(text)
            };
            tokio::time::timeout(Duration::from_secs(10), call)
                .await
                .ok()
                .flatten()
        })
    }
}

const TITLE_SYSTEM: &str = "Write a session title: 3 to 6 words naming the task, in the language of the prompt. Plain words only: no quotes, no emoji, no trailing punctuation or colon.";

/// Normalizes a model-proposed title into a single short display line.
pub fn sanitize_title(raw: &str) -> Option<String> {
    let line = raw.lines().map(str::trim).find(|l| !l.is_empty())?;
    let trimmed = line.trim_matches(|c: char| {
        c.is_whitespace() || matches!(c, '"' | '\'' | '`' | '*' | '#' | '_' | '>' | '-')
    });
    let mut words: Vec<&str> = trimmed.split_whitespace().collect();
    if words.is_empty() {
        return None;
    }
    words.truncate(8);
    let mut title = words.join(" ");
    title = title
        .trim_matches(|c: char| {
            c.is_whitespace()
                || matches!(
                    c,
                    '"' | '\'' | '`' | ':' | ';' | '.' | ',' | '!' | '?' | '*' | '#'
                )
        })
        .to_string();
    if title.chars().count() > 60 {
        let mut cut: String = title.chars().take(60).collect();
        if let Some(pos) = cut.rfind(' ') {
            cut.truncate(pos);
        }
        title = cut;
    }
    title = title.trim().to_string();
    if title.chars().count() < 2 {
        return None;
    }
    Some(title)
}

struct Exec {
    ok: bool,
    detail: String,
    diff: Option<(String, Vec<DiffLine>)>,
}

impl Exec {
    fn error(detail: String) -> Self {
        Self {
            ok: false,
            detail,
            diff: None,
        }
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
    tx.send(event)
        .await
        .map_err(|_| "turn dibatalkan".to_string())
}

const SUMMARY_NUDGE: &str = "You've reached the maximum number of tool-calling iterations allowed. Please provide a final response summarizing what you've found and accomplished so far, without calling any more tools.";

async fn run_turn(
    inner: Arc<Inner>,
    request: TurnRequest,
    mut decisions: mpsc::Receiver<bool>,
    tx: mpsc::Sender<AgentEvent>,
) -> Result<(), String> {
    let system =
        json!({ "role": "system", "content": system_prompt(&inner.cfg, request.plan_mode) });
    let history = inner.replay(&request.session);
    let specs = tool_specs(request.plan_mode);
    let ctx = tools::Context::new(inner.cfg.root.clone());
    let mut committed: Vec<Value> = vec![json!({ "role": "user", "content": request.prompt })];
    let max = inner.cfg.max_iterations.max(1);
    let mut tool_rounds = 0usize;
    let mut balanced = 1usize;
    let mut guard = guardrails::Guard::default();

    loop {
        if tool_rounds >= max {
            return finish_budget(
                &inner,
                &request.session,
                &history,
                &system,
                committed,
                max,
                &tx,
            )
            .await;
        }
        emit(
            &tx,
            AgentEvent::Iteration {
                n: tool_rounds + 1,
                max,
            },
        )
        .await?;

        let mut messages = Vec::with_capacity(history.len() + committed.len() + 1);
        messages.push(system.clone());
        messages.extend(history.iter().cloned());
        messages.extend(committed.iter().cloned());

        let round = match stream_round(&inner, &messages, &specs, &tx).await {
            Ok(round) => round,
            Err(error) => {
                commit_partial(&inner, &request.session, &committed, balanced);
                return Err(error);
            }
        };
        if round.calls.is_empty() {
            committed.push(json!({ "role": "assistant", "content": round.text }));
            inner.commit(&request.session, committed);
            return emit(&tx, AgentEvent::TurnFinished).await;
        }

        tool_rounds += 1;
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
            emit(
                &tx,
                AgentEvent::ToolStarted {
                    name: call.name.clone(),
                    summary: summary.clone(),
                },
            )
            .await?;

            if let Some(reason) = guard.blocked(&call.name, &call.arguments) {
                emit(
                    &tx,
                    AgentEvent::ToolFinished {
                        name: call.name.clone(),
                        ok: false,
                        detail: reason.clone(),
                    },
                )
                .await?;
                committed.push(json!({
                    "role": "tool",
                    "tool_call_id": call.id,
                    "content": reason
                }));
                continue;
            }

            let outcome = match authorize_and_run(
                &call.name,
                &args,
                &summary,
                &ctx,
                request.plan_mode,
                &mut decisions,
                &tx,
            )
            .await
            {
                Ok(outcome) => outcome,
                Err(error) => {
                    commit_partial(&inner, &request.session, &committed, balanced);
                    return Err(error);
                }
            };
            if let Some((path, lines)) = &outcome.diff {
                emit(
                    &tx,
                    AgentEvent::Diff {
                        path: path.clone(),
                        lines: lines.clone(),
                    },
                )
                .await?;
            }
            emit(
                &tx,
                AgentEvent::ToolFinished {
                    name: call.name.clone(),
                    ok: outcome.ok,
                    detail: trunc(&outcome.detail, 2000),
                },
            )
            .await?;

            let mut result = outcome.result_text();
            if let Some(stub) = guard.observe(&call.name, &call.arguments, outcome.ok, &result) {
                result = stub;
            }
            if let Some(notice) = guard.take_notice() {
                emit(&tx, AgentEvent::Notice { message: notice }).await?;
            }
            committed.push(json!({
                "role": "tool",
                "tool_call_id": call.id,
                "content": result
            }));
        }
        balanced = committed.len();
    }
}

/// Persists only the fully balanced prefix of a failed turn (user message plus
/// complete assistant/tool rounds), so the next `continue` resumes with context.
fn commit_partial(inner: &Inner, session: &str, committed: &[Value], balanced: usize) {
    if balanced > 1 && committed.len() >= balanced {
        inner.commit(session, committed[..balanced].to_vec());
    }
}

/// Grace request when the tool-call budget runs out: ask for a summary without
/// tools, retry once, then fall back to a `continue` hint and commit the turn.
#[allow(clippy::too_many_arguments)]
async fn finish_budget(
    inner: &Inner,
    session: &str,
    history: &[Value],
    system: &Value,
    mut committed: Vec<Value>,
    max: usize,
    tx: &mpsc::Sender<AgentEvent>,
) -> Result<(), String> {
    emit(
        tx,
        AgentEvent::Notice {
            message: format!("batas iterasi tool tercapai ({max}/{max}) — minta ringkasan"),
        },
    )
    .await?;
    committed.push(json!({ "role": "user", "content": SUMMARY_NUDGE }));

    let mut text = String::new();
    let mut last_error = None;
    for attempt in 0..2 {
        let mut messages = Vec::with_capacity(history.len() + committed.len() + 1);
        messages.push(system.clone());
        messages.extend(history.iter().cloned());
        messages.extend(committed.iter().cloned());
        match stream_round(inner, &messages, &[], tx).await {
            Ok(round) if !round.text.trim().is_empty() => {
                text = round.text;
                break;
            }
            Ok(_) => last_error = Some("respons kosong".to_string()),
            Err(error) => last_error = Some(error),
        }
        if attempt == 0 {
            emit(
                tx,
                AgentEvent::Notice {
                    message: "ringkasan gagal, coba sekali lagi".into(),
                },
            )
            .await?;
        }
    }

    if text.trim().is_empty() {
        let detail = last_error.unwrap_or_else(|| "respons kosong".into());
        text = format!(
            "Batas langkah turn ini habis ({max} tool call). Kirim `continue` untuk melanjutkan, atau naikkan `max_iterations` di agent.json."
        );
        emit(
            tx,
            AgentEvent::Notice {
                message: format!(
                    "ringkasan tidak tersedia ({}) — pakai pesan fallback",
                    trunc(&detail, 140)
                ),
            },
        )
        .await?;
        emit(tx, AgentEvent::TextDelta { text: text.clone() }).await?;
    }

    committed.push(json!({ "role": "assistant", "content": text }));
    inner.commit(session, committed);
    emit(tx, AgentEvent::TurnFinished).await
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
    let mutating = tools::is_mutating(name, args.as_ref().ok());
    let outside = match args {
        Ok(value) => tools::outside_target(name, value, ctx),
        Err(_) => None,
    };
    if plan_mode {
        if mutating {
            return Ok(Exec::error("blocked: plan mode is read-only".into()));
        }
        if outside.is_some() {
            return Ok(Exec::error(
                "blocked: plan mode is workspace read-only".into(),
            ));
        }
    }
    // shell always needs approval (system-wide scope); mutating git actions
    // need approval; write/edit/todo/ast-grep inside the workspace run
    // automatically; any external path needs approval.
    let needs_approval = tools::requires_approval(name, args.as_ref().ok()) || outside.is_some();
    if needs_approval {
        let key = tools::approval_key_for(name, args.as_ref().ok(), outside.as_deref());
        emit(
            tx,
            AgentEvent::ApprovalRequested {
                name: name.to_string(),
                summary: summary.to_string(),
                key,
            },
        )
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
        Ok(outcome) => Exec {
            ok: outcome.ok,
            detail: outcome.detail,
            diff: outcome.diff,
        },
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

fn provider_error(status: reqwest::StatusCode, body: &str, model: &str) -> String {
    let detail = trunc(body.trim(), 400);
    let hint = match status.as_u16() {
        401 => {
            "API key ditolak 9router (invalid/nonaktif) — cek `api_key` di agent.json dan status key di dashboard 9router"
        }
        402 => "kuota/kredit provider habis",
        403 => {
            "9router menolak key ini untuk model tersebut — buka dashboard 9router → API Keys, izinkan model itu atau matikan restrict access"
        }
        _ => "",
    };
    if hint.is_empty() {
        format!("HTTP {status}: {detail}")
    } else {
        let model = if status.as_u16() == 403 {
            format!(" (model `{model}`)")
        } else {
            String::new()
        };
        format!("HTTP {status}: {hint}{model}. {detail}")
    }
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
                let entry = state
                    .thinking
                    .get_or_insert_with(|| (String::new(), Instant::now()));
                entry.0.push_str(&delta.reasoning);
                emit(
                    tx,
                    AgentEvent::ThinkingDelta {
                        text: delta.reasoning,
                    },
                )
                .await?;
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
    let url = format!(
        "{}/chat/completions",
        inner.cfg.base_url.trim_end_matches('/')
    );
    let mut body = json!({ "model": inner.cfg.model, "messages": messages, "stream": true });
    if !specs.is_empty() {
        body["tools"] = Value::Array(specs.to_vec());
    }
    let mut retry_state = retry::RetryState::default();
    let response = loop {
        let mut request = inner.client.post(&url).json(&body);
        if let Some(key) = &inner.cfg.api_key {
            request = request.bearer_auth(key);
        }
        match request.send().await {
            Ok(response) => {
                let status = response.status();
                if status.is_success() {
                    break response;
                }
                let headers = response.headers().clone();
                let text = response.text().await.unwrap_or_default();
                let failure = retry::classify_status(status, &headers);
                if let Some(delay) = retry_state.next(&failure) {
                    emit(
                        tx,
                        AgentEvent::Retrying {
                            attempt: retry_state.retries,
                            max: retry::MAX_RETRIES,
                            delay_ms: delay,
                            reason: failure.reason().to_string(),
                        },
                    )
                    .await?;
                    tokio::time::sleep(retry::sleep(delay)).await;
                    continue;
                }
                return Err(provider_error(status, &text, &inner.cfg.model));
            }
            Err(error) => {
                let failure = retry::classify_error(&error);
                if let Some(delay) = retry_state.next(&failure) {
                    emit(
                        tx,
                        AgentEvent::Retrying {
                            attempt: retry_state.retries,
                            max: retry::MAX_RETRIES,
                            delay_ms: delay,
                            reason: failure.reason().to_string(),
                        },
                    )
                    .await?;
                    tokio::time::sleep(retry::sleep(delay)).await;
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
        emit(
            tx,
            AgentEvent::ThinkingFinished {
                seconds: started.elapsed().as_secs_f32(),
            },
        )
        .await?;
    }
    let calls = state.acc.take();
    if calls.is_empty() && state.text.trim().is_empty() {
        return Err("model mengembalikan respons kosong".into());
    }
    Ok(Round {
        text: state.text,
        calls,
    })
}

fn tool_specs(plan_mode: bool) -> Vec<Value> {
    if !plan_mode {
        return tools::specs();
    }
    let read_only = [
        "read",
        "list",
        "grep",
        "web_fetch",
        "todo",
        "git",
        "ast_grep",
        "evrax-graph",
    ];
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
         Use todo for multi-step work, evrax-graph for code-graph queries (index/search/callers/impact/outline), git for repo inspection/mutations, web_fetch for docs, ast_grep for structural code search, and subagent to launch/control another evrax terminal agent.\n\
         After code changes, verify with `cargo test` (and `cargo clippy` when asked).\n\
         File tools run automatically inside the workspace (read AND write/edit). Any path outside the workspace needs explicit user approval, so ask before touching files elsewhere. Never write outside the workspace unless the user asked for it.\n\
         Keep replies concise. When the task is done, answer with plain text and no tool calls.",
        cfg.root.display(),
        std::env::consts::OS
    );
    if plan_mode {
        prompt.push_str("\nPLAN MODE: read-only and workspace-only. Read-only web_fetch, todo list, git status/diff/log/show, ast_grep search, and evrax-graph queries/index are allowed; never attempt write/edit/shell or mutating todo/git/ast-grep/subagent actions, and never read files outside the workspace; investigate and present a plan.");
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

    /// One SSE reply that invokes `name` with `args` (properly JSON-escaped).
    fn tool_call_reply(name: &str, args: Value) -> String {
        let frame = json!({
            "choices": [{ "delta": { "tool_calls": [{
                "index": 0,
                "id": "c1",
                "function": { "name": name, "arguments": args.to_string() }
            }] } }]
        });
        sse(&[&frame.to_string(), "[DONE]"])
    }

    struct FakeReply {
        status: u16,
        headers: Vec<(&'static str, &'static str)>,
        body: String,
    }

    impl FakeReply {
        fn ok(body: String) -> Self {
            Self {
                status: 200,
                headers: Vec::new(),
                body,
            }
        }

        fn status(status: u16, body: &str) -> Self {
            Self {
                status,
                headers: Vec::new(),
                body: body.to_string(),
            }
        }
    }

    /// Serves one scripted reply per request; records each request body.
    fn fake_server(replies: Vec<String>) -> (Arc<Mutex<Vec<String>>>, String) {
        fake_server_raw(replies.into_iter().map(FakeReply::ok).collect())
    }

    fn fake_server_raw(replies: Vec<FakeReply>) -> (Arc<Mutex<Vec<String>>>, String) {
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
                    let mut response =
                        Response::from_data(reply.body).with_status_code(reply.status);
                    for (name, value) in reply.headers {
                        if let Ok(header) = tiny_http::Header::from_bytes(name, value) {
                            response.add_header(header);
                        }
                    }
                    let _ = request.respond(response);
                }
            });
        }
        (bodies, format!("http://{addr}/v1"))
    }

    fn backend_with(base_url: String, root: PathBuf, max_iterations: usize) -> ChatBackend {
        ChatBackend::new(AgentConfig {
            base_url,
            model: "test-model".into(),
            api_key: None,
            max_iterations,
            root,
        })
        .unwrap()
    }

    fn backend(base_url: String, root: PathBuf) -> ChatBackend {
        backend_with(base_url, root, 4)
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
        assert!(
            events
                .iter()
                .any(|e| matches!(e, AgentEvent::ThinkingDelta { .. }))
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, AgentEvent::ThinkingFinished { .. }))
        );
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
        let reply2 = sse(&[
            r#"{"choices":[{"delta":{"content":"isinya: rahasia"}}]}"#,
            "[DONE]",
        ]);
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
        assert!(
            second.contains("\"role\":\"tool\""),
            "second request: {second}"
        );
    }

    #[tokio::test]
    async fn write_inside_workspace_runs_without_approval() {
        let dir = tempfile::tempdir().unwrap();
        let call = sse(&[
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"write","arguments":"{\"path\":\"x.txt\",\"content\":\"boom\"}"}}]}}]}"#,
            "[DONE]",
        ]);
        let final_text = sse(&[
            r#"{"choices":[{"delta":{"content":"sudah ditulis"}}]}"#,
            "[DONE]",
        ]);
        let (bodies, url) = fake_server(vec![call, final_text]);
        let backend = backend(url, dir.path().to_path_buf());
        let events = collect(&backend, "tulis").await;
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, AgentEvent::ApprovalRequested { .. })),
            "inside write must not ask for approval: {events:?}"
        );
        assert!(events.iter().any(|e| matches!(
            e,
            AgentEvent::ToolFinished { name, ok: true, .. } if name == "write"
        )));
        assert!(dir.path().join("x.txt").exists());
        assert_eq!(bodies.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn outside_read_requires_approval_with_directory_key() {
        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "rahasia luar\n").unwrap();
        let target = outside.path().join("secret.txt");
        let call = tool_call_reply("read", json!({ "path": target.to_string_lossy() }));
        let final_text = sse(&[r#"{"choices":[{"delta":{"content":"oke"}}]}"#, "[DONE]"]);
        let server = Server::http("127.0.0.1:0").unwrap();
        let addr = server.server_addr().to_ip().unwrap();
        std::thread::spawn(move || {
            for reply in [call, final_text] {
                let mut request = server.recv().unwrap();
                let mut body = String::new();
                request.as_reader().read_to_string(&mut body).unwrap();
                let _ = request.respond(Response::from_data(reply));
            }
        });
        let backend = backend(format!("http://{addr}/v1"), workspace.path().to_path_buf());
        let (decide_tx, decide_rx) = mpsc::channel(4);
        let mut stream = backend.run_turn(
            TurnRequest {
                prompt: "baca luar".into(),
                session: "o".into(),
                plan_mode: false,
            },
            decide_rx,
        );
        let expected = tools::approval_key("read", Some(&target));
        let mut key_seen = None;
        let mut read_ok = false;
        while let Some(event) = stream.next().await {
            match event {
                AgentEvent::ApprovalRequested { name, key, .. } => {
                    assert_eq!(name, "read");
                    key_seen = Some(key);
                    decide_tx.send(true).await.unwrap();
                }
                AgentEvent::ToolFinished {
                    name,
                    ok: true,
                    detail,
                } if name == "read" && detail.contains("rahasia luar") => {
                    read_ok = true;
                }
                _ => {}
            }
        }
        assert_eq!(key_seen.as_deref(), Some(expected.as_str()));
        assert!(expected.starts_with("read:"));
        assert!(read_ok);
    }

    #[tokio::test]
    async fn plan_mode_blocks_outside_path_without_approval() {
        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "rahasia\n").unwrap();
        let target = outside.path().join("secret.txt");
        let call = tool_call_reply("read", json!({ "path": target.to_string_lossy() }));
        let final_text = sse(&[r#"{"choices":[{"delta":{"content":"encana"}}]}"#, "[DONE]"]);
        let server = Server::http("127.0.0.1:0").unwrap();
        let addr = server.server_addr().to_ip().unwrap();
        std::thread::spawn(move || {
            for reply in [call, final_text] {
                let mut request = server.recv().unwrap();
                let mut body = String::new();
                request.as_reader().read_to_string(&mut body).unwrap();
                let _ = request.respond(Response::from_data(reply));
            }
        });
        let backend = backend(format!("http://{addr}/v1"), workspace.path().to_path_buf());
        let (decide_tx, decide_rx) = mpsc::channel(2);
        let mut stream = backend.run_turn(
            TurnRequest {
                prompt: "baca luar".into(),
                session: "po".into(),
                plan_mode: true,
            },
            decide_rx,
        );
        let mut saw_approval = false;
        let mut saw_block = false;
        while let Some(event) = stream.next().await {
            match event {
                AgentEvent::ApprovalRequested { .. } => saw_approval = true,
                AgentEvent::ToolFinished {
                    detail, ok: false, ..
                } if detail.contains("plan mode is workspace read-only") => {
                    saw_block = true;
                }
                _ => {}
            }
        }
        assert!(saw_block);
        assert!(!saw_approval);
        drop(decide_tx);
    }

    #[tokio::test]
    async fn shell_always_requires_approval_with_shell_key() {
        let dir = tempfile::tempdir().unwrap();
        let call = sse(&[
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"shell","arguments":"{\"command\":\"echo hi\"}"}}]}}]}"#,
            "[DONE]",
        ]);
        let final_text = sse(&[r#"{"choices":[{"delta":{"content":"selesai"}}]}"#, "[DONE]"]);
        let server = Server::http("127.0.0.1:0").unwrap();
        let addr = server.server_addr().to_ip().unwrap();
        std::thread::spawn(move || {
            for reply in [call, final_text] {
                let mut request = server.recv().unwrap();
                let mut body = String::new();
                request.as_reader().read_to_string(&mut body).unwrap();
                let _ = request.respond(Response::from_data(reply));
            }
        });
        let backend = backend(format!("http://{addr}/v1"), dir.path().to_path_buf());
        let (decide_tx, decide_rx) = mpsc::channel(4);
        let mut stream = backend.run_turn(
            TurnRequest {
                prompt: "jalankan".into(),
                session: "sh".into(),
                plan_mode: false,
            },
            decide_rx,
        );
        let mut key = None;
        while let Some(event) = stream.next().await {
            if let AgentEvent::ApprovalRequested { name, key: k, .. } = event {
                assert_eq!(name, "shell");
                key = Some(k);
                decide_tx.send(true).await.unwrap();
            }
        }
        assert_eq!(key.as_deref(), Some("shell"));
    }

    #[tokio::test]
    async fn outside_write_reject_is_reported() {
        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("x.txt");
        let call = tool_call_reply(
            "write",
            json!({ "path": target.to_string_lossy(), "content": "boom" }),
        );
        let final_text = sse(&[
            r#"{"choices":[{"delta":{"content":"tidak jadi"}}]}"#,
            "[DONE]",
        ]);
        let server = Server::http("127.0.0.1:0").unwrap();
        let addr = server.server_addr().to_ip().unwrap();
        std::thread::spawn(move || {
            for reply in [call, final_text] {
                let mut request = server.recv().unwrap();
                let mut body = String::new();
                request.as_reader().read_to_string(&mut body).unwrap();
                let _ = request.respond(Response::from_data(reply));
            }
        });
        let backend = backend(format!("http://{addr}/v1"), workspace.path().to_path_buf());
        let (decide_tx, decide_rx) = mpsc::channel(4);
        let mut stream = backend.run_turn(
            TurnRequest {
                prompt: "tulis luar".into(),
                session: "ow".into(),
                plan_mode: false,
            },
            decide_rx,
        );
        let mut rejected_seen = false;
        while let Some(event) = stream.next().await {
            if matches!(event, AgentEvent::ApprovalRequested { .. }) {
                decide_tx.send(false).await.unwrap();
            }
            if matches!(event, AgentEvent::ToolFinished { ok: false, detail, .. } if detail == "rejected by user")
            {
                rejected_seen = true;
            }
        }
        assert!(rejected_seen);
        assert!(!target.exists());
    }

    #[tokio::test]
    async fn plan_mode_blocks_mutating_tools_without_approval() {
        let dir = tempfile::tempdir().unwrap();
        let call = sse(&[
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"shell","arguments":"{\"command\":\"echo hi\"}"}}]}}]}"#,
            "[DONE]",
        ]);
        let final_text = sse(&[
            r#"{"choices":[{"delta":{"content":"encana matang"}}]}"#,
            "[DONE]",
        ]);
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
            TurnRequest {
                prompt: "jalankan".into(),
                session: "p".into(),
                plan_mode: true,
            },
            decide_rx,
        );
        let mut saw_block = false;
        let mut saw_approval = false;
        while let Some(event) = stream.next().await {
            if matches!(event, AgentEvent::ApprovalRequested { .. }) {
                saw_approval = true;
            }
            if matches!(event, AgentEvent::ToolFinished { detail, ok: false, .. } if detail.contains("plan mode"))
            {
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
            let mut request = server.recv().unwrap();
            let mut body = String::new();
            request.as_reader().read_to_string(&mut body).unwrap();
            let _ = request.respond(Response::from_string("kaboom").with_status_code(400));
        });
        let backend = backend(format!("http://{addr}/v1"), std::env::temp_dir());
        let events = collect(&backend, "halo").await;
        let Some(AgentEvent::Failed { message }) = events.last() else {
            panic!("expected Failed, got {events:?}");
        };
        assert!(message.contains("400"));
    }

    #[tokio::test]
    async fn retries_rate_limits_before_failing_over() {
        let (bodies, url) = fake_server_raw(vec![
            FakeReply::status(429, "slow down"),
            FakeReply::ok(sse(&[
                r#"{"choices":[{"delta":{"content":"halo"}}]}"#,
                "[DONE]",
            ])),
        ]);
        let backend = backend(url, std::env::temp_dir());
        let events = collect(&backend, "sapa").await;
        assert!(
            events.iter().any(|event| matches!(
                event,
                AgentEvent::Retrying { attempt: 1, max: 10, reason, .. } if reason.contains("429")
            )),
            "missing retry event: {events:?}"
        );
        assert!(events.iter().any(|event| matches!(
            event,
            AgentEvent::TextDelta { text } if text == "halo"
        )));
        assert_eq!(events.last(), Some(&AgentEvent::TurnFinished));
        assert_eq!(bodies.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn forbidden_error_is_not_retried_and_names_the_key_scope() {
        let (_bodies, url) = fake_server_raw(vec![FakeReply::status(
            403,
            r#"{"error":{"message":"This API key is not allowed to use model test-model"}}"#,
        )]);
        let backend = backend(url, std::env::temp_dir());
        let events = collect(&backend, "halo").await;
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, AgentEvent::Retrying { .. }))
        );
        let Some(AgentEvent::Failed { message }) = events.last() else {
            panic!("expected Failed, got {events:?}");
        };
        assert!(message.contains("403"));
        assert!(message.contains("test-model"), "{message}");
        assert!(message.contains("API Keys"), "{message}");
    }

    #[tokio::test]
    async fn budget_exhaustion_requests_a_toolless_summary() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "isi\n").unwrap();
        let (bodies, url) = fake_server(vec![
            tool_call_reply("read", json!({ "path": "a.txt" })),
            tool_call_reply("read", json!({ "path": "a.txt" })),
            sse(&[
                r#"{"choices":[{"delta":{"content":"ringkasan"}}]}"#,
                "[DONE]",
            ]),
        ]);
        let backend = backend_with(url, dir.path().to_path_buf(), 2);
        let events = collect(&backend, "loop").await;

        assert!(
            events
                .iter()
                .any(|event| matches!(event, AgentEvent::Iteration { n: 2, max: 2 }))
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(event, AgentEvent::Notice { .. }))
        );
        assert!(events.iter().any(|event| matches!(
            event,
            AgentEvent::TextDelta { text } if text == "ringkasan"
        )));
        assert_eq!(events.last(), Some(&AgentEvent::TurnFinished));

        let bodies = bodies.lock().unwrap();
        let summary = bodies.last().unwrap();
        assert!(
            !summary.contains("\"tools\":"),
            "summary request must not expose tools: {summary}"
        );
        assert!(summary.contains("maximum number of tool-calling iterations"));
    }

    #[tokio::test]
    async fn budget_summary_falls_back_and_still_finishes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "isi\n").unwrap();
        let (_bodies, url) = fake_server(vec![
            tool_call_reply("read", json!({ "path": "a.txt" })),
            tool_call_reply("read", json!({ "path": "a.txt" })),
            String::new(),
            String::new(),
        ]);
        let backend = backend_with(url, dir.path().to_path_buf(), 2);
        let events = collect(&backend, "loop").await;

        assert!(
            !events
                .iter()
                .any(|event| matches!(event, AgentEvent::Failed { .. }))
        );
        assert!(events.iter().any(|event| matches!(
            event,
            AgentEvent::TextDelta { text } if text.contains("Batas langkah")
        )));
        assert_eq!(events.last(), Some(&AgentEvent::TurnFinished));
    }

    #[tokio::test]
    async fn provider_error_keeps_completed_rounds_for_continue() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "isi rahasia\n").unwrap();
        let (bodies, url) = fake_server(vec![
            tool_call_reply("read", json!({ "path": "a.txt" })),
            sse(&[r#"{"error":{"message":"boom"}}"#]),
            sse(&[
                r#"{"choices":[{"delta":{"content":"lanjut ok"}}]}"#,
                "[DONE]",
            ]),
        ]);
        let backend = backend(url, dir.path().to_path_buf());

        let first = collect(&backend, "baca").await;
        assert!(
            matches!(first.last(), Some(AgentEvent::Failed { message }) if message.contains("boom"))
        );

        let _second = collect(&backend, "lanjut").await;
        let bodies = bodies.lock().unwrap();
        let second_turn = bodies.get(2).expect("second turn request");
        assert!(
            second_turn.contains("isi rahasia"),
            "completed tool round was not committed: {second_turn}"
        );
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
            TurnRequest {
                prompt: "pertanyaan pertama".into(),
                session: "same".into(),
                plan_mode: false,
            },
            rx,
        );
        while stream.next().await.is_some() {}
        let (tx, rx) = mpsc::channel(1);
        drop(tx);
        let mut stream = backend.run_turn(
            TurnRequest {
                prompt: "lanjutan".into(),
                session: "same".into(),
                plan_mode: false,
            },
            rx,
        );
        while stream.next().await.is_some() {}
        let second = bodies.lock().unwrap()[1].clone();
        assert!(
            second.contains("pertanyaan pertama"),
            "second body: {second}"
        );
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
        apply(
            &mut config,
            ConfigFile {
                base_url: Some("http://x:1/v1".into()),
                model: Some("m".into()),
                api_key: None,
                max_iterations: Some(3),
                mock: None,
            },
        );
        assert_eq!(config.base_url, "http://x:1/v1");
        assert_eq!(config.max_iterations, 3);
    }

    #[test]
    fn plan_mode_specs_are_read_only() {
        let specs = tool_specs(true);
        assert_eq!(specs.len(), 8);
        let names: Vec<&str> = specs
            .iter()
            .map(|s| s.pointer("/function/name").and_then(Value::as_str).unwrap())
            .collect();
        assert_eq!(
            names,
            [
                "read",
                "list",
                "grep",
                "web_fetch",
                "todo",
                "git",
                "ast_grep",
                "evrax-graph"
            ]
        );
    }

    #[test]
    fn history_commit_trims_oldest_turns() {
        let inner = Inner::for_test();
        let big = "x".repeat(MAX_HISTORY_CHARS / 2 + 10);
        for i in 0..3 {
            inner.commit(
                "s",
                vec![json!({"role":"user","content":format!("q{i} {big}")})],
            );
        }
        let replay = inner.replay("s");
        assert_eq!(replay.len(), 1, "only the newest turn survives");
        assert!(replay[0]["content"].as_str().unwrap().contains("q2"));
    }

    #[test]
    fn history_evicts_cold_sessions_beyond_cap() {
        let inner = Inner::for_test();
        for i in 0..=MAX_HISTORY_SESSIONS {
            inner.commit(
                &format!("s{i}"),
                vec![json!({"role":"user","content":"hi"})],
            );
        }
        let conversations = inner.conversations.lock().unwrap();
        assert_eq!(conversations.len(), MAX_HISTORY_SESSIONS);
        assert!(
            !conversations.contains_key("s0"),
            "the least recently used session should be gone"
        );
        assert!(conversations.contains_key(&format!("s{MAX_HISTORY_SESSIONS}")));
    }

    #[test]
    fn sanitize_title_strips_noise_and_caps() {
        assert_eq!(
            sanitize_title("\"Fix SSE heartbeat\"\n").as_deref(),
            Some("Fix SSE heartbeat")
        );
        assert_eq!(
            sanitize_title("# Re: refactor??").as_deref(),
            Some("Re: refactor")
        );
        assert_eq!(
            sanitize_title("a".repeat(90).as_str()).as_deref(),
            Some("a".repeat(60).as_str())
        );
        assert_eq!(sanitize_title("  ! "), None);
        assert_eq!(sanitize_title(""), None);
    }

    #[tokio::test]
    async fn suggest_title_parses_model_reply_and_survives_errors() {
        let good = json!({"choices":[{"message":{"content":"\"Fix SSE heartbeat\""}}]}).to_string();
        let (_bodies, url) = fake_server(vec![good]);
        let b1 = backend(url, std::env::temp_dir());
        let title = b1
            .suggest_title("perbaiki sse".into(), "sudah".into())
            .await;
        assert_eq!(title.as_deref(), Some("Fix SSE heartbeat"));

        let (_bodies, url) = fake_server(vec!["not json".into()]);
        let backend2 = backend(url, std::env::temp_dir());
        assert_eq!(backend2.suggest_title("a".into(), "b".into()).await, None);
    }
}
