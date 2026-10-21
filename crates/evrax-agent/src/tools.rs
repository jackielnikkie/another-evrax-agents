//! Filesystem + shell tools exposed to the model.

mod ast_grep;
mod git;
mod graph;
mod todo;
mod web;

use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use evrax_core::{DiffKind, DiffLine};
use serde_json::{Value, json};
use tokio::process::Command;

use crate::diff::diff_lines;

pub const MAX_OUTPUT_LINES: usize = 400;
pub const MAX_OUTPUT_BYTES: usize = 30 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Risk {
    Read,
    Mutate,
}

#[derive(Debug)]
pub struct ToolOutcome {
    pub ok: bool,
    pub detail: String,
    pub diff: Option<(String, Vec<DiffLine>)>,
}

pub struct Context {
    pub root: PathBuf,
}

/// A resolved model-supplied path plus whether it leaves the workspace.
pub struct Resolved {
    pub path: PathBuf,
    pub outside: bool,
}

impl Context {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Resolve a model-supplied path and classify whether it leaves the
    /// workspace. Escapes are allowed (approval is handled by the caller).
    fn classify(&self, raw: &str) -> Result<Resolved, String> {
        let cleaned = raw.trim().trim_matches('"');
        if cleaned.is_empty() {
            return Err("empty path".into());
        }
        let candidate = Path::new(cleaned);
        let joined = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            self.root.join(candidate)
        };
        let mut out = PathBuf::new();
        for component in joined.components() {
            match component {
                Component::ParentDir => {
                    out.pop();
                }
                Component::CurDir => {}
                other => out.push(other.as_os_str()),
            }
        }
        let root = canonical(&self.root);
        let inside = out.starts_with(&self.root) || out.starts_with(&root);
        Ok(Resolved {
            path: out,
            outside: !inside,
        })
    }

    fn display(&self, path: &Path) -> String {
        path.strip_prefix(&self.root)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| path.to_string_lossy().to_string())
    }
}

fn canonical(path: &Path) -> PathBuf {
    let resolved = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    strip_verbatim(&resolved)
}

/// Drop the Windows extended-length (`\\?\`) prefix so approval keys stay readable.
fn strip_verbatim(path: &Path) -> PathBuf {
    match path.to_string_lossy().strip_prefix(r"\\?\") {
        Some(rest) => PathBuf::from(rest),
        None => path.to_path_buf(),
    }
}

/// First workspace-external target for a file tool, used to decide approval.
/// Returns `None` for tools with no path argument (e.g. `grep`, `shell`) or
/// when every candidate path stays inside the workspace.
pub fn outside_target(name: &str, args: &Value, ctx: &Context) -> Option<PathBuf> {
    let key = match name {
        "read" | "write" | "edit" | "git" => "path",
        "list" => "pattern",
        _ => return None,
    };
    let raw = args.get(key)?.as_str()?;
    let resolved = ctx.classify(raw).ok()?;
    if resolved.outside {
        Some(resolved.path)
    } else {
        None
    }
}

/// Approval identity: `<tool>:<canonical parent dir>` for an external target,
/// so "always allow" opens only that directory. `<tool>` otherwise.
pub fn approval_key(name: &str, target: Option<&Path>) -> String {
    match target {
        Some(path) => {
            let dir = path
                .parent()
                .map(canonical)
                .unwrap_or_else(|| path.to_path_buf());
            format!("{name}:{}", dir.to_string_lossy().replace('\\', "/"))
        }
        None => name.to_string(),
    }
}

pub fn approval_key_for(name: &str, args: Option<&Value>, target: Option<&Path>) -> String {
    if name == "git"
        && let Some(action) = args
            .and_then(|a| a.get("action"))
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
    {
        return format!("git:{}", action.to_ascii_lowercase());
    }
    approval_key(name, target)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
    pub risk: Risk,
}

pub fn catalog() -> Vec<ToolInfo> {
    specs()
        .into_iter()
        .filter_map(|spec| {
            let function = spec.get("function")?;
            let name = function.get("name")?.as_str()?.to_string();
            let description = function.get("description")?.as_str()?.to_string();
            Some(ToolInfo {
                risk: risk(&name),
                name,
                description,
            })
        })
        .collect()
}

