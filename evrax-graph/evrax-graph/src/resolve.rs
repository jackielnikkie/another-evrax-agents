//! Phase-2 call resolution for Rust: import-aware, path-aware,
//! receiver-type-aware.
//!
//! The MVP resolved every call by bare name (false positives when names
//! collide). This resolver instead:
//!   1. resolves `a::b::c()` / `crate::x::y()` / `self::z()` via module paths,
//!   2. resolves `Type::assoc_fn()` via the type's `impl` blocks,
//!   3. resolves `x.method()` via `let x: Type` bindings (or `self` -> impl type),
//!   4. resolves bare `foo()` via the file's `use` imports,
//!   5. falls back to name-based matching (handled by the caller).
//!
//! Everything is derived from tree-sitter output + the graph itself —
//! no rust-analyzer dependency, keeping the binary light.

use std::collections::HashMap;

use crate::extract::{RawCall, RawLet, RawUse};

/// (qualified_name, id, file, label)
pub type NodeInfo = (String, i64, String, String);

pub struct RustResolver {
    /// rel_path -> module segments, e.g. "src/foo/bar.rs" -> ["foo", "bar"]
    modules: HashMap<String, Vec<String>>,
    /// module segments -> rel_path
    mod_to_file: HashMap<Vec<String>, String>,
    /// file -> Vec<(local_name, full_path)>
    uses: HashMap<String, Vec<(String, String)>>,
    /// func qualified_name -> Vec<(var, type)>
    lets: HashMap<String, Vec<(String, String)>>,
    /// every node in the project (for lookups)
    nodes: Vec<NodeInfo>,
}

impl RustResolver {
    pub fn build(
        rs_files: &[String],
        uses: Vec<RawUse>,
        lets: Vec<RawLet>,
        nodes: Vec<NodeInfo>,
    ) -> Self {
        let mut modules = HashMap::new();
        let mut mod_to_file = HashMap::new();
        for f in rs_files {
            let m = module_path_of(f);
            mod_to_file.insert(m.clone(), f.clone());
            modules.insert(f.clone(), m);
        }
        let mut use_map: HashMap<String, Vec<(String, String)>> = HashMap::new();
        for u in uses {
            use_map
                .entry(u.file)
                .or_default()
                .push((u.local, u.full_path));
        }
        let mut let_map: HashMap<String, Vec<(String, String)>> = HashMap::new();
        for l in lets {
            let_map
                .entry(l.func_qualified)
                .or_default()
                .push((l.var, l.ty));
        }
        Self {
            modules,
            mod_to_file,
            uses: use_map,
            lets: let_map,
            nodes,
        }
    }

    /// Resolve one call. Returns the target node id, or None to fall back
    /// to name-based matching.
    pub fn resolve(&self, call: &RawCall, caller_qn: &str) -> Option<i64> {
        // 1. path calls: `a::b::c()`, `crate::x::y()`, `Foo::assoc()`
        if let Some(path) = &call.path {
            if let Some(id) =
                self.resolve_path(&call.caller_file, caller_qn, path, &call.callee_name)
            {
                return Some(id);
            }
            // path didn't resolve -> fall through to other strategies
        }
        // 2. method call on a receiver: `x.foo()`, `self.foo()`
        if let Some(recv) = &call.receiver {
            if let Some(ty) = self.receiver_type(caller_qn, recv) {
                if let Some(id) = self.find_impl_method(&ty, &call.callee_name) {
                    return Some(id);
                }
            }
        }
        // 3. bare call: consult `use` imports of the caller's file
        if call.receiver.is_none() && call.path.is_none() {
            if let Some(id) = self.resolve_via_use(&call.caller_file, &call.callee_name) {
                return Some(id);
            }
        }
        None
    }

    // -- path resolution --------------------------------------------------

