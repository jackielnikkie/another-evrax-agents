use serde_json::Value;

use super::{Context, ToolOutcome, arg_str, run_command};

pub(super) async fn run(args: &Value, ctx: &Context) -> Result<ToolOutcome, String> {
    let action = args
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or("status")
        .to_ascii_lowercase();
    let command = build(&action, args, ctx)?;
    let refs: Vec<&str> = command.iter().map(String::as_str).collect();
    let result = run_command("git", &refs, &ctx.root, 60).await?;
    let detail = format!(
        "git {} exit {}\n{}",
        action,
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

fn build(action: &str, args: &Value, ctx: &Context) -> Result<Vec<String>, String> {
    let field = |key: &str| {
        args.get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    match action {
        "status" => Ok(vec![
            "status".into(),
            "--short".into(),
            "--branch".into(),
            "--untracked-files=all".into(),
        ]),
        "diff" => {
            let mut cmd = vec!["diff".into()];
            if args.get("staged").and_then(Value::as_bool).unwrap_or(false) {
                cmd.push("--cached".into());
            }
            if let Some(context) = args.get("context").and_then(Value::as_u64) {
                cmd.push("-U".into());
                cmd.push(context.clamp(0, 40).to_string());
            }
            let path = field("path");
            if !path.is_empty() {
                cmd.extend(["--".into(), path]);
            }
            Ok(cmd)
        }
        "log" => {
            let limit = args
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(20)
                .clamp(1, 500)
                .to_string();
            let mut cmd = vec![
                "log".into(),
                "--oneline".into(),
                "--decorate".into(),
                "-n".into(),
                limit,
            ];
            let reference = field("ref");
            if !reference.is_empty() {
                validate_ref(&reference)?;
                cmd.push(reference);
            }
            Ok(cmd)
        }
        "show" => {
            let reference = arg_str(args, "ref")?;
            validate_ref(&reference)?;
            Ok(vec!["show".into(), reference])
        }
        "branch" => {
            let name = field("name");
            if name.is_empty() {
                Ok(vec!["branch".into(), "--list".into(), "-vv".into()])
            } else {
                validate_ref(&name)?;
                Ok(vec!["branch".into(), name])
            }
        }
        "add" => {
            let files = paths(args, ctx)?;
            if files.is_empty() {
                return Err("add requires files".into());
            }
            let mut cmd = vec!["add".into(), "--".into()];
            cmd.extend(files);
            Ok(cmd)
        }
        "commit" => {
            let message = arg_str(args, "message")?;
            if message.trim().is_empty() {
                return Err("commit message must not be empty".into());
            }
            Ok(vec!["commit".into(), "-m".into(), message])
        }
        "push" => {
            let mut cmd = vec!["push".into()];
            let remote = field("remote");
            if !remote.is_empty() {
                validate_ref(&remote)?;
                cmd.push(remote);
            }
            let branch = field("branch");
            if !branch.is_empty() {
                validate_ref(&branch)?;
                cmd.push(branch);
            }
            Ok(cmd)
        }
        "pull" => {
            let mut cmd = vec!["pull".into()];
            let remote = field("remote");
            if !remote.is_empty() {
                validate_ref(&remote)?;
                cmd.push(remote);
            }
            let branch = field("branch");
            if !branch.is_empty() {
                validate_ref(&branch)?;
                cmd.push(branch);
            }
            Ok(cmd)
        }
        "fetch" => {
            let mut cmd = vec!["fetch".into()];
            let remote = field("remote");
            if !remote.is_empty() {
                validate_ref(&remote)?;
                cmd.push(remote);
            }
            Ok(cmd)
        }
        "checkout" => {
            let reference = arg_str(args, "ref")?;
            validate_ref(&reference)?;
            Ok(vec!["checkout".into(), reference])
        }
        "restore" => {
            let files = paths(args, ctx)?;
            if files.is_empty() {
                return Err("restore requires files".into());
            }
            let mut cmd = vec!["restore".into(), "--".into()];
            cmd.extend(files);
            Ok(cmd)
        }
        other => Err(format!("unknown git action: {other}")),
    }
}

fn paths(args: &Value, ctx: &Context) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    if let Some(values) = args.get("files").and_then(Value::as_array) {
        for value in values {
            let raw = value
                .as_str()
                .ok_or_else(|| "files must be strings".to_string())?;
            out.push(checked_path(raw, ctx)?);
        }
    } else if let Some(raw) = args.get("path").and_then(Value::as_str)
        && !raw.trim().is_empty()
    {
        out.push(checked_path(raw, ctx)?);
    }
    Ok(out)
}

fn checked_path(raw: &str, ctx: &Context) -> Result<String, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("path must not be empty".into());
    }
    let resolved = ctx.classify(raw).map_err(|e| e.to_string())?;
    if resolved.outside {
        return Err(format!(
            "{} is outside the workspace",
            resolved.path.display()
        ));
    }
    Ok(raw.to_string())
}

fn validate_ref(value: &str) -> Result<(), String> {
    let value = value.trim();
    if value.is_empty() || value.starts_with('-') || !value.chars().all(valid_ref_char) {
        return Err(format!("invalid git ref/name: {value:?}"));
    }
    Ok(())
}

fn valid_ref_char(ch: char) -> bool {
    ch.is_alphanumeric() || ".-_/@^~".contains(ch)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ctx() -> (tempfile::TempDir, Context) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        (dir, Context::new(root))
    }

    #[test]
    fn git_mutating_rules() {
        assert!(!crate::tools::is_mutating(
            "git",
            Some(&json!({"action":"status"}))
        ));
        assert!(!crate::tools::is_mutating(
            "git",
            Some(&json!({"action":"branch"}))
        ));
        assert!(crate::tools::is_mutating(
            "git",
            Some(&json!({"action":"branch","name":"x"}))
        ));
        assert!(crate::tools::is_mutating(
            "git",
            Some(&json!({"action":"commit"}))
        ));
    }

    #[test]
    fn build_status_and_log_args() {
        let (_d, ctx) = ctx();
        let status = build("status", &json!({}), &ctx).unwrap();
        assert_eq!(status[0], "status");
        let log = build("log", &json!({"limit":3}), &ctx).unwrap();
        assert_eq!(log, ["log", "--oneline", "--decorate", "-n", "3"]);
        let err = build("show", &json!({"ref":"--help"}), &ctx).unwrap_err();
        assert!(err.contains("invalid"));
    }

    #[test]
    fn add_blocks_external_paths() {
        let (_d, ctx) = ctx();
        let err = build("add", &json!({"action":"add","files":["../secret"]}), &ctx).unwrap_err();
        assert!(err.contains("outside"));
    }

    #[tokio::test]
    async fn status_runs_in_a_temp_repo() {
        let (_d, ctx) = ctx();
        let ok = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(&ctx.root)
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        if !ok {
            return;
        }
        let out = run(&json!({"action":"status"}), &ctx).await.unwrap();
        assert!(out.ok, "{}", out.detail);
        assert!(out.detail.contains("exit 0"));
    }
}