pub fn specs() -> Vec<Value> {
    vec![
        json!({
            "type": "function",
            "function": {
                "name": "read",
                "description": "Read a UTF-8 text file from the workspace. Supports offset/limit line windows; long output is truncated.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Workspace-relative file path" },
                        "offset": { "type": "integer", "description": "1-based first line to read" },
                        "limit": { "type": "integer", "description": "Max lines to return (default 300)" }
                    },
                    "required": ["path"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "list",
                "description": "List workspace files matching a glob pattern.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "pattern": { "type": "string", "description": "Glob relative to the workspace, e.g. 'crates/**/*.rs' (default '**/*')" }
                    }
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "grep",
                "description": "Regex search in workspace file contents. Skips target/, .git/, node_modules/ and binary files.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "pattern": { "type": "string", "description": "Rust regex" },
                        "include": { "type": "string", "description": "Optional file glob filter, e.g. '*.rs'" }
                    },
                    "required": ["pattern"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "write",
                "description": "Create or overwrite a file; produces a diff. Runs automatically inside the workspace; a path outside the workspace requires user approval.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" },
                        "content": { "type": "string" }
                    },
                    "required": ["path", "content"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "edit",
                "description": "Replace an exact string in a file; produces a diff. Runs automatically inside the workspace; a path outside the workspace requires user approval.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" },
                        "old": { "type": "string", "description": "Exact text to find" },
                        "new": { "type": "string", "description": "Replacement text" },
                        "replace_all": { "type": "boolean", "description": "Replace every occurrence (default false)" }
                    },
                    "required": ["path", "old", "new"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "shell",
                "description": "Run a PowerShell command in the workspace and return its output. Requires user approval.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "command": { "type": "string" },
                        "timeout_secs": { "type": "integer", "description": "Kill after this many seconds (default 120, max 600)" }
                    },
                    "required": ["command"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "web_fetch",
                "description": "Fetch an http(s) URL and return readable text (or raw body). Useful for docs, issues, and API references.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "url": { "type": "string", "description": "http:// or https:// URL" },
                        "format": { "type": "string", "enum": ["text", "raw"], "description": "text strips HTML; raw returns the response body (default text)" },
                        "max_chars": { "type": "integer", "description": "Maximum characters to return (default 16000)" }
                    },
                    "required": ["url"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "todo",
                "description": "Maintain a workspace todo list in .evrax/todos.json. Use list/add/update/remove/clear.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "action": { "type": "string", "enum": ["list", "add", "update", "remove", "clear"] },
                        "id": { "type": "integer", "description": "Todo id for update/remove" },
                        "content": { "type": "string", "description": "Task text for add/update" },
                        "status": { "type": "string", "enum": ["pending", "in_progress", "completed"] },
                        "priority": { "type": "string", "enum": ["low", "medium", "high"] }
                    },
                    "required": ["action"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "git",
                "description": "Inspect or change the workspace git repository. Read-only actions run automatically; mutating actions require approval.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "action": { "type": "string", "enum": ["status", "diff", "log", "show", "branch", "add", "commit", "push", "pull", "fetch", "checkout", "restore"] },
                        "path": { "type": "string", "description": "Optional workspace-relative path" },
                        "staged": { "type": "boolean", "description": "diff --cached" },
                        "limit": { "type": "integer", "description": "Log entry limit (default 20)" },
                        "ref": { "type": "string", "description": "Commit/branch ref for log/show/checkout" },
                        "name": { "type": "string", "description": "Branch name for branch action" },
                        "message": { "type": "string", "description": "Commit message" },
                        "remote": { "type": "string", "description": "Remote for push/pull/fetch" },
                        "branch": { "type": "string", "description": "Branch for push/pull" },
                        "files": { "type": "array", "items": { "type": "string" }, "description": "Files for add/restore" }
                    },
                    "required": ["action"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "ast_grep",
                "description": "AST-aware structural code search/rewrite with ast-grep (sg). Use search first; replace with apply=true mutates files.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "action": { "type": "string", "enum": ["search", "replace"] },
                        "pattern": { "type": "string", "description": "ast-grep pattern (use $VAR / $$$ wildcards, not regex)" },
                        "lang": { "type": "string", "description": "Language, e.g. rust, ts, python" },
                        "path": { "type": "string", "description": "Workspace-relative path (default .)" },
                        "rewrite": { "type": "string", "description": "Replacement pattern for replace" },
                        "apply": { "type": "boolean", "description": "Apply the rewrite (default false = preview)" }
                    },
                    "required": ["action", "pattern", "lang"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "evrax-graph",
                "description": "Query the built-in code graph (symbols, callers/callees, impact, dead code, file outline). Run action=index first; the graph lives in .evrax/graph.db.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "action": { "type": "string", "enum": ["index", "status", "search", "callers", "callees", "impact", "dead_code", "outline"] },
                        "pattern": { "type": "string", "description": "Name pattern for search" },
                        "symbol": { "type": "string", "description": "Qualified name (src/main.rs::run) or a unique short name" },
                        "path": { "type": "string", "description": "Workspace-relative file for outline" },
                        "depth": { "type": "integer", "description": "Impact depth (default 3, max 10)" },
                        "limit": { "type": "integer", "description": "Search result limit (default 50, max 200)" },
                        "project": { "type": "string", "description": "Project name (default: workspace dir name)" }
                    },
                    "required": ["action"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "subagent",
                "description": "Launch and control another evrax agent in a separate terminal. start returns an id; use send/status/list/stop.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "action": { "type": "string", "enum": ["start", "send", "status", "list", "stop"] },
                        "id": { "type": "string", "description": "Subagent id returned by start" },
                        "name": { "type": "string", "description": "Display name for start" },
                        "prompt": { "type": "string", "description": "Initial or follow-up prompt" }
                    },
                    "required": ["action"]
                }
            }
        }),
    ]
}

