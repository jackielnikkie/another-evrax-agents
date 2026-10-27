//! Real evrax backend: OpenAI-compatible SSE chat (9router) + agent loop.

pub mod anthropic;
pub mod catalog;
pub mod diff;
mod guardrails;
mod retry;
pub mod sse;
pub mod subagent;
pub mod tools;
pub mod update;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use evrax_core::{AgentBackend, AgentEvent, DiffLine, TurnRequest};
use futures_util::StreamExt;
use futures_util::future::BoxFuture;
use futures_util::stream::{self, BoxStream};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::sse::{Frame, SseDecoder, ToolCall, ToolCallAccumulator, parse_frame};

pub const DEFAULT_BASE_URL: &str = "http://127.0.0.1:20128/v1";
pub const DEFAULT_MODEL: &str = "bebas";
pub const DEFAULT_MAX_ITERATIONS: usize = 500;
const MAX_HISTORY_CHARS: usize = 100_000;
const MAX_HISTORY_SESSIONS: usize = 24;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    #[default]
    Openai,
    Anthropic,
}

impl ProviderKind {
    pub const ALL: [ProviderKind; 2] = [ProviderKind::Openai, ProviderKind::Anthropic];

    pub fn label(self) -> &'static str {
        match self {
            ProviderKind::Openai => "openai-compatible",
            ProviderKind::Anthropic => "anthropic",
        }
    }
}

/// One saved provider entry (name, protocol, endpoint, key, selected model).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Provider {
    pub name: String,
    #[serde(default)]
    pub kind: ProviderKind,
    pub base_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(default)]
    pub model: String,
}

impl Provider {
    pub fn new(name: impl Into<String>, base_url: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            kind: ProviderKind::Openai,
            base_url: base_url.into(),
            api_key: None,
            model: String::new(),
        }
    }

    pub fn clean_key(&self) -> Option<&str> {
        self.api_key
            .as_deref()
            .map(str::trim)
            .filter(|key| !key.is_empty())
    }
}

/// Resolved runtime config (the active provider, flattened for the backend).
#[derive(Clone, Debug, PartialEq)]
pub struct AgentConfig {
    pub kind: ProviderKind,
    pub provider: String,
    pub base_url: String,
    pub model: String,
    pub api_key: Option<String>,
    pub max_iterations: usize,
    pub root: PathBuf,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            kind: ProviderKind::Openai,
            provider: String::new(),
            base_url: DEFAULT_BASE_URL.to_string(),
            model: DEFAULT_MODEL.to_string(),
            api_key: None,
            max_iterations: DEFAULT_MAX_ITERATIONS,
            root: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        }
    }
}

