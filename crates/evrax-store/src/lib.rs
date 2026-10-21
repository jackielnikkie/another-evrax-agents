//! On-disk project + session storage shared by the agent and the TUI.
//!
//! Layout: `%LOCALAPPDATA%\evrax\projects\<slug>-<hash16>\`
//!   - `project.json`
//!   - `sessions\<id>\meta.json`
//!   - `sessions\<id>\transcript.json`
//!   - `sessions\<id>\history.json`

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Sessions kept per project; the coldest are pruned.
pub const MAX_SESSIONS: usize = 50;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionMeta {
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub title_pending: bool,
    #[serde(default)]
    pub created_at: u64,
    #[serde(default)]
    pub updated_at: u64,
}

impl SessionMeta {
    pub fn new(id: impl Into<String>, title: impl Into<String>, mode: impl Into<String>) -> Self {
        let now = now_ms();
        Self {
            id: id.into(),
            title: title.into(),
            mode: mode.into(),
            title_pending: false,
            created_at: now,
            updated_at: now,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProjectFile {
    root: String,
    name: String,
}

pub struct ProjectStore {
    root: PathBuf,
    dir: PathBuf,
}

impl ProjectStore {
    /// Opens (and creates) the store for the project containing `cwd`.
    pub fn open(cwd: &Path) -> Result<Self> {
        Self::open_in(cwd, &default_projects_dir()?)
    }

    /// Same as [`Self::open`] with an explicit projects directory (tests).
    pub fn open_in(cwd: &Path, projects_dir: &Path) -> Result<Self> {
        let root = project_root(cwd);
        let (slug, hash) = slug_and_hash(&root);
        let dir = projects_dir.join(format!("{slug}-{hash}"));
        fs::create_dir_all(dir.join("sessions"))
            .with_context(|| format!("create store dir {}", dir.display()))?;

        let file_path = dir.join("project.json");
        let root_text = root.to_string_lossy().to_string();
        if file_path.is_file() {
            let existing: ProjectFile = read_json_file(&file_path)?;
            if !same_path(&existing.root, &root_text) {
                bail!(
                    "project store mismatch: {} belongs to {}",
                    dir.display(),
                    existing.root
                );
            }
        } else {
            write_json_file(
                &file_path,
                &ProjectFile {
                    root: root_text.clone(),
                    name: root
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default(),
                },
            )?;
        }

        Ok(Self { root, dir })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn list(&self) -> Result<Vec<SessionMeta>> {
        let sessions = self.dir.join("sessions");
        let mut out = Vec::new();
        if !sessions.is_dir() {
            return Ok(out);
        }
        for entry in fs::read_dir(&sessions)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let meta = entry.path().join("meta.json");
            if meta.is_file()
                && let Ok(meta) = read_json_file::<SessionMeta>(&meta)
            {
                out.push(meta);
            }
        }
        out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(out)
    }

    pub fn load_meta(&self, id: &str) -> Result<Option<SessionMeta>> {
        let path = self.session_dir(id)?.join("meta.json");
        if !path.is_file() {
            return Ok(None);
        }
        Ok(Some(read_json_file(&path)?))
    }

    pub fn save_meta(&self, meta: &SessionMeta) -> Result<()> {
        let path = self.session_dir(&meta.id)?.join("meta.json");
        write_json_file(&path, meta)?;
        self.prune(MAX_SESSIONS)?;
        Ok(())
    }

    pub fn read_json<T: DeserializeOwned>(&self, id: &str, file: &str) -> Result<Option<T>> {
        let path = self.session_dir(id)?.join(file);
        if !path.is_file() {
            return Ok(None);
        }
        Ok(Some(read_json_file(&path)?))
    }

    pub fn write_json<T: Serialize>(&self, id: &str, file: &str, value: &T) -> Result<()> {
        let path = self.session_dir(id)?.join(file);
        write_json_file(&path, value)
    }

    pub fn delete(&self, id: &str) -> Result<()> {
        let dir = self.session_dir(id)?;
        if dir.is_dir() {
            fs::remove_dir_all(&dir).with_context(|| format!("delete {}", dir.display()))?;
        }
        Ok(())
    }

    /// Removes the coldest sessions beyond `keep`; returns how many were deleted.
    pub fn prune(&self, keep: usize) -> Result<usize> {
        let mut metas = self.list()?;
        if metas.len() <= keep {
            return Ok(0);
        }
        let mut removed = 0;
        for meta in metas.split_off(keep) {
            self.delete(&meta.id)?;
            removed += 1;
        }
        Ok(removed)
    }

    fn session_dir(&self, id: &str) -> Result<PathBuf> {
        validate_id(id)?;
        let dir = self.dir.join("sessions").join(id);
        if let Some(parent) = dir.parent() {
            fs::create_dir_all(parent)?;
        }
        Ok(dir)
    }
}

/// Walks up from `cwd` to the nearest `.git` entry, else returns `cwd`.
pub fn project_root(cwd: &Path) -> PathBuf {
    let start = fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
    for dir in start.ancestors() {
        if dir.join(".git").exists() {
            return dir.to_path_buf();
        }
    }
    start
}

fn default_projects_dir() -> Result<PathBuf> {
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        return Ok(PathBuf::from(local).join("evrax").join("projects"));
    }
    if let Some(home) = std::env::var_os("HOME") {
        return Ok(PathBuf::from(home)
            .join(".local")
            .join("share")
            .join("evrax")
            .join("projects"));
    }
    bail!("no LOCALAPPDATA or HOME to place the evrax store")
}

fn slug_and_hash(root: &Path) -> (String, String) {
    let raw = root
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let mut slug: String = raw
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    while slug.contains("--") {
        slug = slug.replace("--", "-");
    }
    let slug = slug.trim_matches('-');
    let slug = if slug.is_empty() { "project" } else { slug };
    let slug: String = slug.chars().take(24).collect();

    let mut hasher = Sha256::new();
    hasher.update(root.to_string_lossy().to_lowercase().as_bytes());
    let digest = hasher.finalize();
    let hash = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
    (slug, hash)
}

fn validate_id(id: &str) -> Result<()> {
    if id.is_empty() || id.len() > 128 {
        bail!("invalid session id");
    }
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        bail!("invalid session id: {id}");
    }
    Ok(())
}

fn same_path(a: &str, b: &str) -> bool {
    if cfg!(windows) {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}

fn read_json_file<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let text = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))
}