pub fn risk(name: &str) -> Risk {
    match name {
        "write" | "edit" | "shell" | "todo" | "git" | "subagent" => Risk::Mutate,
        _ => Risk::Read,
    }
}

pub fn is_mutating(name: &str, args: Option<&Value>) -> bool {
    let action = args
        .and_then(|a| a.get("action"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    match name {
        "write" | "edit" | "shell" => true,
        "todo" => action != "list",
        "git" => match action.as_str() {
            "status" | "diff" | "log" | "show" => false,
            "branch" => args.is_some_and(|a| a.get("name").is_some()),
            _ => true,
        },
        "ast_grep" => {
            action == "replace"
                && args.is_some_and(|a| a.get("apply").and_then(Value::as_bool).unwrap_or(false))
        }
        "subagent" => action != "status" && action != "list",
        _ => false,
    }
}

pub fn requires_approval(name: &str, args: Option<&Value>) -> bool {
    name == "shell" || (name == "git" && is_mutating(name, args))
}

/// Short human-readable label for the TUI tool card.
pub fn summary(name: &str, args: &Value) -> String {
    let field = |key: &str| {
        args.get(key)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string()
    };
    match name {
        "read" | "write" | "edit" => field("path"),
        "list" => {
            let pattern = field("pattern");
            if pattern.is_empty() {
                "**/*".into()
            } else {
                pattern
            }
        }
        "grep" => field("pattern"),
        "shell" => field("command"),
        "web_fetch" => field("url"),
        "todo" => {
            let action = field("action");
            let id = args
                .get("id")
                .and_then(|v| v.as_u64())
                .map(|v| v.to_string())
                .unwrap_or_else(|| field("content"));
            format!("{action} {id}").trim().to_string()
        }
        "git" => {
            let mut parts = vec![field("action")];
            for key in ["path", "ref", "name", "message", "remote", "branch"] {
                let value = field(key);
                if !value.is_empty() {
                    parts.push(value);
                    break;
                }
            }
            if let Some(files) = args.get("files").and_then(Value::as_array) {
                parts.push(format!("{} files", files.len()));
            }
            parts.join(" ")
        }
        "ast_grep" => format!("{} {} {}", field("action"), field("pattern"), field("lang"))
            .trim()
            .to_string(),
        "evrax-graph" => {
            let mut parts = vec![field("action")];
            for key in ["symbol", "pattern", "path", "project"] {
                let value = field(key);
                if !value.is_empty() {
                    parts.push(value);
                    break;
                }
            }
            parts.join(" ")
        }
        "subagent" => format!("{} {}", field("action"), field("name"))
            .trim()
            .to_string(),
        _ => String::new(),
    }
}

pub async fn run(name: &str, args: &Value, ctx: &Context) -> Result<ToolOutcome, String> {
    match name {
        "read" => read(args, ctx),
        "list" => list(args, ctx),
        "grep" => grep(args, ctx),
        "write" => write(args, ctx),
        "edit" => edit(args, ctx),
        "shell" => shell(args, ctx).await,
        "web_fetch" => web::run(args).await,
        "todo" => todo::run(args, ctx),
        "git" => git::run(args, ctx).await,
        "ast_grep" => ast_grep::run(args, ctx).await,
        "evrax-graph" => graph::run(args, ctx),
        "subagent" => crate::subagent::run(args, ctx),
        other => Err(format!("unknown tool: {other}")),
    }
}

fn arg_str(args: &Value, key: &str) -> Result<String, String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| format!("missing string argument `{key}`"))
}