impl AgentConfig {
    pub fn for_provider(provider: &Provider, max_iterations: usize) -> Self {
        Self {
            kind: provider.kind,
            provider: provider.name.clone(),
            base_url: provider.base_url.clone(),
            model: provider.model.clone(),
            api_key: provider.clean_key().map(str::to_string),
            max_iterations,
            root: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct ConfigFile {
    active: Option<String>,
    providers: Option<Vec<Provider>>,
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
    pub providers: Vec<Provider>,
    pub active: Option<String>,
    pub max_iterations: usize,
}

/// Case-insensitive provider lookup by name.
pub fn provider_index(providers: &[Provider], name: &str) -> Option<usize> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    providers
        .iter()
        .position(|provider| provider.name.eq_ignore_ascii_case(name))
}

fn provider_name_from_url(base_url: &str) -> String {
    if base_url.contains(":20128") {
        return "9router".to_string();
    }
    let host = base_url
        .split("://")
        .nth(1)
        .unwrap_or(base_url)
        .split('/')
        .next()
        .unwrap_or(base_url);
    let host = host.rsplit('@').next().unwrap_or(host);
    let host = host.split(':').next().unwrap_or(host);
    let host = host.trim_start_matches("api.").trim_start_matches("www.");
    if host.is_empty() {
        "default".to_string()
    } else {
        host.to_string()
    }
}

pub fn load_config() -> LoadedConfig {
    let mut mock = false;
    let mut max_iterations = DEFAULT_MAX_ITERATIONS;
    let mut providers: Vec<Provider> = Vec::new();
    let mut active: Option<String> = None;
    if let Some(path) = config_file_path()
        && let Ok(text) = std::fs::read_to_string(path)
        && let Ok(file) = serde_json::from_str::<ConfigFile>(&text)
    {
        (mock, max_iterations, providers, active) = file_state(file);
    }
    if let Ok(v) = std::env::var("EVRAX_PROVIDER") {
        active = Some(v);
    }
    if std::env::var("EVRAX_MOCK").is_ok_and(|v| v == "1" || v == "true") {
        mock = true;
    }
    let (mut config, active) = resolve(&providers, active.as_deref(), max_iterations);
    apply_env(&mut config);
    max_iterations = config.max_iterations;
    LoadedConfig {
        mock,
        config,
        providers,
        active,
        max_iterations,
    }
}

fn file_state(file: ConfigFile) -> (bool, usize, Vec<Provider>, Option<String>) {
    let ConfigFile {
        active,
        providers,
        base_url,
        model,
        api_key,
        max_iterations,
        mock,
    } = file;
    let mut providers = providers.unwrap_or_default();
    // Legacy flat file: synthesize a single provider, then save rewrites it.
    if providers.is_empty() && (base_url.is_some() || model.is_some() || api_key.is_some()) {
        let base_url = base_url.unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
        providers.push(Provider {
            name: provider_name_from_url(&base_url),
            kind: ProviderKind::Openai,
            base_url,
            api_key,
            model: model.unwrap_or_else(|| DEFAULT_MODEL.to_string()),
        });
    }
    (
        mock.unwrap_or(false),
        max_iterations.unwrap_or(DEFAULT_MAX_ITERATIONS),
        providers,
        active.filter(|name| !name.trim().is_empty()),
    )
}

/// Picks the active provider (missing/blank name falls back to the first) and
/// flattens it into runtime config.
fn resolve(
    providers: &[Provider],
    active: Option<&str>,
    max_iterations: usize,
) -> (AgentConfig, Option<String>) {
    let selected = active
        .and_then(|name| provider_index(providers, name))
        .or(if providers.is_empty() { None } else { Some(0) });
    let config = match selected {
        Some(index) => AgentConfig::for_provider(&providers[index], max_iterations),
        None => AgentConfig {
            max_iterations,
            ..AgentConfig::default()
        },
    };
    let active = selected.map(|index| providers[index].name.clone());
    (config, active)
}

fn apply_env(config: &mut AgentConfig) {
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
}

/// Parse `--mock`, `--provider <name>`, `--model <v>`, `--base-url <v>`,
/// `--api-key <v>`, `--max-iterations <v>`, `--root <v>` on top of env/file
/// config. Provider/URL/key/model overrides are runtime-only (never saved).
pub fn config_from_args() -> LoadedConfig {
    let mut loaded = load_config();
    let mut provider: Option<String> = None;
    let mut base_url: Option<String> = None;
    let mut model: Option<String> = None;
    let mut api_key: Option<String> = None;
    let mut max_iterations: Option<usize> = None;
    let mut root: Option<PathBuf> = None;
    let mut mock = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--mock" => mock = true,
            "--provider" => provider = args.next(),
            "--model" => model = args.next(),
            "--base-url" | "--base_url" => base_url = args.next(),
            "--api-key" | "--api_key" => api_key = args.next(),
            "--max-iterations" => {
                max_iterations = args.next().and_then(|v| v.parse().ok());
            }
            "--root" => root = args.next().map(PathBuf::from),
            _ => {}
        }
    }
    loaded.mock |= mock;
    if let Some(name) = provider
        && let Some(index) = provider_index(&loaded.providers, &name)
    {
        loaded.config = AgentConfig::for_provider(&loaded.providers[index], loaded.max_iterations);
        apply_env(&mut loaded.config);
        loaded.active = Some(loaded.providers[index].name.clone());
    }
    if let Some(v) = base_url {
        loaded.config.base_url = v;
    }
    if let Some(v) = model {
        loaded.config.model = v;
    }
    if let Some(v) = api_key {
        loaded.config.api_key = Some(v);
    }
    if let Some(v) = max_iterations {
        loaded.config.max_iterations = v;
    }
    if let Some(v) = root {
        loaded.config.root = v;
    }
    loaded
}

pub fn config_file_path() -> Option<PathBuf> {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .map(|dir| dir.join("evrax").join("agent.json"))
}

/// Everything the wizard / `/model` dialog can change and persist.
#[derive(Clone, Debug, PartialEq)]
pub struct ConfigSettings {
    pub providers: Vec<Provider>,
    pub active: String,
    pub max_iterations: usize,
    pub mock: bool,
}

impl ConfigSettings {
    pub fn active_index(&self) -> Option<usize> {
        provider_index(&self.providers, &self.active)
    }

    /// Resolved runtime config of the active provider.
    pub fn resolved(&self) -> AgentConfig {
        let mut config = match self.active_index() {
            Some(index) => AgentConfig::for_provider(&self.providers[index], self.max_iterations),
            None => AgentConfig {
                max_iterations: self.max_iterations,
                ..AgentConfig::default()
            },
        };
        apply_env(&mut config);
        config
    }
}