    fn resolve_path(&self, file: &str, caller_qn: &str, path: &str, item: &str) -> Option<i64> {
        let mut segs: Vec<String> = path
            .split("::")
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect();
        if segs.is_empty() {
            return None;
        }
        // drop the item itself if included (`a::b::c` with item `c`)
        if segs.last().map(|s| s.as_str()) == Some(item) {
            segs.pop();
        }
        if segs.is_empty() {
            return None;
        }
        // `Self::assoc` / `Type::assoc`: first segment is a type name
        if segs.len() == 1 {
            let ty = if segs[0] == "Self" {
                impl_type_of(caller_qn)?
            } else {
                segs[0].clone()
            };
            if self.is_type_name(&ty) {
                return self.find_impl_method(&ty, item);
            }
        }
        // module path candidates
        for (mod_segs, it) in self.path_candidates(file, &segs, item) {
            let f = self.mod_to_file.get(&mod_segs)?;
            if let Some(id) = self.find_in_file(f, &it) {
                return Some(id);
            }
        }
        None
    }

    /// Candidate (module_segments, item) pairs for a path, most specific first.
    fn path_candidates(
        &self,
        file: &str,
        segs: &[String],
        item: &str,
    ) -> Vec<(Vec<String>, String)> {
        let cur_mod = self.modules.get(file).cloned().unwrap_or_default();
        let mut segs = segs.to_vec();
        let mut out = Vec::new();
        match segs.first().map(|s| s.as_str()) {
            Some("crate") => {
                segs.remove(0);
                out.push((segs, item.to_string()));
            }
            Some("self") => {
                segs.remove(0);
                let mut m = cur_mod;
                m.extend(segs);
                out.push((m, item.to_string()));
            }
            Some("super") => {
                let mut m = cur_mod;
                while segs.first().map(|s| s.as_str()) == Some("super") {
                    segs.remove(0);
                    m.pop();
                }
                m.extend(segs);
                out.push((m, item.to_string()));
            }
            _ => {
                let mut m = cur_mod.clone();
                m.extend(segs.iter().cloned());
                out.push((m, item.to_string())); // current module first
                out.push((segs, item.to_string())); // then crate root
            }
        }
        out
    }

    // -- receiver types ----------------------------------------------------

    /// Type of a receiver expression: `self` -> enclosing impl type,
    /// otherwise looked up in the function's `let` bindings.
    fn receiver_type(&self, caller_qn: &str, recv: &str) -> Option<String> {
        let r = recv.trim();
        if r == "self" || r == "Self" {
            return impl_type_of(caller_qn);
        }
        // plain variable: `let x: Foo`
        let first = r.split(['.', '(']).next()?.trim();
        self.lets
            .get(caller_qn)?
            .iter()
            .find(|(var, _)| var == first)
            .map(|(_, ty)| ty.clone())
    }

    /// Find method `name` inside `impl {ty}` blocks (qualified `::impl Ty::`).
    fn find_impl_method(&self, ty: &str, name: &str) -> Option<i64> {
        let marker = format!("::impl {ty}::");
        // also match generic impls recorded as `impl Foo`
        let ty_base = ty.split('<').next().unwrap_or(ty).trim();
        let marker2 = format!("::impl {ty_base}::");
        self.nodes
            .iter()
            .find(|(qn, _, _, label)| {
                (label == "Method" || label == "Function")
                    && qn.rsplit("::").next() == Some(name)
                    && (qn.contains(&marker) || qn.contains(&marker2))
            })
            .map(|(_, id, _, _)| *id)
    }

    // -- `use` imports ------------------------------------------------------

    fn resolve_via_use(&self, file: &str, name: &str) -> Option<i64> {
        let uses = self.uses.get(file)?;
        for (local, full_path) in uses {
            // Split full_path into (module segments, item). For a direct
            // import `use a::b::c` (possibly aliased) the path includes the
            // item; for a glob `use a::b::*` the item is the called name.
            let (mod_segs, item): (Vec<String>, String) = if local == name {
                let mut segs = split_segs(full_path);
                if segs.is_empty() {
                    continue;
                }
                let it = segs.pop().unwrap();
                (segs, it)
            } else if local == "*" {
                (split_segs(full_path), name.to_string())
            } else {
                continue;
            };
            for (m, it) in self.path_candidates(file, &mod_segs, &item) {
                let f = self.mod_to_file.get(&m)?;
                if let Some(id) = self.find_in_file(f, &it) {
                    return Some(id);
                }
            }
        }
        None
    }