fn read(args: &Value, ctx: &Context) -> Result<ToolOutcome, String> {
    let path = ctx.classify(&arg_str(args, "path")?)?.path;
    let offset = args
        .get("offset")
        .and_then(|v| v.as_u64())
        .unwrap_or(1)
        .max(1) as usize;
    let limit = args
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize)
        .unwrap_or(300)
        .min(MAX_OUTPUT_LINES);
    let text =
        std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", ctx.display(&path)))?;
    let total = text.lines().count();
    let body: Vec<&str> = text.lines().skip(offset - 1).take(limit).collect();
    let mut out = String::new();
    for (i, line) in body.iter().enumerate() {
        out.push_str(&format!("{:>5}  {line}\n", offset + i));
    }
    truncate(&mut out);
    let header = if body.len() < total.saturating_sub(offset - 1) {
        format!("{} lines shown (of {total})", body.len())
    } else {
        format!("{} lines", body.len())
    };
    Ok(ToolOutcome {
        ok: true,
        detail: if out.trim().is_empty() {
            "(empty file)".into()
        } else {
            format!("{header}\n{out}")
        },
        diff: None,
    })
}

const SKIP_DIRS: [&str; 4] = ["target", ".git", ".codebase-memory", "node_modules"];

fn list(args: &Value, ctx: &Context) -> Result<ToolOutcome, String> {
    let pattern = args
        .get("pattern")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("**/*")
        .to_string();
    let full = ctx.root.join(&pattern).to_string_lossy().replace('\\', "/");
    let entries = glob::glob(&full).map_err(|e| e.to_string())?;
    let mut out = String::new();
    let mut count = 0usize;
    for entry in entries.flatten() {
        if skip_path(ctx, &entry) || !entry.is_file() {
            continue;
        }
        out.push_str(&ctx.display(&entry));
        out.push('\n');
        count += 1;
        if count >= 400 {
            out.push_str("â€¦ truncated\n");
            break;
        }
    }
    Ok(ToolOutcome {
        ok: true,
        detail: if count == 0 {
            "no files matched".into()
        } else {
            format!("{count} paths\n{out}")
        },
        diff: None,
    })
}

fn skip_path(ctx: &Context, path: &Path) -> bool {
    path.strip_prefix(&ctx.root)
        .unwrap_or(path)
        .components()
        .any(|c| SKIP_DIRS.contains(&c.as_os_str().to_string_lossy().as_ref()))
}

fn grep(args: &Value, ctx: &Context) -> Result<ToolOutcome, String> {
    let pattern = arg_str(args, "pattern")?;
    let re = regex::Regex::new(&pattern).map_err(|e| format!("bad regex: {e}"))?;
    let include = match args
        .get("include")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
    {
        Some(raw) => Some(glob::Pattern::new(raw).map_err(|e| format!("bad include glob: {e}"))?),
        None => None,
    };
    let mut out = String::new();
    let mut hits = 0usize;
    let mut files = 0usize;
    let mut truncated = false;
    for entry in walk(&ctx.root) {
        let Ok(text) = std::fs::read_to_string(&entry) else {
            continue;
        };
        let rel = ctx.display(&entry);
        if let Some(p) = &include {
            let name = entry.file_name().and_then(|f| f.to_str()).unwrap_or("");
            if !(p.matches(&rel) || p.matches(name)) {
                continue;
            }
        }
        let mut file_hits = 0usize;
        for (n, line) in text.lines().enumerate() {
            if !re.is_match(line) {
                continue;
            }
            if hits >= 200 {
                truncated = true;
                break;
            }
            out.push_str(&format!("{rel}:{}: {}\n", n + 1, line.trim_end()));
            hits += 1;
            file_hits += 1;
        }
        if file_hits > 0 {
            files += 1;
        }
        if truncated {
            break;
        }
    }
    Ok(ToolOutcome {
        ok: true,
        detail: if hits == 0 {
            "no matches".into()
        } else {
            let marker = if truncated { "\nâ€¦ truncated" } else { "" };
            format!("{hits} matches in {files} files\n{out}{marker}")
        },
        diff: None,
    })
}

