use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    let manifest = env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    let skill_root = Path::new(&manifest)
        .parent()
        .and_then(|path| path.parent())
        .unwrap_or_else(|| Path::new("."))
        .join("skill-evrax");

    println!("cargo:rerun-if-changed={}", skill_root.display());

    let skills = collect_skills(&skill_root);
    println!(
        "cargo:warning=evrax: embedded {} SKILL.md from {}",
        skills.len(),
        skill_root.display()
    );

    let mut out = String::new();
    out.push_str("pub static EMBEDDED_SKILLS: &[(&str, &str)] = &[\n");
    for path in &skills {
        let label = path
            .strip_prefix(&skill_root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        let abs = path.canonicalize().unwrap_or_else(|_| path.clone());
        let abs = abs.to_string_lossy().replace('\\', "/");
        println!("cargo:rerun-if-changed={}", path.display());
        out.push_str(&format!(
            "    (\"{}\", include_str!(\"{}\")),\n",
            rust_escape(&label),
            rust_escape(&abs)
        ));
    }
    out.push_str("];\n");

    let out_dir = env::var("OUT_DIR").unwrap_or_default();
    let dest = Path::new(&out_dir).join("embedded_skills.rs");
    fs::write(&dest, out).expect("write embedded_skills.rs");
}

fn collect_skills(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if !root.is_dir() {
        return files;
    }
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        if is_skipped(&dir) {
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
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                stack.push((entry.path(), depth + 1));
            }
        }
    }
    files.sort();
    files
}

fn is_skipped(dir: &Path) -> bool {
    dir.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == ".git" || name == "bin")
}

fn rust_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}
