use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

pub use crate::tools::ToolInfo;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillInfo {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpInfo {
    pub name: String,
    pub enabled: bool,
    pub kind: String,
}

#[derive(Clone, Debug)]
pub struct Catalog {
    pub skills: Vec<SkillInfo>,
    pub mcps: Vec<McpInfo>,
    pub tools: Vec<ToolInfo>,
}

impl Default for Catalog {
    fn default() -> Self {
        Self {
            skills: Vec::new(),
            mcps: Vec::new(),
            tools: crate::tools::catalog(),
        }
    }
}

impl Catalog {
    pub fn load() -> Self {
        let skill_dirs = env::var_os("EVRAX_SKILLS_DIR")
            .map(|dir| vec![PathBuf::from(dir)])
            .unwrap_or_else(default_skill_dirs);
        let mcp_path = env::var_os("EVRAX_MCP_CONFIG")
            .map(PathBuf::from)
            .or_else(default_mcp_path);
        Self::load_from(&skill_dirs, mcp_path.as_deref())
    }

    pub fn load_from(skill_dirs: &[PathBuf], mcp_path: Option<&Path>) -> Self {
        Self {
            skills: discover_skills(skill_dirs),
            mcps: load_mcps(mcp_path),
            tools: crate::tools::catalog(),
        }
    }
}

fn default_skill_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|path| path.parent())
        .unwrap_or_else(|| Path::new("."));
    dirs.push(repo.join("skill-evrax"));
    if let Some(appdata) = env::var_os("APPDATA") {
        dirs.push(PathBuf::from(appdata).join("evrax").join("skills"));
    }
    dirs
}

fn default_mcp_path() -> Option<PathBuf> {
    env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .map(|dir| dir.join("evrax").join("mcp.json"))
}

fn discover_skills(dirs: &[PathBuf]) -> Vec<SkillInfo> {
    let mut found: BTreeMap<String, SkillInfo> = BTreeMap::new();
    for dir in dirs {
        for skill_path in find_skill_files(dir) {
            if let Some(skill) = read_skill(&skill_path) {
                found.entry(skill.name.to_lowercase()).or_insert(skill);
            }
        }
    }
    found.into_values().collect()
}

fn find_skill_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        if dir.file_name().and_then(|name| name.to_str()) == Some(".git") {
            continue;
        }
        let skill = dir.join("SKILL.md");
        if skill.is_file() {
            files.push(skill);
        }
        if depth >= 6 {
            continue;
        }
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        let mut subdirs = Vec::new();
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                subdirs.push(entry.path());
            }
        }
        subdirs.sort();
        for path in subdirs.into_iter().rev() {
            stack.push((path, depth + 1));
        }
    }
    files
}

fn read_skill(path: &Path) -> Option<SkillInfo> {
    let text = fs::read_to_string(path).ok()?;
    let (name, description) = parse_frontmatter(&text);
    let fallback = path.parent()?.file_name()?.to_string_lossy().to_string();
    let name = name.unwrap_or(fallback);
    if name.trim().is_empty() {
        return None;
    }
    Some(SkillInfo {
        name,
        description: description.unwrap_or_default(),
        path: path.to_path_buf(),
    })
}

fn parse_frontmatter(text: &str) -> (Option<String>, Option<String>) {
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some("---") {
        return (None, None);
    }
    let mut name = None;
    let mut description = None;
    for line in lines {
        let line = line.trim();
        if line == "---" {
            break;
        }
        if name.is_none()
            && let Some(value) = line.strip_prefix("name:")
        {
            name = Some(clean(value));
            continue;
        }
        if description.is_none()
            && let Some(value) = line.strip_prefix("description:")
        {
            description = Some(clean(value));
        }
    }
    (name, description)
}

fn clean(value: &str) -> String {
    value
        .trim()
        .trim_matches(|ch| ch == '"' || ch == '\'')
        .to_string()
}

#[derive(Debug, Default, Deserialize)]
struct McpDoc {
    #[serde(default)]
    servers: BTreeMap<String, McpEntry>,
    #[serde(default)]
    mcp: BTreeMap<String, McpEntry>,
}

#[derive(Debug, Default, Deserialize)]
struct McpEntry {
    enabled: Option<bool>,
    #[serde(rename = "type")]
    kind: Option<String>,
}

fn load_mcps(path: Option<&Path>) -> Vec<McpInfo> {
    let Some(path) = path else {
        return Vec::new();
    };
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(doc) = serde_json::from_str::<McpDoc>(&text) else {
        return Vec::new();
    };
    let mut entries = doc.servers;
    entries.extend(doc.mcp);
    entries
        .into_iter()
        .map(|(name, entry)| McpInfo {
            name,
            enabled: entry.enabled.unwrap_or(true),
            kind: entry.kind.unwrap_or_else(|| "local".to_string()),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn tool_catalog_exposes_the_builtin_tools() {
        let tools = crate::tools::catalog();
        let names: Vec<&str> = tools.iter().map(|tool| tool.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "read",
                "list",
                "grep",
                "write",
                "edit",
                "shell",
                "web_fetch",
                "todo",
                "git",
                "ast_grep",
                "evrax-graph",
                "subagent"
            ]
        );
        assert_eq!(tools[0].risk, crate::tools::Risk::Read);
        assert_eq!(tools[3].risk, crate::tools::Risk::Mutate);
    }

    #[test]
    fn skills_are_sorted_and_deduplicated() {
        let dir = tempdir().unwrap();
        let first = dir.path().join("zeta");
        let second = dir.path().join("alpha");
        let nested = dir.path().join("group").join("nested");
        let duplicate = dir.path().join("alpha-dup");
        fs::create_dir_all(&first).unwrap();
        fs::create_dir_all(&second).unwrap();
        fs::create_dir_all(&nested).unwrap();
        fs::create_dir_all(&duplicate).unwrap();
        fs::write(
            first.join("SKILL.md"),
            "---\nname: Zeta\ndescription: first\n---\nbody\n",
        )
        .unwrap();
        fs::write(
            second.join("SKILL.md"),
            "---\nname: Alpha\ndescription: \"second\"\n---\nbody\n",
        )
        .unwrap();
        fs::write(
            nested.join("SKILL.md"),
            "---\nname: Nested\ndescription: third\n---\nbody\n",
        )
        .unwrap();
        fs::write(
            duplicate.join("SKILL.md"),
            "---\nname: alpha\ndescription: duplicate\n---\n",
        )
        .unwrap();
        let catalog = Catalog::load_from(&[dir.path().to_path_buf()], None);
        let names: Vec<&str> = catalog
            .skills
            .iter()
            .map(|skill| skill.name.as_str())
            .collect();
        assert_eq!(names, ["Alpha", "Nested", "Zeta"]);
        assert_eq!(catalog.skills[0].description, "second");
    }

    #[test]
    fn mcp_entries_are_loaded_from_the_native_config() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        fs::write(
            &path,
            r#"{"servers":{"serena":{"enabled":true,"type":"local"},"off":{"enabled":false}}}"#,
        )
        .unwrap();
        let catalog = Catalog::load_from(&[], Some(&path));
        assert_eq!(catalog.mcps.len(), 2);
        assert_eq!(catalog.mcps[0].name, "off");
        assert!(!catalog.mcps[0].enabled);
        assert_eq!(catalog.mcps[1].name, "serena");
        assert!(catalog.mcps[1].enabled);
        assert_eq!(catalog.mcps[1].kind, "local");
    }
}
