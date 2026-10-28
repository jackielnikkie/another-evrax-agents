use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{Context, ToolOutcome};

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Memory {
    id: u64,
    key: String,
    content: String,
    #[serde(default)]
    tags: Vec<String>,
    updated_ms: u64,
}

pub(super) fn run(args: &Value, ctx: &Context) -> Result<ToolOutcome, String> {
    let action = args
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or("list")
        .to_ascii_lowercase();
    let path = memory_path(ctx);
    let mut items = load(&path)?;
    match action.as_str() {
        "list" => Ok(outcome(render(&items), true)),
        "search" => {
            let query = required(args, "query")?.to_ascii_lowercase();
            let hits: Vec<Memory> = items
                .iter()
                .filter(|item| matches(item, &query))
                .cloned()
                .collect();
            Ok(outcome(render(&hits), true))
        }
        "add" => {
            let content = required(args, "content")?;
            let key = args
                .get("key")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_string();
            let tags = tags(args)?;
            let now = now_ms();
            if !key.is_empty()
                && let Some(item) = items.iter_mut().find(|item| item.key == key)
            {
                item.content = content;
                item.tags = tags;
                item.updated_ms = now;
                save(&path, &items)?;
                return Ok(outcome(
                    format!("key '{key}' updated\n{}", render(&items)),
                    true,
                ));
            }
            let id = items.iter().map(|item| item.id).max().unwrap_or(0) + 1;
            items.push(Memory {
                id,
                key,
                content,
                tags,
                updated_ms: now,
            });
            save(&path, &items)?;
            Ok(outcome(format!("#{id} added\n{}", render(&items)), true))
        }
        "update" => {
            let id = id(args)?;
            let item = items
                .iter_mut()
                .find(|item| item.id == id)
                .ok_or_else(|| format!("memory #{id} not found"))?;
            if let Some(content) = args.get("content").and_then(Value::as_str) {
                if content.trim().is_empty() {
                    return Err("content must not be empty".into());
                }
                item.content = content.trim().to_string();
            }
            if let Some(key) = args.get("key").and_then(Value::as_str) {
                item.key = key.trim().to_string();
            }
            if args.get("tags").is_some() {
                item.tags = tags(args)?;
            }
            item.updated_ms = now_ms();
            save(&path, &items)?;
            Ok(outcome(format!("#{id} updated\n{}", render(&items)), true))
        }
        "remove" => {
            let id = id(args)?;
            let before = items.len();
            items.retain(|item| item.id != id);
            if items.len() == before {
                return Err(format!("memory #{id} not found"));
            }
            save(&path, &items)?;
            Ok(outcome(format!("#{id} removed\n{}", render(&items)), true))
        }
        "clear" => {
            items.clear();
            save(&path, &items)?;
            Ok(outcome("memory cleared".into(), true))
        }
        other => Err(format!("unknown memory action: {other}")),
    }
}

fn memory_path(ctx: &Context) -> PathBuf {
    ctx.root.join(".evrax").join("memory.json")
}

fn load(path: &PathBuf) -> Result<Vec<Memory>, String> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Ok(Vec::new());
    };
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

fn save(path: &PathBuf, items: &[Memory]) -> Result<(), String> {
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

fn tags(args: &Value) -> Result<Vec<String>, String> {
    match args.get("tags") {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(values)) => values
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
                    .ok_or_else(|| "tags must be non-empty strings".to_string())
            })
            .collect(),
        Some(_) => Err("tags must be an array of strings".into()),
    }
}

fn matches(item: &Memory, query: &str) -> bool {
    item.key.to_ascii_lowercase().contains(query)
        || item.content.to_ascii_lowercase().contains(query)
        || item
            .tags
            .iter()
            .any(|tag| tag.to_ascii_lowercase().contains(query))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn render(items: &[Memory]) -> String {
    let mut lines = vec![format!("memory: {} entries", items.len())];
    for item in items {
        let head = if item.key.is_empty() {
            item.content.chars().take(40).collect::<String>()
        } else {
            item.key.clone()
        };
        let tags = if item.tags.is_empty() {
            String::new()
        } else {
            format!(" [{}]", item.tags.join(", "))
        };
        let content: String = item.content.chars().take(160).collect();
        lines.push(format!("[#{}] {head}{tags} {content}", item.id));
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
    fn add_list_update_by_key_and_search() {
        let (_dir, ctx) = ctx();
        run(
            &serde_json::json!({"action":"add","key":"a","content":"x"}),
            &ctx,
        )
        .unwrap();
        let second = run(
            &serde_json::json!({"action":"add","key":"a","content":"y"}),
            &ctx,
        )
        .unwrap();
        assert!(second.detail.contains("key 'a' updated"));
        let listed = run(&serde_json::json!({"action":"list"}), &ctx).unwrap();
        assert!(listed.detail.contains("memory: 1 entries"));
        assert!(listed.detail.contains("y"));
        assert!(!listed.detail.contains(" x"));

        let hit = run(&serde_json::json!({"action":"search","query":"y"}), &ctx).unwrap();
        assert!(hit.detail.contains("memory: 1 entries"));
        let miss = run(&serde_json::json!({"action":"search","query":"zzz"}), &ctx).unwrap();
        assert!(miss.detail.contains("memory: 0 entries"));

        let reloaded = load(&memory_path(&ctx)).unwrap();
        assert_eq!(reloaded.len(), 1);
        assert_eq!(reloaded[0].content, "y");
    }

    #[test]
    fn add_without_key_appends() {
        let (_dir, ctx) = ctx();
        run(&serde_json::json!({"action":"add","content":"one"}), &ctx).unwrap();
        run(&serde_json::json!({"action":"add","content":"two"}), &ctx).unwrap();
        let listed = run(&serde_json::json!({"action":"list"}), &ctx).unwrap();
        assert!(listed.detail.contains("memory: 2 entries"));
    }

    #[test]
    fn remove_and_clear() {
        let (_dir, ctx) = ctx();
        run(&serde_json::json!({"action":"add","content":"one"}), &ctx).unwrap();
        run(&serde_json::json!({"action":"add","content":"two"}), &ctx).unwrap();
        let removed = run(&serde_json::json!({"action":"remove","id":1}), &ctx).unwrap();
        assert!(removed.detail.contains("#1 removed"));
        assert!(run(&serde_json::json!({"action":"remove","id":1}), &ctx).is_err());
        let cleared = run(&serde_json::json!({"action":"clear"}), &ctx).unwrap();
        assert!(cleared.detail.contains("memory cleared"));
        let listed = run(&serde_json::json!({"action":"list"}), &ctx).unwrap();
        assert!(listed.detail.contains("memory: 0 entries"));
    }

    #[test]
    fn update_by_id() {
        let (_dir, ctx) = ctx();
        run(
            &serde_json::json!({"action":"add","key":"a","content":"x"}),
            &ctx,
        )
        .unwrap();
        let updated = run(
            &serde_json::json!({"action":"update","id":1,"content":"z","tags":["k"]}),
            &ctx,
        )
        .unwrap();
        assert!(updated.detail.contains("#1 updated"));
        let listed = run(&serde_json::json!({"action":"list"}), &ctx).unwrap();
        assert!(listed.detail.contains("z"));
        assert!(listed.detail.contains("k"));
        assert!(
            run(
                &serde_json::json!({"action":"update","id":9,"content":"q"}),
                &ctx
            )
            .is_err()
        );
    }

    #[test]
    fn unknown_action_errors() {
        let (_dir, ctx) = ctx();
        assert!(run(&serde_json::json!({"action":"bogus"}), &ctx).is_err());
    }
}
