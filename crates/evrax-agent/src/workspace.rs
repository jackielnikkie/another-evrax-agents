//! Workspace snapshot + project context file, injected into the system prompt.
//!
//! Pure and synchronous: a few `stat`s plus short, timeout-guarded `git`
//! probes. Every probe is fail-safe — a missing `git` or a non-repo simply
//! drops the corresponding line instead of erroring.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const MANIFESTS: [&str; 10] = [
    "Cargo.toml",
    "package.json",
    "pyproject.toml",
    "go.mod",
    "pom.xml",
    "build.gradle",
    "Gemfile",
    "composer.json",
    "Makefile",
    "Dockerfile",
];

const LOCKFILES: [(&str, &str); 8] = [
    ("Cargo.lock", "cargo"),
    ("pnpm-lock.yaml", "pnpm"),
    ("yarn.lock", "yarn"),
    ("bun.lockb", "bun"),
    ("bun.lock", "bun"),
    ("package-lock.json", "npm"),
    ("uv.lock", "uv"),
    ("poetry.lock", "poetry"),
];

const AGENTS_FILE: &str = "AGENTS.md";
const AGENTS_OVERRIDE: &str = "AGENTS.override.md";
const SOUL_FILE: &str = "SOUL.md";
const CONTEXT_CAP: usize = 8192;
const CONTEXT_MAX_LEVELS: usize = 6;

/// One-line-per-fact workspace snapshot injected into the system prompt.
/// Probed once per session; replay via the cached string.
pub fn snapshot(root: &Path) -> String {
    if !root.is_dir() {
        return String::new();
    }
    let mut out = String::from("Workspace:\n");
    out.push_str(&format!("- Root: {}\n", root.display()));

    if is_git_repo(root) {
        if let Some(line) = branch_line(root) {
            out.push_str(&line);
            out.push('\n');
        }
        if let Some(line) = status_line(root) {
            out.push_str(&line);
            out.push('\n');
        }
        if let Some(line) = commits_line(root) {
            out.push_str(&line);
            out.push('\n');
        }
    }

    if let Some(line) = project_line(root) {
        out.push_str(&line);
        out.push('\n');
    }
    let verify = verify_commands(root);
    if !verify.is_empty() {
        out.push_str(&format!("- Verify: {}\n", verify.join("; ")));
    }
    if let Some((name, _)) = agents_md_chain(root) {
        out.push_str(&format!("- Context files: {name}\n"));
    }
    out
}

/// Identity file: `%APPDATA%\evrax\SOUL.md` (global user) first, else
/// `<root>/SOUL.md`. Capped like the other context files.
pub fn soul_file(root: &Path) -> Option<(String, String)> {
    let appdata = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")));
    soul_file_in(appdata.as_deref(), root)
}

fn soul_file_in(appdata: Option<&Path>, root: &Path) -> Option<(String, String)> {
    if let Some(path) = appdata.map(|dir| dir.join("evrax").join(SOUL_FILE))
        && path.is_file()
    {
        return read_capped(SOUL_FILE, &path);
    }
    let local = root.join(SOUL_FILE);
    if local.is_file() {
        return read_capped(SOUL_FILE, &local);
    }
    None
}