    // -- node helpers --------------------------------------------------------

    fn is_type_name(&self, name: &str) -> bool {
        self.nodes
            .iter()
            .any(|(qn, _, _, label)| is_type_label(label) && qn.rsplit("::").next() == Some(name))
    }

    /// Find a node called `name` inside file `f`, preferring callables/types.
    fn find_in_file(&self, f: &str, name: &str) -> Option<i64> {
        let mut fallback = None;
        for (qn, id, file, label) in &self.nodes {
            if file != f || qn.rsplit("::").next() != Some(name) {
                continue;
            }
            if label == "Function"
                || label == "Method"
                || is_type_label(label)
                || label == "Constant"
            {
                return Some(*id);
            }
            if fallback.is_none() {
                fallback = Some(*id);
            }
        }
        fallback
    }
}

fn is_type_label(label: &str) -> bool {
    matches!(
        label,
        "Class" | "Struct" | "Enum" | "Trait" | "Interface" | "Type"
    )
}

fn split_segs(path: &str) -> Vec<String> {
    path.split("::")
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

/// Module segments for a `.rs` file relative to the crate root.
/// `src/main.rs` -> [], `src/foo.rs` -> ["foo"], `src/foo/bar.rs` -> ["foo","bar"].
fn module_path_of(rel: &str) -> Vec<String> {
    let p = rel.strip_prefix("src/").unwrap_or(rel);
    let mut segs: Vec<&str> = p.split('/').collect();
    let file = segs.pop().unwrap_or("");
    let stem = file.strip_suffix(".rs").unwrap_or(file);
    let mut mods: Vec<String> = segs.iter().map(|s| s.to_string()).collect();
    match stem {
        "main" | "lib" | "mod" => {}
        s => mods.push(s.to_string()),
    }
    mods
}

/// `src/x.rs::impl Foo::method` -> `Foo`.
fn impl_type_of(caller_qn: &str) -> Option<String> {
    let i = caller_qn.find("::impl ")?;
    let rest = &caller_qn[i + 7..];
    let end = rest.find("::")?;
    let ty = rest[..end].trim();
    // strip generics: `Foo<T>` -> `Foo`
    Some(ty.split('<').next().unwrap_or(ty).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_paths() {
        assert_eq!(module_path_of("src/main.rs"), Vec::<String>::new());
        assert_eq!(module_path_of("src/lib.rs"), Vec::<String>::new());
        assert_eq!(module_path_of("src/foo.rs"), vec!["foo".to_string()]);
        assert_eq!(
            module_path_of("src/foo/bar.rs"),
            vec!["foo".to_string(), "bar".to_string()]
        );
        assert_eq!(module_path_of("src/foo/mod.rs"), vec!["foo".to_string()]);
    }

    #[test]
    fn impl_types() {
        assert_eq!(
            impl_type_of("src/x.rs::impl Foo::bar"),
            Some("Foo".to_string())
        );
        assert_eq!(
            impl_type_of("src/x.rs::impl Foo<T>::bar"),
            Some("Foo".to_string())
        );
        assert_eq!(impl_type_of("src/x.rs::baz"), None);
    }

    fn resolver_with(nodes: Vec<NodeInfo>) -> RustResolver {
        RustResolver::build(&[], vec![], vec![], nodes)
    }

    fn node(qn: &str, id: i64, file: &str, label: &str) -> NodeInfo {
        (qn.to_string(), id, file.to_string(), label.to_string())
    }

    #[test]
    fn finds_impl_method() {
        let r = resolver_with(vec![
            node("src/a.rs::impl Foo::bar", 1, "src/a.rs", "Method"),
            node("src/a.rs::bar", 2, "src/a.rs", "Function"),
        ]);
        assert_eq!(r.find_impl_method("Foo", "bar"), Some(1));
        assert_eq!(r.find_impl_method("Foo", "missing"), None);
        assert_eq!(r.find_impl_method("Baz", "bar"), None);
    }
}
