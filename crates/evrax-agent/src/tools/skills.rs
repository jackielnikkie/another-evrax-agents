use serde_json::Value;

use crate::catalog::Catalog;

use super::{Context, ToolOutcome, arg_str};

const MAX_SKILL_CHARS: usize = 20_000;

pub(super) fn list(_ctx: &Context) -> Result<ToolOutcome, String> {
    let catalog = Catalog::load();
    let mut out = format!("{} skills\n", catalog.skills.len());
    for skill in &catalog.skills {
        let desc = skill.description.trim();
        if desc.is_empty() {
            out.push_str(&format!("{}\n", skill.name));
        } else {
            out.push_str(&format!("{} — {}\n", skill.name, desc));
        }
    }
    Ok(ToolOutcome {
        ok: true,
        detail: out.trim_end().to_string(),
        diff: None,
    })
}

pub(super) fn view(args: &Value, _ctx: &Context) -> Result<ToolOutcome, String> {
    let name = arg_str(args, "name")?;
    let needle = name.trim().to_lowercase();
    if needle.is_empty() {
        return Err("`name` must not be empty".into());
    }
    let catalog = Catalog::load();
    if let Some(skill) = catalog
        .skills
        .iter()
        .find(|skill| skill.name.to_lowercase() == needle)
    {
        return Ok(render(skill.name.clone(), skill.content.clone()));
    }
    let matches: Vec<_> = catalog
        .skills
        .iter()
        .filter(|skill| skill.name.to_lowercase().contains(&needle))
        .collect();
    match matches.as_slice() {
        [] => Err(format!("skill '{name}' not found")),
        [skill] => Ok(render(skill.name.clone(), skill.content.clone())),
        many => Err(format!(
            "skill name '{name}' is ambiguous ({} matches)",
            many.len()
        )),
    }
}

fn render(name: String, content: String) -> ToolOutcome {
    let body = if content.chars().count() > MAX_SKILL_CHARS {
        let mut cut: String = content.chars().take(MAX_SKILL_CHARS).collect();
        cut.push_str("\n[... truncated]");
        cut
    } else {
        content
    };
    ToolOutcome {
        ok: true,
        detail: format!("# {name}\n\n{body}"),
        diff: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::Context;

    fn ctx() -> (tempfile::TempDir, Context) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        (dir, Context::new(root))
    }

    #[test]
    fn list_returns_embedded_skills() {
        let (_d, c) = ctx();
        let out = list(&c).unwrap();
        assert!(out.ok);
        assert!(
            out.detail.contains("humanizer") || out.detail.contains("systematic-debugging"),
            "{}",
            out.detail
        );
    }

    #[test]
    fn view_finds_skill_by_name() {
        let (_d, c) = ctx();
        let out = view(&serde_json::json!({"name":"HUMANIZER"}), &c).unwrap();
        assert!(out.ok);
        assert!(
            out.detail.to_lowercase().contains("humanizer"),
            "{}",
            out.detail
        );
    }

    #[test]
    fn view_unknown_errors() {
        let (_d, c) = ctx();
        let err = view(&serde_json::json!({"name":"no-such-skill-xyz"}), &c).unwrap_err();
        assert!(err.contains("not found"), "{err}");
    }
}
