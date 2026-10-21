use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;

use super::{Context, ToolOutcome, arg_str, run_command};

pub(super) async fn run(args: &Value, ctx: &Context) -> Result<ToolOutcome, String> {
    let action = args
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or("search")
        .to_ascii_lowercase();
    if action != "search" && action != "replace" {
        return Err(format!("unknown ast_grep action: {action}"));
    }
    let pattern = arg_str(args, "pattern")?;
    let lang = arg_str(args, "lang")?;
    let path = args
        .get("path")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(".");
    let resolved = ctx.classify(path)?;
    if resolved.outside {
        return Err(format!(
            "{} is outside the workspace",
            resolved.path.display()
        ));
    }
    let binary = find_binary().ok_or_else(|| {
        "ast-grep is not installed; run skill-evrax/optional/software-development/ast-grep/install.ps1 -Quiet".to_string()
    })?;
    let binary = binary.to_string_lossy().to_string();

    let mut command = vec!["run".to_string(), "--pattern".into(), pattern.clone()];
    command.extend(["--lang".into(), lang.clone()]);
    if action == "replace" {
        let rewrite = arg_str(args, "rewrite")?;
        command.extend(["--rewrite".into(), rewrite]);
    }
    let apply = action == "replace" && args.get("apply").and_then(Value::as_bool).unwrap_or(false);
    if apply {
        command.push("--update-all".into());
    } else {
        command.push("--json=compact".into());
    }
    command.push(path.to_string());

    let refs: Vec<&str> = command.iter().map(String::as_str).collect();
    let result = run_command(&binary, &refs, &ctx.root, 120).await?;
    let detail = format!(
        "ast-grep {action} exit {}\n{}",
        result.code,
        if result.text.trim().is_empty() {
            "(no output)"
        } else {
            result.text.trim_end()
        }
    );
    Ok(ToolOutcome {
        ok: result.ok,
        detail,
        diff: None,
    })
}

fn find_binary() -> Option<PathBuf> {
    for name in ["ast-grep", "sg"] {
        if Command::new(name)
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
        {
            return Some(PathBuf::from(name));
        }
    }
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).parent()?.parent()?;
    let bundled = repo
        .join("skill-evrax")
        .join("optional")
        .join("software-development")
        .join("ast-grep")
        .join("bin")
        .join(if cfg!(windows) { "sg.exe" } else { "sg" });
    bundled.is_file().then_some(bundled)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{find_binary, run};
    use crate::tools::Context;

    #[test]
    fn ast_grep_apply_is_mutating() {
        assert!(!crate::tools::is_mutating(
            "ast_grep",
            Some(&json!({"action":"search"}))
        ));
        assert!(!crate::tools::is_mutating(
            "ast_grep",
            Some(&json!({"action":"replace","apply":false}))
        ));
        assert!(crate::tools::is_mutating(
            "ast_grep",
            Some(&json!({"action":"replace","apply":true}))
        ));
    }

    #[tokio::test]
    async fn search_finds_structural_matches() {
        if find_binary().is_none() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let ctx = Context::new(dir.path().to_path_buf());
        write_file(&dir.path().join("a.rs"), "fn main() {}\n");
        let out = run(
            &json!({"action":"search","pattern":"fn $NAME() $$$","lang":"rust","path":"a.rs"}),
            &ctx,
        )
        .await
        .unwrap();
        assert!(out.ok, "{}", out.detail);
        assert!(out.detail.contains("a.rs"), "{}", out.detail);
    }

    fn write_file(path: &std::path::Path, content: &str) {
        std::fs::write(path, content).unwrap();
    }
}