/// Persists provider settings, preserving unrelated fields and backing up the
/// previous file as `agent.json.bak-<ms>`. Rewrites any legacy flat keys.
pub fn save_config(settings: &ConfigSettings) -> anyhow::Result<PathBuf> {
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
    // Drop legacy flat keys so agent.json has exactly one shape.
    for key in ["base_url", "model", "api_key", "active", "providers"] {
        object.remove(key);
    }
    object.insert(
        "active".into(),
        Value::String(settings.active.trim().to_string()),
    );
    object.insert(
        "providers".into(),
        serde_json::to_value(&settings.providers)?,
    );
    object.insert(
        "max_iterations".into(),
        Value::Number(settings.max_iterations.into()),
    );
    object.insert("mock".into(), Value::Bool(settings.mock));
    std::fs::write(&path, serde_json::to_string_pretty(&value)?)?;
    Ok(path)
}

fn http_client(base_url: &str) -> Result<reqwest::Client, String> {
    let mut builder = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .read_timeout(Duration::from_secs(30));
    let lower = base_url.to_lowercase();
    if lower.contains("127.0.0.1") || lower.contains("localhost") || lower.contains("[::1]") {
        builder = builder.no_proxy();
    }
    builder.build().map_err(|e| e.to_string())
}

/// Attaches credentials in the way the provider's protocol expects.
fn auth_request(
    request: reqwest::RequestBuilder,
    kind: ProviderKind,
    key: Option<&str>,
) -> reqwest::RequestBuilder {
    let key = key.filter(|key| !key.is_empty());
    match kind {
        ProviderKind::Openai => match key {
            Some(key) => request.bearer_auth(key),
            None => request,
        },
        ProviderKind::Anthropic => {
            let request = request.header("anthropic-version", anthropic::API_VERSION);
            match key {
                Some(key) => request.header("x-api-key", key),
                None => request,
            }
        }
    }
}

/// One-shot provider check used by the setup wizard: a tiny chat request.
pub async fn test_provider(cfg: &AgentConfig) -> Result<String, String> {
    let client = http_client(&cfg.base_url)?;
    let is_anthropic = cfg.kind == ProviderKind::Anthropic;
    let prompt = json!([{ "role": "user", "content": "Reply with the single word OK." }]);
    let (url, body) = if is_anthropic {
        (
            anthropic::messages_url(&cfg.base_url),
            json!({
                "model": cfg.model,
                "max_tokens": 16,
                "messages": prompt,
                "stream": false,
            }),
        )
    } else {
        (
            format!("{}/chat/completions", cfg.base_url.trim_end_matches('/')),
            json!({
                "model": cfg.model,
                "messages": prompt,
                "max_tokens": 16,
                "temperature": 0,
                "stream": false,
            }),
        )
    };
    let request = auth_request(
        client.post(url).json(&body),
        cfg.kind,
        cfg.api_key.as_deref(),
    );
    let response = request
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;
    let status = response.status();
    let text = response.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        let detail = if is_anthropic {
            anthropic::error_message(&text)
        } else {
            text.clone()
        };
        return Err(format!(
            "HTTP {}: {}",
            status.as_u16(),
            trunc(&detail, 240).replace('\n', " ")
        ));
    }
    let value: Value = serde_json::from_str(&text).map_err(|e| format!("bad JSON: {e}"))?;
    let reply = if is_anthropic {
        anthropic::reply_text(&value).unwrap_or_default()
    } else {
        value["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("")
            .to_string()
    };
    let reply = reply.trim().to_string();
    if reply.is_empty() {
        Ok(format!("HTTP {} OK", status.as_u16()))
    } else {
        Ok(reply)
    }
}

/// Lists the models a provider offers (`GET {base}/models`). This doubles as
/// the key check: no model is required, unlike `test_provider`.
pub async fn list_models(
    kind: ProviderKind,
    base_url: &str,
    api_key: Option<&str>,
) -> Result<Vec<String>, String> {
    let client = http_client(base_url)?;
    let base = base_url.trim_end_matches('/');
    if kind == ProviderKind::Anthropic {
        let mut models: Vec<String> = Vec::new();
        let mut after_id: Option<String> = None;
        for _ in 0..5 {
            let request = auth_request(client.get(format!("{base}/models")), kind, api_key);
            let request = match &after_id {
                Some(id) => request.query(&[("after_id", id.as_str()), ("limit", "1000")]),
                None => request.query(&[("limit", "1000")]),
            };
            let (status, text) = send_text(request).await?;
            if !status.is_success() {
                return Err(models_error(status.as_u16(), &text, kind));
            }
            let value: Value = serde_json::from_str(&text).map_err(|e| format!("bad JSON: {e}"))?;
            models.extend(
                value
                    .get("data")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|item| item.get("id").and_then(Value::as_str).map(str::to_string)),
            );
            if !value
                .get("has_more")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                break;
            }
            match value.get("last_id").and_then(Value::as_str) {
                Some(last) if last != after_id.as_deref().unwrap_or("") => {
                    after_id = Some(last.to_string());
                }
                _ => break,
            }
        }
        Ok(finalize_models(models))
    } else {
        let request = auth_request(client.get(format!("{base}/models")), kind, api_key);
        let (status, text) = send_text(request).await?;
        if !status.is_success() {
            return Err(models_error(status.as_u16(), &text, kind));
        }
        let value: Value = serde_json::from_str(&text).map_err(|e| format!("bad JSON: {e}"))?;
        Ok(finalize_models(parse_openai_models(&value)?))
    }
}