/// `AGENTS.md` chain from the git root down to `root` (`AGENTS.override.md`
/// wins per directory). CLAUDE.md and .cursorrules are deliberately ignored:
/// evrax does not use the Claude conventions. The chain is concatenated, each
/// file capped, with a `## <path>` heading so the model can tell them apart.
pub fn agents_md_chain(root: &Path) -> Option<(String, String)> {
    let base = if root.is_absolute() {
        root.to_path_buf()
    } else {
        std::env::current_dir().ok()?.join(root)
    };
    let start = git_root(&base).unwrap_or_else(|| base.clone());

    // Collect the chain root → base, then read top-down.
    let mut dirs: Vec<PathBuf> = Vec::new();
    let mut dir = Some(base.as_path());
    while let Some(current) = dir {
        dirs.push(current.to_path_buf());
        if same_dir(current, &start) {
            break;
        }
        dir = current.parent();
    }
    dirs.reverse();

    let mut parts: Vec<String> = Vec::new();
    let mut names: Vec<String> = Vec::new();
    let chain_root = dirs.first().cloned().unwrap_or_else(|| base.clone());
    for d in &dirs {
        let file = if d.join(AGENTS_OVERRIDE).is_file() {
            AGENTS_OVERRIDE
        } else if d.join(AGENTS_FILE).is_file() {
            AGENTS_FILE
        } else {
            continue;
        };
        let path = d.join(file);
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        let mut content = raw;
        if content.len() > CONTEXT_CAP {
            truncate_at(&mut content, CONTEXT_CAP);
            content.push_str("\n\n[... truncated ...]");
        }
        let rel = path
            .strip_prefix(&chain_root)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| file.to_string());
        names.push(rel.clone());
        parts.push(format!("## {rel}\n{content}"));
    }
    if parts.is_empty() {
        return None;
    }
    Some((names.join(", "), parts.join("\n\n")))
}

/// Nearest `AGENTS.md` walking up from `root` (nearest first), capped. Kept for
/// callers that want a single file rather than the full chain.
pub fn context_file(root: &Path) -> Option<(String, String)> {
    let base = if root.is_absolute() {
        root.to_path_buf()
    } else {
        std::env::current_dir().ok()?.join(root)
    };
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from);
    let temp = std::env::temp_dir();

    let mut dir = base;
    for _ in 0..CONTEXT_MAX_LEVELS {
        let path = dir.join(AGENTS_FILE);
        if path.is_file() {
            return read_capped(AGENTS_FILE, &path);
        }
        let Some(parent) = dir.parent() else { break };
        if is_home_or_temp(parent, home.as_deref(), &temp) {
            break;
        }
        dir = parent.to_path_buf();
    }
    None
}

fn read_capped(name: &str, path: &Path) -> Option<(String, String)> {
    let mut content = std::fs::read_to_string(path).ok()?;
    if content.len() > CONTEXT_CAP {
        truncate_at(&mut content, CONTEXT_CAP);
        content.push_str("\n\n[... truncated ...]");
    }
    Some((name.to_string(), content))
}

/// Git top-level of `root`, or `None` when `root` is not inside a repo.
fn git_root(root: &Path) -> Option<PathBuf> {
    let out = run_git(root, &["rev-parse", "--show-toplevel"])?;
    let line = out.trim();
    if line.is_empty() {
        return None;
    }
    Some(PathBuf::from(line))
}

fn is_home_or_temp(dir: &Path, home: Option<&Path>, temp: &Path) -> bool {
    home.is_some_and(|h| same_dir(dir, h)) || same_dir(dir, temp)
}