fn walk(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if SKIP_DIRS.contains(&name.as_str()) {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

fn write(args: &Value, ctx: &Context) -> Result<ToolOutcome, String> {
    let path = ctx.classify(&arg_str(args, "path")?)?.path;
    let content = arg_str(args, "content")?;
    let old = std::fs::read_to_string(&path).unwrap_or_default();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, &content).map_err(|e| format!("{}: {e}", ctx.display(&path)))?;
    let lines = diff_lines(&old, &content);
    let added = lines.iter().filter(|l| l.kind == DiffKind::Add).count();
    let removed = lines.iter().filter(|l| l.kind == DiffKind::Rem).count();
    Ok(ToolOutcome {
        ok: true,
        detail: format!("{} written (+{added} / -{removed})", ctx.display(&path)),
        diff: Some((ctx.display(&path), lines)),
    })
}

fn edit(args: &Value, ctx: &Context) -> Result<ToolOutcome, String> {
    let path = ctx.classify(&arg_str(args, "path")?)?.path;
    let old = arg_str(args, "old")?;
    let new = arg_str(args, "new")?;
    let replace_all = args
        .get("replace_all")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if old.is_empty() {
        return Err("`old` must not be empty".into());
    }
    let text =
        std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", ctx.display(&path)))?;
    let occurrences = text.matches(&old).count();
    if occurrences == 0 {
        return Err(format!(
            "{}: text not found; read the file and retry with an exact snippet",
            ctx.display(&path)
        ));
    }
    if occurrences > 1 && !replace_all {
        return Err(format!(
            "{}: {occurrences} matches; add more context or set replace_all=true",
            ctx.display(&path)
        ));
    }
    let updated = if replace_all {
        text.replace(&old, &new)
    } else {
        text.replacen(&old, &new, 1)
    };
    std::fs::write(&path, &updated).map_err(|e| format!("{}: {e}", ctx.display(&path)))?;
    let lines = diff_lines(&text, &updated);
    let added = lines.iter().filter(|l| l.kind == DiffKind::Add).count();
    let removed = lines.iter().filter(|l| l.kind == DiffKind::Rem).count();
    let label = if occurrences > 1 {
        format!("{occurrences} replacements")
    } else {
        "1 hunk applied".to_string()
    };
    Ok(ToolOutcome {
        ok: true,
        detail: format!("{}: {label} (+{added} / -{removed})", ctx.display(&path)),
        diff: Some((ctx.display(&path), lines)),
    })
}

async fn shell(args: &Value, ctx: &Context) -> Result<ToolOutcome, String> {
    let command = arg_str(args, "command")?;
    let secs = args
        .get("timeout_secs")
        .and_then(|v| v.as_u64())
        .unwrap_or(120)
        .clamp(1, 600);
    let started = std::time::Instant::now();
    let child = Command::new(shell_program())
        .args(shell_args(&command))
        .current_dir(&ctx.root)
        .env("PATH", path_with_cargo())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("spawn failed: {e}"))?;
    let output =
        match tokio::time::timeout(Duration::from_secs(secs), child.wait_with_output()).await {
            Ok(Ok(output)) => output,
            Ok(Err(e)) => return Err(format!("shell error: {e}")),
            Err(_) => return Err(format!("command killed after {secs}s timeout")),
        };
    let mut text = String::from_utf8_lossy(&output.stdout).to_string();
    let err = String::from_utf8_lossy(&output.stderr).to_string();
    if !err.trim().is_empty() {
        if !text.trim().is_empty() {
            text.push_str("\n[stderr]\n");
        }
        text.push_str(&err);
    }
    truncate(&mut text);
    let ok = output.status.success();
    let code = output
        .status
        .code()
        .map(|c| c.to_string())
        .unwrap_or_else(|| "signal".into());
    let detail = format!(
        "exit {code} · {:.1}s\n{}",
        started.elapsed().as_secs_f32(),
        if text.trim().is_empty() {
            "(no output)"
        } else {
            text.trim_end()
        }
    );
    Ok(ToolOutcome {
        ok,
        detail,
        diff: None,
    })
}