async fn send_text(
    request: reqwest::RequestBuilder,
) -> Result<(reqwest::StatusCode, String), String> {
    let response = request
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;
    let status = response.status();
    let text = response.text().await.map_err(|e| e.to_string())?;
    Ok((status, text))
}

/// Tolerates the shapes gateways use: `{"data":[{"id"}]}`,
/// `{"models":[{"id"|"name"}]}` and bare arrays.
fn parse_openai_models(value: &Value) -> Result<Vec<String>, String> {
    let items = value
        .as_array()
        .or_else(|| value.get("data").and_then(Value::as_array))
        .or_else(|| value.get("models").and_then(Value::as_array))
        .ok_or("/models response not recognized")?;
    Ok(items
        .iter()
        .filter_map(|item| {
            item.get("id")
                .and_then(Value::as_str)
                .or_else(|| item.get("name").and_then(Value::as_str))
                .map(str::to_string)
        })
        .collect())
}

fn finalize_models(mut models: Vec<String>) -> Vec<String> {
    models.retain(|model| !model.trim().is_empty());
    models.sort_by_key(|model| model.to_lowercase());
    models.dedup();
    models.truncate(1000);
    models
}

fn models_error(status: u16, body: &str, kind: ProviderKind) -> String {
    let raw = if kind == ProviderKind::Anthropic {
        anthropic::error_message(body)
    } else {
        body.trim().to_string()
    };
    let detail = trunc(&raw.replace('\n', " "), 240);
    let hint = match status {
        401 | 403 => "API key rejected - check api_key",
        404 | 405 | 501 => "this endpoint does not serve /models - enter the model manually",
        _ => "",
    };
    if hint.is_empty() {
        format!("HTTP {status}: {detail}")
    } else {
        format!("HTTP {status}: {hint}. {detail}")
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
                root: std::env::temp_dir(),
                ..AgentConfig::default()
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
            let user = format!(
                "PROMPT:\n{}\n\nREPLY:\n{}",
                trunc(&prompt, 1200),
                trunc(&reply, 1200)
            );
            let is_anthropic = inner.cfg.kind == ProviderKind::Anthropic;
            let (url, body) = if is_anthropic {
                (
                    anthropic::messages_url(&inner.cfg.base_url),
                    json!({
                        "model": inner.cfg.model,
                        "system": TITLE_SYSTEM,
                        "messages": [{"role": "user", "content": user}],
                        "stream": false,
                        "max_tokens": 32,
                        "temperature": 0.2
                    }),
                )
            } else {
                (
                    format!(
                        "{}/chat/completions",
                        inner.cfg.base_url.trim_end_matches('/')
                    ),
                    json!({
                        "model": inner.cfg.model,
                        "messages": [
                            {"role": "system", "content": TITLE_SYSTEM},
                            {"role": "user", "content": user}
                        ],
                        "stream": false,
                        "max_tokens": 16,
                        "temperature": 0.2
                    }),
                )
            };
            let call = async {
                let request = auth_request(
                    inner.client.post(&url).json(&body),
                    inner.cfg.kind,
                    inner.cfg.api_key.as_deref(),
                );
                let response = request.send().await.ok()?;
                if !response.status().is_success() {
                    return None;
                }
                let value: Value = response.json().await.ok()?;
                let text = if is_anthropic {
                    anthropic::reply_text(&value)?
                } else {
                    value
                        .pointer("/choices/0/message/content")?
                        .as_str()?
                        .to_string()
                };
                sanitize_title(&text)
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
        .map_err(|_| "turn cancelled".to_string())
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
            message: format!("tool iteration budget reached ({max}/{max}) - requesting a summary"),
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
            Ok(_) => last_error = Some("empty response".to_string()),
            Err(error) => last_error = Some(error),
        }
        if attempt == 0 {
            emit(
                tx,
                AgentEvent::Notice {
                    message: "summary failed, retrying once".into(),
                },
            )
            .await?;
        }
    }

    if text.trim().is_empty() {
        let detail = last_error.unwrap_or_else(|| "empty response".into());
        text = format!(
            "This turn hit its step budget ({max} tool calls). Send `continue` to keep going, or raise `max_iterations` in agent.json."
        );
        emit(
            tx,
            AgentEvent::Notice {
                message: format!(
                    "summary unavailable ({}) - using the fallback message",
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
        // The call is fully formed but held for approval: show it as still
        // "preparing" so the transcript does not read as a running tool.
        emit(
            tx,
            AgentEvent::ToolPreparing {
                name: name.to_string(),
                bytes: args.as_ref().map(|v| v.to_string().len()).unwrap_or(0),
            },
        )
        .await?;
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

fn provider_error(
    kind: ProviderKind,
    base_url: &str,
    status: reqwest::StatusCode,
    body: &str,
    model: &str,
) -> String {
    let detail = if kind == ProviderKind::Anthropic {
        trunc(&anthropic::error_message(body), 400)
    } else {
        trunc(body.trim(), 400)
    };
    let lower = base_url.to_lowercase();
    let is_nine_router = base_url.contains("20128") || lower.contains("9router");
    let hint = match status.as_u16() {
        401 if is_nine_router => {
            "9router rejected the API key (invalid/inactive) - check `api_key` in agent.json and the key status in the 9router dashboard"
        }
        401 => "provider rejected the API key - check `api_key` in agent.json",
        402 => "kuota/kredit provider habis",
        403 if is_nine_router => {
            "9router denied this key for that model - open the 9router dashboard  API Keys, allow the model or disable restrict access"
        }
        403 => {
            "provider denied this key for that model - check key/model permissions in the provider dashboard"
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

/// Throttle for `AgentEvent::ToolPreparing`: one event per 4 KB of arguments
/// per 200 ms window.
const PREP_STEP_BYTES: usize = 4 * 1024;
const PREP_INTERVAL: Duration = Duration::from_millis(200);

/// Per-call argument streaming progress, so the UI can show "preparing …".
#[derive(Default)]
struct Prep {
    name: String,
    bytes: usize,
    emitted: usize,
    at: Option<Instant>,
}

struct StreamState {
    text: String,
    thinking: Option<(String, Instant)>,
    acc: ToolCallAccumulator,
    prep: HashMap<usize, Prep>,
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
                let index = call.index;
                let progress = {
                    let prep = state.prep.entry(index).or_default();
                    if let Some(name) = &call.name {
                        prep.name = name.clone();
                    }
                    if let Some(args) = &call.arguments {
                        prep.bytes += args.len();
                    }
                    let due = prep.bytes.saturating_sub(prep.emitted) >= PREP_STEP_BYTES
                        || prep.at.is_none_or(|at| at.elapsed() >= PREP_INTERVAL);
                    if due && !prep.name.is_empty() {
                        prep.emitted = prep.bytes;
                        prep.at = Some(Instant::now());
                        Some((prep.name.clone(), prep.bytes))
                    } else {
                        None
                    }
                };
                state.acc.apply(call);
                if let Some((name, bytes)) = progress {
                    emit(tx, AgentEvent::ToolPreparing { name, bytes }).await?;
                }
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
    let anthropic = inner.cfg.kind == ProviderKind::Anthropic;
    let (url, body) = if anthropic {
        let (system, converted) = anthropic::convert_messages(messages);
        let tools = anthropic::convert_tools(specs);
        (
            anthropic::messages_url(&inner.cfg.base_url),
            anthropic::request_body(&inner.cfg.model, &system, &converted, &tools, true),
        )
    } else {
        let url = format!(
            "{}/chat/completions",
            inner.cfg.base_url.trim_end_matches('/')
        );
        let mut body = json!({ "model": inner.cfg.model, "messages": messages, "stream": true });
        if !specs.is_empty() {
            body["tools"] = Value::Array(specs.to_vec());
        }
        (url, body)
    };
    let mut retry_state = retry::RetryState::default();
    let response = loop {
        let request = auth_request(
            inner.client.post(&url).json(&body),
            inner.cfg.kind,
            inner.cfg.api_key.as_deref(),
        );
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
                return Err(provider_error(
                    inner.cfg.kind,
                    &inner.cfg.base_url,
                    status,
                    &text,
                    &inner.cfg.model,
                ));
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
                return Err(format!("failed to reach {}: {error}", inner.cfg.base_url));
            }
        }
    };

    let mut stream = response.bytes_stream();
    let mut decoder = SseDecoder::new();
    if anthropic {
        let mut acc = anthropic::StreamAcc::default();
        let mut thinking_started: Option<Instant> = None;
        let mut finished = false;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| format!("stream interrupted: {e}"))?;
            for payload in decoder.push(&chunk) {
                if handle_anthropic_payload(&payload, &mut acc, &mut thinking_started, tx).await? {
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
                if handle_anthropic_payload(&payload, &mut acc, &mut thinking_started, tx).await? {
                    break;
                }
            }
        }
        return finish_anthropic_round(acc, thinking_started, tx).await;
    }
    let mut state = StreamState {
        text: String::new(),
        thinking: None,
        acc: ToolCallAccumulator::default(),
        prep: HashMap::new(),
    };

    let mut finished = false;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("stream interrupted: {e}"))?;
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

async fn handle_anthropic_payload(
    payload: &str,
    acc: &mut anthropic::StreamAcc,
    thinking_started: &mut Option<Instant>,
    tx: &mpsc::Sender<AgentEvent>,
) -> Result<bool, String> {
    let (text, thinking) = anthropic::apply_event(acc, payload);
    if !thinking.is_empty() {
        thinking_started.get_or_insert_with(Instant::now);
        emit(tx, AgentEvent::ThinkingDelta { text: thinking }).await?;
    }
    if !text.is_empty() {
        emit(tx, AgentEvent::TextDelta { text }).await?;
    }
    if let Some(error) = acc.error.take() {
        return Err(format!("provider error: {error}"));
    }
    Ok(acc.done)
}

async fn finish_anthropic_round(
    acc: anthropic::StreamAcc,
    thinking_started: Option<Instant>,
    tx: &mpsc::Sender<AgentEvent>,
) -> Result<Round, String> {
    if let Some(started) = thinking_started {
        emit(
            tx,
            AgentEvent::ThinkingFinished {
                seconds: started.elapsed().as_secs_f32(),
            },
        )
        .await?;
    }
    if acc.calls.is_empty() && acc.text.trim().is_empty() {
        return Err("model returned an empty response".into());
    }
    Ok(Round {
        text: acc.text,
        calls: acc.calls,
    })
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
        return Err("model returned an empty response".into());
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
         Keep replies concise. When the task is done, answer with plain text and no tool calls.\n\
         Write your reply in the language the user writes in: Indonesian replies to Indonesian, English to English; never switch languages on your own. Keep code, identifiers, file paths, commands and error strings in their original form, and keep strings that render inside the evrax UI in English.",
        cfg.root.display(),
        std::env::consts::OS
    );
    if plan_mode {
        prompt.push_str("\nPLAN MODE: read-only and workspace-only. Read-only web_fetch, todo list, git status/diff/log/show, ast_grep search, and evrax-graph queries/index are allowed; never attempt write/edit/shell or mutating todo/git/ast-grep/subagent actions, and never read files outside the workspace; investigate and present a plan.");
    }
    prompt
}

#[cfg(test)]
mod language_tests {
    use super::*;

    fn cfg() -> AgentConfig {
        AgentConfig {
            root: std::env::temp_dir(),
            ..AgentConfig::default()
        }
    }

    #[test]
    fn system_prompt_matches_user_language_and_protects_english_ui() {
        let prompt = system_prompt(&cfg(), false);
        assert!(
            prompt.contains("Indonesian replies to Indonesian, English to English"),
            "language rule missing:\n{prompt}"
        );
        assert!(
            prompt.contains("keep strings that render inside the evrax UI in English"),
            "UI-language rule missing:\n{prompt}"
        );
        assert!(prompt.contains("You are evrax"));
        assert!(!prompt.contains("PLAN MODE"));
    }

    #[test]
    fn system_prompt_includes_plan_mode_caveat() {
        let prompt = system_prompt(&cfg(), true);
        assert!(prompt.contains("PLAN MODE"));
        assert!(prompt.contains("never attempt write/edit/shell"));
    }
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
            max_iterations,
            root,
            ..AgentConfig::default()
        })
        .unwrap()
    }

    fn backend(base_url: String, root: PathBuf) -> ChatBackend {
        backend_with(base_url, root, 4)
    }

    fn backend_kind(kind: ProviderKind, base_url: String) -> ChatBackend {
        ChatBackend::new(AgentConfig {
            kind,
            base_url,
            root: std::env::temp_dir(),
            ..AgentConfig::default()
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
        assert!(message.contains("check key/model permissions"), "{message}");
    }

    #[test]
    fn provider_error_keeps_9router_hint_for_its_base_url() {
        let message = provider_error(
            ProviderKind::Openai,
            "http://127.0.0.1:20128/v1",
            reqwest::StatusCode::FORBIDDEN,
            "denied",
            "m",
        );
        assert!(message.contains("9router dashboard"), "{message}");
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
            AgentEvent::TextDelta { text } if text.contains("step budget")
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
    fn legacy_flat_config_migrates_to_one_provider() {
        let file: ConfigFile = serde_json::from_str(
            r#"{"base_url":"http://127.0.0.1:20128/v1","model":"bebas",
                "api_key":"k","max_iterations":7}"#,
        )
        .unwrap();
        let (mock, max_iterations, providers, active) = file_state(file);
        assert!(!mock);
        assert_eq!(max_iterations, 7);
        assert_eq!(active, None);
        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].name, "9router");
        assert_eq!(providers[0].kind, ProviderKind::Openai);
        assert_eq!(providers[0].model, "bebas");
        assert_eq!(providers[0].clean_key(), Some("k"));
    }

    #[test]
    fn providers_file_resolves_active_case_insensitively_and_falls_back() {
        let file: ConfigFile = serde_json::from_str(
            r#"{"active":"muse spark","providers":[
                {"name":"Nimbus Go","base_url":"http://a/v1","model":"m1"},
                {"name":"Muse Spark","kind":"anthropic","base_url":"http://b/v1",
                 "api_key":"k","model":"m2"}]}"#,
        )
        .unwrap();
        let (_, max_iterations, providers, active) = file_state(file);
        assert_eq!(max_iterations, DEFAULT_MAX_ITERATIONS);
        assert_eq!(active.as_deref(), Some("muse spark"));
        let (config, active) = resolve(&providers, active.as_deref(), max_iterations);
        assert_eq!(active.as_deref(), Some("Muse Spark"));
        assert_eq!(config.kind, ProviderKind::Anthropic);
        assert_eq!(config.provider, "Muse Spark");
        assert_eq!(config.model, "m2");
        assert_eq!(config.api_key.as_deref(), Some("k"));

        let (config, active) = resolve(&providers, Some("nope"), max_iterations);
        assert_eq!(active.as_deref(), Some("Nimbus Go"));
        assert_eq!(config.base_url, "http://a/v1");
    }

    #[test]
    fn provider_settings_roundtrip_through_json() {
        let settings = ConfigSettings {
            providers: vec![
                Provider::new("Nimbus Go", "http://a/v1"),
                Provider {
                    name: "Muse Spark".into(),
                    kind: ProviderKind::Anthropic,
                    base_url: "http://b/v1".into(),
                    api_key: Some("secret".into()),
                    model: "m2".into(),
                },
            ],
            active: "Muse Spark".into(),
            max_iterations: 42,
            mock: false,
        };
        let text = serde_json::to_string(&settings.providers).unwrap();
        let back: Vec<Provider> = serde_json::from_str(&text).unwrap();
        assert_eq!(back, settings.providers);
        let resolved = settings.resolved();
        assert_eq!(resolved.provider, "Muse Spark");
        assert_eq!(resolved.kind, ProviderKind::Anthropic);
        assert_eq!(resolved.max_iterations, 42);
        assert_eq!(settings.active_index(), Some(1));
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

    #[tokio::test]
    async fn anthropic_stream_uses_messages_api_and_emits_text() {
        let body = sse(&[
            r#"{"type":"message_start","message":{}}"#,
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"halo"}}"#,
            r#"{"type":"content_block_stop","index":0}"#,
            r#"{"type":"message_stop"}"#,
        ]);
        let (bodies, url) = fake_server(vec![body]);
        let backend = backend_kind(ProviderKind::Anthropic, url);
        let events = collect(&backend, "sapa").await;
        assert!(
            events
                .iter()
                .any(|e| matches!(e, AgentEvent::TextDelta { text } if text == "halo")),
            "{events:?}"
        );
        assert!(matches!(events.last(), Some(AgentEvent::TurnFinished)));
        let sent: Value = serde_json::from_str(&bodies.lock().unwrap()[0]).unwrap();
        assert_eq!(sent["model"], DEFAULT_MODEL);
        assert_eq!(sent["stream"], true);
        assert!(sent["system"].as_str().unwrap().contains("evrax"));
        assert!(
            sent["tools"]
                .as_array()
                .is_some_and(|tools| !tools.is_empty())
        );
        assert_eq!(sent["messages"][0]["role"], "user");
        assert!(sent.get("max_tokens").is_some());
    }

    #[tokio::test]
    async fn anthropic_stream_collects_tool_calls() {
        let tool = sse(&[
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_1","name":"list"}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\".\"}"}}"#,
            r#"{"type":"content_block_stop","index":0}"#,
            r#"{"type":"message_stop"}"#,
        ]);
        let done = sse(&[
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"selesai"}}"#,
            r#"{"type":"content_block_stop","index":0}"#,
            r#"{"type":"message_stop"}"#,
        ]);
        let (bodies, url) = fake_server(vec![tool, done]);
        let backend = backend_kind(ProviderKind::Anthropic, url);
        let events = collect(&backend, "list files").await;
        assert!(
            events.iter().any(|e| matches!(
                e,
                AgentEvent::ToolStarted { name, .. } if name == "list"
            )),
            "{events:?}"
        );
        assert!(matches!(events.last(), Some(AgentEvent::TurnFinished)));
        // The follow-up request must carry the tool_use + tool_result blocks.
        let second: Value = serde_json::from_str(&bodies.lock().unwrap()[1]).unwrap();
        let messages = second["messages"].as_array().unwrap();
        let blocks: Vec<&Value> = messages
            .iter()
            .filter_map(|message| message["content"].as_array())
            .flatten()
            .collect();
        assert!(
            blocks
                .iter()
                .any(|block| block["type"] == "tool_use" && block["name"] == "list"),
            "{messages:?}"
        );
        assert!(
            blocks
                .iter()
                .any(|block| block["type"] == "tool_result" && block["tool_use_id"] == "toolu_1"),
            "{messages:?}"
        );
    }

    #[tokio::test]
    async fn list_models_parses_gateway_shapes_and_errors() {
        let (_, url) = fake_server(vec![
            r#"{"data":[{"id":"b"},{"id":"a"},{"id":"a"}]}"#.into(),
        ]);
        let models = list_models(ProviderKind::Openai, &url, Some("k"))
            .await
            .unwrap();
        assert_eq!(models, ["a", "b"]);

        let (_, url) = fake_server(vec![r#"{"models":[{"name":"llama3"}]}"#.into()]);
        let models = list_models(ProviderKind::Openai, &url, None).await.unwrap();
        assert_eq!(models, ["llama3"]);

        let (_, url) = fake_server(vec![
            r#"{"data":[{"id":"claude-b"},{"id":"claude-a"}],"has_more":false}"#.into(),
        ]);
        let models = list_models(ProviderKind::Anthropic, &url, Some("k"))
            .await
            .unwrap();
        assert_eq!(models, ["claude-a", "claude-b"]);

        let (_, url) = fake_server_raw(vec![FakeReply::status(
            401,
            r#"{"error":{"message":"bad key"}}"#,
        )]);
        let err = list_models(ProviderKind::Anthropic, &url, Some("k"))
            .await
            .unwrap_err();
        assert!(err.contains("API key rejected"), "{err}");

        let (_, url) = fake_server_raw(vec![FakeReply::status(404, "not found")]);
        let err = list_models(ProviderKind::Openai, &url, Some("k"))
            .await
            .unwrap_err();
        assert!(err.contains("enter the model manually"), "{err}");
    }

    #[tokio::test]
    async fn streamed_tool_arguments_emit_throttled_preparing() {
        let dir = tempfile::tempdir().unwrap();
        let pad = "x".repeat(200);
        let mut frames = vec![
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"list","arguments":"{\"pattern\":\""}}]}}]}"#
                .to_string(),
        ];
        for _ in 0..40 {
            frames.push(format!(
                r#"{{"choices":[{{"delta":{{"tool_calls":[{{"index":0,"function":{{"arguments":"{pad}"}}}}]}}}}]}}"#
            ));
        }
        frames.push(
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"}"}}]}}]}"#
                .to_string(),
        );
        frames.push("[DONE]".to_string());
        let refs: Vec<&str> = frames.iter().map(String::as_str).collect();
        let call = sse(&refs);
        let final_text = sse(&[r#"{"choices":[{"delta":{"content":"ok"}}]}"#, "[DONE]"]);
        let server = Server::http("127.0.0.1:0").unwrap();
        let addr = server.server_addr().to_ip().unwrap();
        std::thread::spawn(move || {
            for reply in [call, final_text] {
                let mut request = server.recv().unwrap();
                let mut sink = String::new();
                request.as_reader().read_to_string(&mut sink).unwrap();
                let _ = request.respond(Response::from_data(reply));
            }
        });
        let backend = backend(format!("http://{addr}/v1"), dir.path().to_path_buf());
        let (_, decide_rx) = mpsc::channel(1);
        let mut stream = backend.run_turn(
            TurnRequest {
                prompt: "stream args".into(),
                session: "prep".into(),
                plan_mode: false,
            },
            decide_rx,
        );
        let mut prepares = Vec::new();
        let mut saw_started = false;
        while let Some(event) = stream.next().await {
            match event {
                AgentEvent::ToolPreparing { name, bytes } => {
                    assert!(!saw_started, "preparing must arrive before ToolStarted");
                    assert_eq!(name, "list");
                    prepares.push(bytes);
                }
                AgentEvent::ToolStarted { .. } => saw_started = true,
                _ => {}
            }
        }
        assert!(!prepares.is_empty(), "no ToolPreparing event was emitted");
        assert!(
            prepares.iter().any(|bytes| *bytes >= 4096),
            "the 4 KB throttle step was never reached: {prepares:?}"
        );
        assert!(
            prepares.len() <= 4,
            "preparing is not throttled: {prepares:?}"
        );
    }

    #[tokio::test]
    #[ignore = "butuh provider live + %APPDATA%\\evrax\\agent.json"]
    async fn live_list_models_returns_something() {
        let loaded = load_config();
        let models = list_models(
            loaded.config.kind,
            &loaded.config.base_url,
            loaded.config.api_key.as_deref(),
        )
        .await
        .expect("list models");
        assert!(!models.is_empty(), "provider returned no models");
    }
}