fn write_json_file<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_vec_pretty(value)?;
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, text).with_context(|| format!("write {}", tmp.display()))?;
    if path.exists() {
        fs::remove_file(path).with_context(|| format!("replace {}", path.display()))?;
    }
    fs::rename(&tmp, path).with_context(|| format!("commit {}", path.display()))?;
    Ok(())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_project() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join(".git")).unwrap();
        let nested = dir.path().join("src").join("deep");
        fs::create_dir_all(&nested).unwrap();
        (dir, nested)
    }

    #[test]
    fn project_root_walks_up_to_git() {
        let (dir, nested) = temp_project();
        assert_eq!(project_root(&nested), fs::canonicalize(dir.path()).unwrap());
    }

    #[test]
    fn hash_is_stable_and_project_scoped() {
        let a = Path::new("C:/work/alpha");
        let b = Path::new("C:/work/beta");
        assert_eq!(slug_and_hash(a), slug_and_hash(a));
        assert_ne!(slug_and_hash(a).1, slug_and_hash(b).1);
        assert_eq!(slug_and_hash(a).0, "alpha");
    }

    #[test]
    fn meta_and_payloads_roundtrip() {
        let (dir, nested) = temp_project();
        let store = ProjectStore::open_in(&nested, &dir.path().join("data")).unwrap();
        let meta = SessionMeta::new("session-1-1", "hello", "build");
        store.save_meta(&meta).unwrap();
        assert_eq!(store.load_meta("session-1-1").unwrap().unwrap(), meta);

        let payload = serde_json::json!([{"kind": "user", "text": "hi"}]);
        store
            .write_json("session-1-1", "transcript.json", &payload)
            .unwrap();
        let back: serde_json::Value = store
            .read_json("session-1-1", "transcript.json")
            .unwrap()
            .unwrap();
        assert_eq!(back, payload);

        let listed = store.list().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].title, "hello");
    }

    #[test]
    fn rejects_path_traversal_ids() {
        let (dir, nested) = temp_project();
        let store = ProjectStore::open_in(&nested, &dir.path().join("data")).unwrap();
        assert!(store.load_meta("../escape").is_err());
        assert!(store.load_meta("a/b").is_err());
        assert!(store.load_meta("").is_err());
    }

    #[test]
    fn prune_keeps_newest() {
        let (dir, nested) = temp_project();
        let store = ProjectStore::open_in(&nested, &dir.path().join("data")).unwrap();
        for i in 0..5 {
            let mut meta = SessionMeta::new(format!("session-{i}-1"), format!("s{i}"), "build");
            meta.updated_at = i as u64;
            store.save_meta(&meta).unwrap();
        }
        let removed = store.prune(2).unwrap();
        assert_eq!(removed, 3);
        let left: Vec<String> = store.list().unwrap().into_iter().map(|m| m.id).collect();
        assert_eq!(left, vec!["session-4-1", "session-3-1"]);
    }

    #[test]
    fn project_mismatch_is_rejected() {
        let (dir, nested) = temp_project();
        let projects = dir.path().join("data");
        let store = ProjectStore::open_in(&nested, &projects).unwrap();
        let file = store.dir().join("project.json");
        fs::write(&file, r#"{"root":"C:/elsewhere","name":"x"}"#).unwrap();
        assert!(ProjectStore::open_in(&nested, &projects).is_err());
    }

    #[test]
    fn rewrite_replaces_existing_payload() {
        let (dir, nested) = temp_project();
        let store = ProjectStore::open_in(&nested, &dir.path().join("data")).unwrap();
        store
            .write_json("session-1-1", "history.json", &vec!["a"])
            .unwrap();
        store
            .write_json("session-1-1", "history.json", &vec!["b"])
            .unwrap();
        let back: Vec<String> = store.read_json("session-1-1", "history.json").unwrap().unwrap();
        assert_eq!(back, vec!["b"]);
    }
}
