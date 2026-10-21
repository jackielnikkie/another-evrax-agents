use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{Context, ToolOutcome};

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Item {
    id: u64,
    content: String,
    status: String,
    priority: String,
}

pub(super) fn run(args: &Value, ctx: &Context) -> Result<ToolOutcome, String> {
    let action = args
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or("list")
        .to_ascii_lowercase();
    let path = todo_path(ctx);
    let mut items = load(&path)?;
    match action.as_str() {
        "list" => Ok(outcome(render(&items), true)),
        "add" => {
            let content = required(args, "content")?;
            let status = status(args, "pending")?;
            let priority = priority(args, "medium")?;
            let id = items.iter().map(|item| item.id).max().unwrap_or(0) + 1;
            items.push(Item {
                id,
                content,
                status,
                priority,
            });
            save(&path, &items)?;
            Ok(outcome(format!("#{id} added\n{}", render(&items)), true))
        }
        "update" => {
            let id = id(args)?;
            let item = items
                .iter_mut()
                .find(|item| item.id == id)
                .ok_or_else(|| format!("todo #{id} not found"))?;
            if let Some(content) = args.get("content").and_then(Value::as_str) {
                if content.trim().is_empty() {
                    return Err("content must not be empty".into());
                }
                item.content = content.trim().to_string();
            }
            if args.get("status").is_some() {
                item.status = status(args, &item.status)?;
            }
            if args.get("priority").is_some() {
                item.priority = priority(args, &item.priority)?;
            }
            save(&path, &items)?;
            Ok(outcome(format!("#{id} updated\n{}", render(&items)), true))
        }
        "remove" => {
            let id = id(args)?;
            let before = items.len();
            items.retain(|item| item.id != id);
            if items.len() == before {
                return Err(format!("todo #{id} not found"));
            }
            save(&path, &items)?;
            Ok(outcome(format!("#{id} removed\n{}", render(&items)), true))
        }
        "clear" => {
            items.clear();
            save(&path, &items)?;
            Ok(outcome("todos cleared".into(), true))
        }
        other => Err(format!("unknown todo action: {other}")),
    }
}

fn todo_path(ctx: &Context) -> PathBuf {
    ctx.root.join(".evrax").join("todos.json")
}

fn load(path: &PathBuf) -> Result<Vec<Item>, String> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Ok(Vec::new());
    };
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

fn save(path: &PathBuf, items: &[Item]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(items).map_err(|e| e.to_string())?;
    std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

fn required(args: &Value, key: &str) -> Result<String, String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| format!("missing string argument `{key}`"))
}

fn id(args: &Value) -> Result<u64, String> {
    args.get("id")
        .and_then(|value| {
            value
                .as_u64()
                .or_else(|| value.as_str().and_then(|s| s.parse().ok()))
        })
        .ok_or_else(|| "missing integer argument `id`".into())
}

fn status(args: &Value, fallback: &str) -> Result<String, String> {
    let value = args
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or(fallback)
        .to_ascii_lowercase();
    match value.as_str() {
        "pending" | "in_progress" | "completed" => Ok(value),
        _ => Err("status must be pending, in_progress, or completed".into()),
    }
}

fn priority(args: &Value, fallback: &str) -> Result<String, String> {
    let value = args
        .get("priority")
        .and_then(Value::as_str)
        .unwrap_or(fallback)
        .to_ascii_lowercase();
    match value.as_str() {
        "low" | "medium" | "high" => Ok(value),
        _ => Err("priority must be low, medium, or high".into()),
    }
}

fn render(items: &[Item]) -> String {
    let open = items
        .iter()
        .filter(|item| item.status != "completed")
        .count();
    let mut lines = vec![format!("todos: {open} open / {} total", items.len())];
    for item in items {
        let marker = if item.status == "completed" { "x" } else { " " };
        lines.push(format!(
            "[{marker}] #{} ({}, {}) {}",
            item.id, item.status, item.priority, item.content
        ));
    }
    lines.join("\n")
}

fn outcome(detail: String, ok: bool) -> ToolOutcome {
    ToolOutcome {
        ok,
        detail,
        diff: None,
    }
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
    fn add_list_update_and_remove() {
        let (_dir, ctx) = ctx();
        let added = run(
            &serde_json::json!({"action":"add","content":"ship it"}),
            &ctx,
        )
        .unwrap();
        assert!(added.detail.contains("#1"));
        let listed = run(&serde_json::json!({"action":"list"}), &ctx).unwrap();
        assert!(listed.detail.contains("ship it"));
        let updated = run(
            &serde_json::json!({"action":"update","id":1,"status":"completed"}),
            &ctx,
        )
        .unwrap();
        assert!(updated.detail.contains("completed"));
        let removed = run(&serde_json::json!({"action":"remove","id":1}), &ctx).unwrap();
        assert!(removed.detail.contains("removed"));
        assert!(run(&serde_json::json!({"action":"remove","id":1}), &ctx).is_err());
    }
}