fn same_dir(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

fn truncate_at(s: &mut String, cap: usize) {
    if s.len() <= cap {
        return;
    }
    let mut end = cap;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s.truncate(end);
}

fn is_git_repo(root: &Path) -> bool {
    run_git(root, &["rev-parse", "--is-inside-work-tree"]).is_some_and(|out| out.trim() == "true")
}

fn branch_line(root: &Path) -> Option<String> {
    let branch = run_git(root, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    let branch = branch.trim();
    if branch.is_empty() {
        return None;
    }
    if branch == "HEAD" {
        return Some("- Branch: (detached HEAD)".to_string());
    }
    let mut line = format!("- Branch: {branch}");
    if let Some(up) = run_git(
        root,
        &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"],
    ) {
        let up = up.trim();
        if !up.is_empty() {
            let mut arrow = format!(" \u{2192} {up}");
            if let Some(counts) = run_git(
                root,
                &["rev-list", "--left-right", "--count", "@{u}...HEAD"],
            ) {
                let mut it = counts.split_whitespace();
                let behind = it.next().and_then(|s| s.parse::<u64>().ok());
                let ahead = it.next().and_then(|s| s.parse::<u64>().ok());
                let mut parts = Vec::new();
                if ahead.is_some_and(|a| a > 0) {
                    parts.push(format!("ahead {}", ahead.unwrap()));
                }
                if behind.is_some_and(|b| b > 0) {
                    parts.push(format!("behind {}", behind.unwrap()));
                }
                if !parts.is_empty() {
                    arrow.push_str(&format!(" ({})", parts.join(", ")));
                }
            }
            line.push_str(&arrow);
        }
    }
    Some(line)
}

fn status_line(root: &Path) -> Option<String> {
    let out = run_git(root, &["status", "--porcelain"])?;
    let mut modified = 0usize;
    let mut untracked = 0usize;
    for line in out.lines() {
        if line.trim().is_empty() {
            continue;
        }
        if line.starts_with("??") {
            untracked += 1;
        } else {
            modified += 1;
        }
    }
    let mut parts = Vec::new();
    if modified > 0 {
        parts.push(format!("{modified} modified"));
    }
    if untracked > 0 {
        parts.push(format!("{untracked} untracked"));
    }
    if parts.is_empty() {
        Some("- Status: clean".to_string())
    } else {
        Some(format!("- Status: {}", parts.join(", ")))
    }
}

fn commits_line(root: &Path) -> Option<String> {
    let out = run_git(root, &["log", "--oneline", "-3", "--no-decorate"])?;
    let mut block = String::from("- Recent commits:");
    let mut any = false;
    for line in out.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        any = true;
        block.push_str(&format!("\n    {line}"));
    }
    if any { Some(block) } else { None }
}

fn project_line(root: &Path) -> Option<String> {
    let manifests: Vec<&str> = MANIFESTS
        .iter()
        .copied()
        .filter(|name| root.join(name).is_file())
        .collect();
    if manifests.is_empty() {
        return None;
    }
    let pms: Vec<&str> = LOCKFILES
        .iter()
        .filter(|(lock, _)| root.join(lock).is_file())
        .map(|(_, pm)| *pm)
        .collect();
    let mut line = format!("- Project: {}", manifests.join(", "));
    if !pms.is_empty() {
        line.push_str(&format!(" (package-manager: {})", pms.join("/")));
    }
    Some(line)
}

fn package_manager(root: &Path) -> &'static str {
    LOCKFILES
        .iter()
        .find(|(lock, _)| root.join(lock).is_file())
        .map(|(_, pm)| *pm)
        .unwrap_or("npm")
}

fn verify_commands(root: &Path) -> Vec<String> {
    let mut cmds: Vec<String> = Vec::new();
    if root.join("Cargo.toml").is_file() {
        cmds.push("cargo test".to_string());
        cmds.push("cargo clippy".to_string());
    }
    if root.join("package.json").is_file() {
        let pm = package_manager(root);
        for target in ["test", "lint", "build", "typecheck", "check"] {
            if package_script(root, target) {
                cmds.push(format!("{pm} run {target}"));
            }
        }
    }
    for makefile in ["Makefile", "makefile"] {
        if root.join(makefile).is_file() {
            for target in make_targets(root, makefile) {
                cmds.push(format!("make {target}"));
            }
            break;
        }
    }
    if root.join("pyproject.toml").is_file() || root.join("pytest.ini").is_file() {
        cmds.push("pytest".to_string());
    }
    cmds.truncate(6);
    cmds
}

fn package_script(root: &Path, name: &str) -> bool {
    let Ok(text) = std::fs::read_to_string(root.join("package.json")) else {
        return false;
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
        return false;
    };
    json.get("scripts")
        .and_then(serde_json::Value::as_object)
        .is_some_and(|scripts| scripts.contains_key(name))
}