fn shell_program() -> &'static str {
    if cfg!(windows) {
        "powershell.exe"
    } else {
        "sh"
    }
}

fn shell_args(command: &str) -> Vec<&str> {
    if cfg!(windows) {
        vec!["-NoProfile", "-NonInteractive", "-Command", command]
    } else {
        vec!["-c", command]
    }
}

pub(super) struct CmdResult {
    pub ok: bool,
    pub code: String,
    pub text: String,
}

pub(super) async fn run_command(
    program: &str,
    args: &[&str],
    cwd: &Path,
    secs: u64,
) -> Result<CmdResult, String> {
    let child = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .env("PATH", path_with_cargo())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("spawn {program} failed: {e}"))?;
    let output =
        match tokio::time::timeout(Duration::from_secs(secs), child.wait_with_output()).await {
            Ok(Ok(output)) => output,
            Ok(Err(e)) => return Err(format!("{program} error: {e}")),
            Err(_) => return Err(format!("{program} killed after {secs}s timeout")),
        };
    let mut text = String::from_utf8_lossy(&output.stdout).to_string();
    let err = String::from_utf8_lossy(&output.stderr).to_string();
    if !err.trim().is_empty() {
        if !text.trim().is_empty() {
            text.push_str("\n[stderr]\n");
        }
        text.push_str(&err);
    }
    truncate(&mut text);
    Ok(CmdResult {
        ok: output.status.success(),
        code: output
            .status
            .code()
            .map(|c| c.to_string())
            .unwrap_or_else(|| "signal".into()),
        text,
    })
}

/// Make sure `cargo` is reachable even when evrax was started from a shell
/// whose PATH lacks the cargo bin directory.
fn path_with_cargo() -> std::ffi::OsString {
    let mut path = std::env::var_os("PATH").unwrap_or_default();
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"));
    if let Some(home) = home {
        let bin = Path::new(&home).join(".cargo").join("bin");
        if bin.is_dir() {
            path.push(if cfg!(windows) { ";" } else { ":" });
            path.push(bin.as_os_str());
        }
    }
    path
}

/// Keep the head and tail of oversized tool output.
pub fn truncate(text: &mut String) {
    if text.len() <= MAX_OUTPUT_BYTES {
        return;
    }
    let head_end = floor_char_boundary(text, MAX_OUTPUT_BYTES / 2);
    let tail_start = floor_char_boundary(text, text.len() - MAX_OUTPUT_BYTES / 2);
    let omitted = tail_start - head_end;
    let head = text[..head_end].to_string();
    let tail = text[tail_start..].to_string();
    *text = format!("{head}\nâ€¦ {omitted} bytes omitted â€¦\n{tail}");
}

fn floor_char_boundary(text: &str, mut idx: usize) -> usize {
    idx = idx.min(text.len());
    while idx > 0 && !text.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> (tempfile::TempDir, Context) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        (dir, Context::new(root))
    }

    #[test]
    fn classify_flags_outside_paths() {
        let (_d, c) = ctx();
        assert!(!c.classify("a/b.txt").unwrap().outside);
        let up = c.classify("../secret").unwrap();
        assert!(up.outside);
        assert!(!up.path.starts_with(&c.root));
        assert!(c.classify("a/../../b").unwrap().outside);
        // A not-yet-existing file inside the workspace still classifies cleanly.
        assert!(!c.classify("new/dir/file.txt").unwrap().outside);
    }

    #[test]
    fn outside_target_and_key_only_for_external_paths() {
        let (_d, c) = ctx();
        // shell/grep have no path argument -> never flagged external.
        assert!(outside_target("shell", &json!({"command": "ls"}), &c).is_none());
        assert!(outside_target("grep", &json!({"pattern": "x"}), &c).is_none());
        // inside path -> none.
        assert!(outside_target("read", &json!({"path": "a.txt"}), &c).is_none());
        // external path -> Some, key is tool:canonical-parent.
        let external = std::env::temp_dir().join("evrax-outside-target.txt");
        let raw = external.to_string_lossy().to_string();
        let target = outside_target("read", &json!({"path": raw}), &c).unwrap();
        let key = approval_key("read", Some(&target));
        assert!(key.starts_with("read:"));
        assert!(
            !key.contains("evrax-outside-target"),
            "key drops the file name: {key}"
        );
        assert_eq!(approval_key("shell", None), "shell");
    }

    #[test]
    fn write_then_read_roundtrip() {
        let (_d, c) = ctx();
        let out = write(&json!({"path":"a.txt","content":"hello\nworld\n"}), &c).unwrap();
        assert!(out.ok);
        assert!(out.diff.is_some());
        let read_back = read(&json!({"path":"a.txt"}), &c).unwrap();
        assert!(read_back.detail.contains("hello"));
    }

    #[test]
    fn edit_requires_unique_match_by_default() {
        let (_d, c) = ctx();
        write(&json!({"path":"a.txt","content":"x\nx\n"}), &c).unwrap();
        assert!(edit(&json!({"path":"a.txt","old":"x","new":"y"}), &c).is_err());
        let ok = edit(
            &json!({"path":"a.txt","old":"x","new":"y","replace_all":true}),
            &c,
        )
        .unwrap();
        assert!(ok.ok);
    }

    #[test]
    fn edit_reports_missing_text() {
        let (_d, c) = ctx();
        write(&json!({"path":"a.txt","content":"abc"}), &c).unwrap();
        let err = edit(&json!({"path":"a.txt","old":"zzz","new":"y"}), &c).unwrap_err();
        assert!(err.contains("not found"));
    }

    #[test]
    fn grep_and_list_find_files() {
        let (_d, c) = ctx();
        write(&json!({"path":"src/a.rs","content":"fn main() {}\n"}), &c).unwrap();
        write(&json!({"path":"src/b.rs","content":"let x = 1;\n"}), &c).unwrap();
        let hits = grep(&json!({"pattern":"fn main","include":"*.rs"}), &c).unwrap();
        assert!(hits.detail.contains("1 matches"));
        assert!(hits.detail.contains("src/a.rs"));
        let listed = list(&json!({"pattern":"**/*.rs"}), &c).unwrap();
        assert!(listed.detail.contains("src/b.rs"));
    }

    #[test]
    fn read_honors_offset_and_limit() {
        let (_d, c) = ctx();
        let content: String = (1..=10).map(|i| format!("line {i}\n")).collect();
        write(&json!({"path":"a.txt","content":content}), &c).unwrap();
        let out = read(&json!({"path":"a.txt","offset":3,"limit":2}), &c).unwrap();
        assert!(out.detail.contains("line 3"));
        assert!(out.detail.contains("line 4"));
        assert!(!out.detail.contains("line 5"));
    }

    #[test]
    fn truncate_keeps_head_and_tail() {
        let mut text = "a".repeat(MAX_OUTPUT_BYTES * 2);
        text.push_str("END");
        truncate(&mut text);
        assert!(text.len() < MAX_OUTPUT_BYTES + 64);
        assert!(text.contains("omitted"));
        assert!(text.ends_with("END"));
    }

    #[test]
    fn mutate_tools_need_approval() {
        assert_eq!(risk("write"), Risk::Mutate);
        assert_eq!(risk("shell"), Risk::Mutate);
        assert_eq!(risk("read"), Risk::Read);
    }

    #[tokio::test]
    async fn shell_runs_and_captures_output() {
        let (_d, c) = ctx();
        let out = shell(&json!({"command":"echo hi"}), &c).await.unwrap();
        assert!(out.ok);
        assert!(out.detail.contains("hi"));
    }

    #[tokio::test]
    async fn shell_reports_nonzero_exit() {
        let (_d, c) = ctx();
        let out = shell(&json!({"command":"exit 3"}), &c).await.unwrap();
        assert!(!out.ok);
        assert!(out.detail.contains("exit 3"));
    }
}