fn make_targets(root: &Path, name: &str) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(root.join(name)) else {
        return Vec::new();
    };
    let Ok(re) = regex::Regex::new(r"(?m)^(test|lint|check|build)\s*:") else {
        return Vec::new();
    };
    re.captures_iter(&text)
        .filter_map(|caps| caps.get(1).map(|m| m.as_str().to_string()))
        .collect()
}

/// Run `git -C <root> <args>` with a hard 2s deadline. Returns trimmed stdout
/// only on a successful exit; any failure (missing git, non-repo, timeout)
/// yields `None`.
fn run_git(root: &Path, args: &[&str]) -> Option<String> {
    use std::io::Read;
    use std::process::{Command, Stdio};

    let mut child = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return None;
                }
                let mut buf = String::new();
                if let Some(mut out) = child.stdout.take() {
                    out.read_to_string(&mut buf).ok()?;
                }
                return Some(buf);
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(_) => return None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git_init(dir: &Path) {
        let _ = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .arg("init")
            .output();
    }

    #[test]
    fn soul_md_loaded_from_appdata_then_workspace() {
        let appdata = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(appdata.path().join("evrax")).unwrap();
        std::fs::write(appdata.path().join("evrax").join("SOUL.md"), "global soul").unwrap();
        std::fs::write(workspace.path().join("SOUL.md"), "local soul").unwrap();

        // AppData wins when present.
        let (name, content) = soul_file_in(Some(appdata.path()), workspace.path()).unwrap();
        assert_eq!(name, "SOUL.md");
        assert_eq!(content, "global soul");

        // Without the AppData copy, the workspace file is used.
        let (_, content) = soul_file_in(None, workspace.path()).unwrap();
        assert_eq!(content, "local soul");

        // Neither → None.
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(soul_file_in(None, empty.path()), None);
    }

    #[test]
    fn agents_md_chain_root_to_cwd() {
        let root = tempfile::tempdir().unwrap();
        git_init(root.path());
        let nested = root.path().join("crates").join("app");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(root.path().join("AGENTS.md"), "root agents").unwrap();
        std::fs::write(nested.join("AGENTS.md"), "nested agents").unwrap();

        let (_name, content) = agents_md_chain(&nested).unwrap();
        let root_pos = content.find("root agents").expect("root file in chain");
        let nested_pos = content.find("nested agents").expect("nested file in chain");
        assert!(
            root_pos < nested_pos,
            "root must come before the child:\n{content}"
        );
    }

    #[test]
    fn agents_override_wins_per_directory() {
        let root = tempfile::tempdir().unwrap();
        git_init(root.path());
        std::fs::write(root.path().join("AGENTS.md"), "base").unwrap();
        std::fs::write(root.path().join("AGENTS.override.md"), "override wins").unwrap();

        let (_name, content) = agents_md_chain(root.path()).unwrap();
        assert!(content.contains("override wins"), "{content}");
        assert!(!content.contains("## AGENTS.md"), "{content}");
    }

    #[test]
    fn claude_and_cursorrules_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("CLAUDE.md"), "claude rules").unwrap();
        std::fs::write(dir.path().join(".cursorrules"), "cursor rules").unwrap();
        assert_eq!(agents_md_chain(dir.path()), None);
        assert_eq!(context_file(dir.path()), None);

        std::fs::write(dir.path().join("AGENTS.md"), "agents rules").unwrap();
        let (_name, content) = agents_md_chain(dir.path()).unwrap();
        assert!(content.contains("agents rules"));
        assert!(!content.contains("claude rules"));
        assert!(!content.contains("cursor rules"));
    }

    #[test]
    fn context_files_are_capped_with_marker() {
        let dir = tempfile::tempdir().unwrap();
        let big = "x".repeat(CONTEXT_CAP + 50);
        std::fs::write(dir.path().join("AGENTS.md"), &big).unwrap();
        let (_name, content) = context_file(dir.path()).unwrap();
        assert!(
            content.ends_with("[... truncated ...]"),
            "len {}",
            content.len()
        );
    }
}
