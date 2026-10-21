//! Single-file extraction: tree-sitter parse -> raw nodes / calls / imports.
//!
//! Uses the per-language [`LangSpec`] tables (no `.scm` files). Name-based
//! call resolution happens later in [`crate::index`]; what we record here is
//! `(caller_qualified_name, callee_text)`.

use std::collections::HashSet;
use tree_sitter::{Node, Parser};

use crate::lang::{Category, LangSpec};
use crate::schema::NodeLabel;

/// A symbol found in one file. `nodes[0]` is always the file node itself.
#[derive(Debug, Clone)]
pub struct RawNode {
    pub label: NodeLabel,
    pub name: String,
    pub qualified_name: String,
    pub start_line: u32, // 1-based
    pub end_line: u32,   // 1-based
    /// index into `FileGraph.nodes` of the enclosing scope (None = file top level)
    pub parent: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct RawCall {
    pub caller_qualified: String,
    pub caller_file: String,
    /// last segment, normalized (`foo`)
    pub callee_name: String,
    /// `x` in `x.foo()`, `self` in `self.foo()`; None for plain `foo()`
    pub receiver: Option<String>,
    /// full `a::b::c` path for path-calls (`foo::bar()`); None otherwise
    pub path: Option<String>,
}

/// A single imported name from a Rust `use` statement:
/// `use a::b::{c, d as e}` -> [("c","a::b::c"), ("e","a::b::d")].
#[derive(Debug, Clone)]
pub struct RawUse {
    pub file: String,
    pub local: String,
    pub full_path: String,
}

/// A local binding with a known type inside one function:
/// `let x: Foo = ...` or `let x = Foo::new()` -> ("x", "Foo").
#[derive(Debug, Clone)]
pub struct RawLet {
    pub func_qualified: String,
    pub var: String,
    pub ty: String,
}

#[derive(Debug, Clone)]
pub struct RawImport {
    pub importer_file: String,
    pub target_file: String,
}

#[derive(Debug, Default)]
pub struct FileGraph {
    pub nodes: Vec<RawNode>,
    pub calls: Vec<RawCall>,
    pub imports: Vec<RawImport>,
    /// Rust `use` name mappings (phase-2 resolution)
    pub uses: Vec<RawUse>,
    /// Rust local bindings with known types (phase-2 resolution)
    pub lets: Vec<RawLet>,
}

/// Parse one file and extract its graph fragment.
///
/// `rel_path` is the `/`-separated path relative to the project root.
/// `files` is the set of all indexed relative paths (for import resolution).
pub fn extract_file(
    spec: &'static LangSpec,
    language: tree_sitter::Language,
    rel_path: &str,
    source: &str,
    files: &HashSet<String>,
) -> FileGraph {
    let mut parser = Parser::new();
    parser
        .set_language(&language)
        .expect("incompatible tree-sitter language");
    let tree = match parser.parse(source, None) {
        Some(t) => t,
        None => return FileGraph::default(),
    };

    let mut ctx = Ctx {
        spec,
        source,
        rel_path,
        files,
        out: FileGraph::default(),
        scope: Vec::new(),
    };
    // nodes[0] = the file node itself.
    ctx.out.nodes.push(RawNode {
        label: NodeLabel::File,
        name: rel_path.to_string(),
        qualified_name: rel_path.to_string(),
        start_line: 1,
        end_line: source.lines().count().max(1) as u32,
        parent: None,
    });
    ctx.scope.push((rel_path.to_string(), 0));
    ctx.visit(tree.root_node());
    ctx.out
}

struct Ctx<'a> {
    spec: &'static LangSpec,
    source: &'a str,
    rel_path: &'a str,
    files: &'a HashSet<String>,
    out: FileGraph,
    /// enclosing scopes: (name, node index)
    scope: Vec<(String, usize)>,
}

impl<'a> Ctx<'a> {
    fn text(&self, node: Node) -> String {
        self.source[node.start_byte()..node.end_byte()].to_string()
    }

    fn qualified(&self, name: &str) -> String {
        let mut q = String::with_capacity(self.rel_path.len() + name.len() + 16);
        q.push_str(self.rel_path);
        for (s, _) in &self.scope {
            // scope[0] is the file itself — already the prefix
            if s.as_str() == self.rel_path {
                continue;
            }
            q.push_str("::");
            q.push_str(s);
        }
        q.push_str("::");
        q.push_str(name);
        q
    }

    fn visit_children(&mut self, node: Node) {
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            self.visit(child);
        }
    }

    fn visit(&mut self, node: Node) {
        let kind = node.kind();
        if kind == self.spec.root_node {
            self.visit_children(node);
            return;
        }
        // Closures/lambdas: don't create a node (no stable name), but still
        // walk inside so calls within them attribute to the enclosing fn.
        if kind == "closure_expression" || kind == "arrow_function" {
            self.visit_children(node);
            return;
        }
        match self.spec.category_of(kind) {
            Some(Category::Function) => self.visit_symbol(node, NodeLabel::Function, None),
            Some(Category::Class) => {
                // C/C++: a specifier without a `body` is a type reference
                // (`struct Point p`), not a definition — don't node it.
                if self.spec.class_needs_body && node.child_by_field_name("body").is_none() {
                    self.visit_children(node);
                } else {
                    self.visit_symbol(node, class_label(kind), None)
                }
            }
            Some(Category::Module) => self.visit_symbol(node, NodeLabel::Module, None),
            Some(Category::Field) => self.visit_symbol(node, NodeLabel::Field, None),
            Some(Category::Variable) => self.visit_variable(node),
            Some(Category::Call) => {
                self.record_call(node);
                self.visit_children(node);
            }
            Some(Category::Import) => {
                self.record_import(node);
                // don't recurse into import statements (noise)
            }
            None => self.visit_children(node),
        }
    }

    fn visit_symbol(&mut self, node: Node, mut label: NodeLabel, forced_name: Option<String>) {
        let mut name = match forced_name.or_else(|| node_name(node, self.source)) {
            Some(n) => n,
            None => {
                self.visit_children(node);
                return;
            }
        };
        // `impl Foo { ... }` has no name field; qualify it so it can't
        // collide with the `Foo` type itself.
        if node.kind() == "impl_item" {
            name = format!("impl {name}");
            label = NodeLabel::Class;
        }
        // A function directly inside a type-like scope is a method.
        if label == NodeLabel::Function {
            if let Some((_, pi)) = self.scope.last() {
                if self.out.nodes[*pi].label.is_type_like() {
                    label = NodeLabel::Method;
                }
            }
        }
        let parent = self.scope.last().map(|(_, i)| *i);
        // Nearest enclosing callable, for `let` type tracking below.
        let enclosing_fn_qn: Option<String> = self.scope.iter().rev().find_map(|(_, i)| {
            let n = &self.out.nodes[*i];
            if n.label == NodeLabel::Function || n.label == NodeLabel::Method {
                Some(n.qualified_name.clone())
            } else {
                None
            }
        });
        let qualified_name = self.qualified(&name);
        let idx = self.out.nodes.len();
        self.out.nodes.push(RawNode {
            label,
            name: name.clone(),
            qualified_name,
            start_line: node.start_position().row as u32 + 1,
            end_line: node.end_position().row as u32 + 1,
            parent,
        });
        self.scope.push((name, idx));
        // `let x: Foo = ...` / `let x = Foo::new()` — record the binding type
        // so phase 2 can resolve `x.method()` calls.
        if node.kind() == "let_declaration" {
            if let (Some(func_qn), Some((var, ty))) =
                (enclosing_fn_qn, let_binding(node, self.source))
            {
                self.out.lets.push(RawLet {
                    func_qualified: func_qn,
                    var,
                    ty,
                });
            }
        }
        self.visit_children(node);
        self.scope.pop();
    }

    /// `const f = () => {}` / `let g = function() {}`: the function takes the
    /// variable's name instead of creating a separate Variable node.
    fn visit_variable(&mut self, node: Node) {
        let vname = node_name(node, self.source);
        if let Some(vname) = vname {
            let mut cursor = node.walk();
            let mut named = false;
            for child in node.named_children(&mut cursor) {
                match child.kind() {
                    "arrow_function" | "function_expression" => {
                        self.visit_symbol(child, NodeLabel::Function, Some(vname.clone()));
                        named = true;
                    }
                    _ => {}
                }
            }
            if named {
                return;
            }
        }
        self.visit_symbol(node, NodeLabel::Variable, None);
    }

    fn record_call(&mut self, node: Node) {
        // Enclosing callable scope = the caller.
        let caller = self.scope.iter().rev().find(|(_, i)| {
            let l = self.out.nodes[*i].label;
            l == NodeLabel::Function || l == NodeLabel::Method
        });
        let Some((_, ci)) = caller else { return };
        let caller_qualified = self.out.nodes[*ci].qualified_name.clone();
        // callee field name varies per grammar: `function` (most),
        // `method` (ruby `call`), `name` (java `method_invocation`)
        let fnode = node
            .child_by_field_name("function")
            .or_else(|| node.child_by_field_name("method"))
            .or_else(|| node.child_by_field_name("name"));
        let Some(fnode) = fnode else {
            return;
        };
        let raw = self.text(fnode).trim().to_string();
        if raw.is_empty() {
            return;
        }
        // Split receiver from callee: `x.foo()` -> receiver `x`, callee `foo`.
        // Path calls `a::b::c()` keep the full path for phase-2 resolution.
        let (receiver, after_dot) = match raw.rfind('.') {
            Some(i) => (
                Some(raw[..i].trim().to_string()),
                raw[i + 1..].trim().to_string(),
            ),
            None => (None, raw.clone()),
        };
        let path = if raw.contains("::") {
            Some(raw.clone())
        } else {
            None
        };
        // callee = last path segment, normalized (`foo::<T>` -> `foo`)
        let mut callee = after_dot
            .rsplit("::")
            .next()
            .unwrap_or("")
            .trim()
            .to_string();
        callee = callee
            .trim_matches(|c: char| !c.is_alphanumeric() && c != '_')
            .to_string();
        if callee.is_empty() {
            return;
        }
        let receiver = receiver.filter(|r| !r.is_empty());
        self.out.calls.push(RawCall {
            caller_qualified,
            caller_file: self.rel_path.to_string(),
            callee_name: callee,
            receiver,
            path,
        });
    }

    fn record_import(&mut self, node: Node) {
        let raw = self.text(node);
        if let Some(target) = resolve_import(self.spec.name, self.rel_path, &raw, self.files) {
            self.out.imports.push(RawImport {
                importer_file: self.rel_path.to_string(),
                target_file: target,
            });
        }
        // Rust: also record name mappings for phase-2 call resolution.
        if self.spec.name == "rust" {
            for (local, full_path) in parse_use(&raw) {
                self.out.uses.push(RawUse {
                    file: self.rel_path.to_string(),
                    local,
                    full_path,
                });
            }
        }
    }
}

/// Map a class-category node kind to a precise label.
fn class_label(kind: &str) -> NodeLabel {
    match kind {
        "struct_item" => NodeLabel::Struct,
        "enum_item" | "enum_declaration" => NodeLabel::Enum,
        "trait_item" | "interface_declaration" => NodeLabel::Trait,
        "type_item" | "type_alias_declaration" | "type_alias" => NodeLabel::Type,
        _ => NodeLabel::Class,
    }
}

/// Best-effort symbol name: `name` field, else first identifier-like child.
fn node_name(node: Node, source: &str) -> Option<String> {
    if let Some(n) = node.child_by_field_name("name") {
        let t = source[n.start_byte()..n.end_byte()].trim().to_string();
        if !t.is_empty() {
            return Some(t);
        }
    }
    // C-style: the name sits under the `declarator` field
    // (`function_declarator` -> `declarator` -> `identifier`).
    if let Some(d) = node.child_by_field_name("declarator") {
        if let Some(t) = find_ident(&d, source, 3) {
            return Some(t);
        }
    }
    // generic fallback: bounded pre-order search for an identifier-like node
    find_ident(&node, source, 3)
}

/// Pre-order search for the first identifier-like descendant, bounded by depth.
/// Skips attribute subtrees (they can contain misleading identifiers).
fn find_ident(node: &tree_sitter::Node, source: &str, depth: u8) -> Option<String> {
    if depth == 0 {
        return None;
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "identifier"
            | "type_identifier"
            | "simple_identifier"
            | "name"
            | "dotted_name"
            | "property_identifier"
            | "field_identifier" => {
                let t = source[child.start_byte()..child.end_byte()]
                    .trim()
                    .to_string();
                if !t.is_empty() {
                    return Some(t);
                }
            }
            k if k.contains("attribute") => {}
            _ => {
                if let Some(t) = find_ident(&child, source, depth - 1) {
                    return Some(t);
                }
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Import resolution (best-effort, file-level)
// ---------------------------------------------------------------------------

fn resolve_import(
    lang: &str,
    importer: &str,
    raw: &str,
    files: &HashSet<String>,
) -> Option<String> {
    match lang {
        "rust" => resolve_rust_import(importer, raw, files),
        "python" => resolve_python_import(importer, raw, files),
        "typescript" => resolve_ts_import(importer, raw, files),
        _ => None, // go: stdlib/external dominate; skip in MVP
    }
}

fn parent_dir(path: &str) -> &str {
    path.rsplit_once('/').map(|(d, _)| d).unwrap_or("")
}

fn rust_candidates(mod_path: &str, importer: &str) -> Vec<String> {
    let rel = mod_path.replace("::", "/");
    let mut c = vec![format!("{rel}.rs"), format!("{rel}/mod.rs")];
    let dir = parent_dir(importer);
    if !dir.is_empty() {
        c.push(format!("{dir}/{rel}.rs"));
        c.push(format!("{dir}/{rel}/mod.rs"));
    }
    // crate root guess: src/
    c.push(format!("src/{rel}.rs"));
    c.push(format!("src/{rel}/mod.rs"));
    c
}

fn resolve_rust_import(importer: &str, raw: &str, files: &HashSet<String>) -> Option<String> {
    let mut p = raw.trim();
    p = p.strip_prefix("use").unwrap_or(p).trim();
    p = p.trim_end_matches(';').trim();
    // cut `::{...}`, ` as x`, trailing `{...}`
    for cut in ["::{", " as "] {
        if let Some(i) = p.find(cut) {
            p = p[..i].trim();
            break;
        }
    }
    if p.is_empty() || p.contains('{') || p.contains('*') {
        return None;
    }
    let first = p.split("::").next().unwrap_or("");
    if ["std", "core", "alloc"].contains(&first) {
        return None; // stdlib
    }
    // normalize super:: / self:: against the importer's directory
    let mut p = p.to_string();
    let dir = parent_dir(importer);
    if let Some(rest) = p.strip_prefix("self::") {
        p = if dir.is_empty() {
            rest.to_string()
        } else {
            format!("{}::{}", dir.replace('/', "::"), rest)
        };
    } else {
        let mut d: Vec<&str> = dir.split('/').filter(|s| !s.is_empty()).collect();
        while let Some(rest) = p.strip_prefix("super::") {
            d.pop();
            p = rest.to_string();
        }
        if p.starts_with("super") {
            return None;
        }
        let _ = d;
    }
    let p = p.strip_prefix("crate::").unwrap_or(&p);
    rust_candidates(p, importer)
        .into_iter()
        .find(|cand| files.contains(cand))
}

fn resolve_python_import(importer: &str, raw: &str, files: &HashSet<String>) -> Option<String> {
    let p = raw.trim().trim_end_matches(';').trim();
    // from X import Y  |  import X
    let module = if let Some(rest) = p.strip_prefix("from") {
        let rest = rest.trim();
        let i = rest.find(" import")?;
        rest[..i].trim()
    } else {
        let rest = p.strip_prefix("import")?;
        rest.trim().split(',').next().unwrap_or("").trim()
    };
    if module.is_empty() {
        return None;
    }
    // relative: leading dots
    let dots = module.chars().take_while(|c| *c == '.').count();
    let module = module.trim_start_matches('.');
    let mut base = parent_dir(importer).to_string();
    for _ in 1..dots {
        base = parent_dir(&base).to_string();
    }
    let rel = module.replace('.', "/");
    let joined = if base.is_empty() {
        rel.clone()
    } else {
        format!("{base}/{rel}")
    };
    [
        format!("{joined}.py"),
        format!("{joined}/__init__.py"),
        format!("{rel}.py"),
        format!("{rel}/__init__.py"),
    ]
    .into_iter()
    .find(|cand| files.contains(cand))
}

fn resolve_ts_import(importer: &str, raw: &str, files: &HashSet<String>) -> Option<String> {
    // grab the quoted module specifier
    let bytes = raw.as_bytes();
    let mut start = None;
    let mut quote = 0u8;
    for (i, b) in bytes.iter().enumerate() {
        if *b == b'\'' || *b == b'"' {
            let Some(start_idx) = start else {
                start = Some(i);
                quote = *b;
                continue;
            };
            if *b == quote {
                let spec = &raw[start_idx + 1..i];
                if !spec.starts_with('.') {
                    return None; // bare specifier -> node_modules
                }
                let dir = parent_dir(importer);
                let joined = if dir.is_empty() {
                    spec.to_string()
                } else {
                    format!("{dir}/{spec}")
                };
                // normalize ./ and ../ segments (simple)
                let norm = normalize_dots(&joined);
                return [
                    format!("{norm}.ts"),
                    format!("{norm}.tsx"),
                    format!("{norm}/index.ts"),
                    format!("{norm}/index.tsx"),
                    norm.clone(),
                ]
                .into_iter()
                .find(|cand| files.contains(cand));
            }
        }
    }
    None
}

fn normalize_dots(path: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            s => out.push(s),
        }
    }
    out.join("/")
}

// ---------------------------------------------------------------------------
// Phase-2 helpers (Rust): `use` name mapping + `let` type inference
// ---------------------------------------------------------------------------

/// Parse `use a::b::{c, d as e};` into (local_name, full_path) pairs.
/// Returns empty for globs handled separately, nested groups, etc.
fn parse_use(raw: &str) -> Vec<(String, String)> {
    let mut p = raw.trim();
    p = p.strip_prefix("use").unwrap_or(p).trim();
    p = p.trim_end_matches(';').trim();
    if p.is_empty() {
        return Vec::new();
    }
    // `use a::b::{c, d as e};`
    if let Some(i) = p.find("::{") {
        let base = p[..i].trim().to_string();
        let inner = p[i + 3..].trim_end_matches('}').trim();
        if inner.contains('{') {
            return Vec::new(); // nested groups: skip in MVP
        }
        return inner
            .split(',')
            .filter_map(|s| {
                let s = s.trim();
                if s.is_empty() {
                    return None;
                }
                if s == "*" {
                    return Some(("*".to_string(), base.clone()));
                }
                if s == "self" {
                    return None;
                }
                let (name, alias) = match s.split_once(" as ") {
                    Some((n, a)) => (n.trim(), a.trim().to_string()),
                    None => (s, s.to_string()),
                };
                if name.is_empty() || alias.is_empty() {
                    None
                } else {
                    Some((alias, format!("{base}::{name}")))
                }
            })
            .collect();
    }
    // `use a::b::c;` / `use a::b::c as d;` / `use a::*;`
    if p.ends_with("::*") {
        let base = p.trim_end_matches("::*").trim().to_string();
        return vec![("*".to_string(), base)];
    }
    if p.contains('{') || p.contains('*') {
        return Vec::new();
    }
    let (path, alias) = match p.split_once(" as ") {
        Some((pa, a)) => (pa.trim(), a.trim().to_string()),
        None => (p, String::new()),
    };
    let local = if alias.is_empty() {
        path.rsplit("::").next().unwrap_or(path).to_string()
    } else {
        alias
    };
    if local.is_empty() || local == "self" {
        return Vec::new();
    }
    vec![(local, path.to_string())]
}

/// Normalize a Rust type text to a bare type name: `&mut Foo<T>` -> `Foo`.
fn normalize_rust_type(t: &str) -> String {
    let mut s = t.trim();
    s = s.strip_prefix('&').unwrap_or(s).trim();
    s = s.strip_prefix("mut ").unwrap_or(s).trim();
    if let Some(i) = s.find('<') {
        s = s[..i].trim();
    }
    if let Some(i) = s.find('[') {
        s = s[..i].trim();
    }
    s.rsplit("::").next().unwrap_or(s).trim().to_string()
}

/// Extract `(var, type)` from a `let_declaration`:
/// - `let x: Foo = ...` -> ("x", "Foo") via the `type` field
/// - `let x = Foo::new()` / `let x = Foo::default()` -> ("x", "Foo") via ctor call
/// - `let x = Foo { ... }` -> ("x", "Foo") via struct literal
fn let_binding(node: Node, source: &str) -> Option<(String, String)> {
    let text_of = |n: Node| source[n.start_byte()..n.end_byte()].trim().to_string();
    let pattern = node.child_by_field_name("pattern")?;
    if pattern.kind() != "identifier" {
        return None; // destructuring etc.: skip in MVP
    }
    let var = text_of(pattern);
    if var.is_empty() || var == "_" {
        return None;
    }
    // 1. explicit annotation
    if let Some(t) = node.child_by_field_name("type") {
        let ty = normalize_rust_type(&text_of(t));
        if !ty.is_empty() {
            return Some((var, ty));
        }
    }
    // 2. constructor call `Foo::new()` / struct literal `Foo {`
    if let Some(v) = node.child_by_field_name("value") {
        match v.kind() {
            "call_expression" => {
                if let Some(f) = v.child_by_field_name("function") {
                    let ft = text_of(f);
                    // `Foo::new` -> `Foo`; plain `foo()` -> can't infer
                    if let Some(i) = ft.find("::") {
                        let ty = normalize_rust_type(&ft[..i]);
                        if !ty.is_empty() && ty != "Self" {
                            return Some((var, ty));
                        }
                    }
                }
            }
            "struct_expression" => {
                let mut cursor = v.walk();
                for child in v.named_children(&mut cursor) {
                    let k = child.kind();
                    if k == "type_identifier" || k == "scoped_type_identifier" {
                        let ty = normalize_rust_type(&text_of(child));
                        if !ty.is_empty() {
                            return Some((var, ty));
                        }
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn use_parsing() {
        assert_eq!(
            parse_use("use crate::foo::bar;"),
            vec![("bar".to_string(), "crate::foo::bar".to_string())]
        );
        assert_eq!(
            parse_use("use crate::foo::{bar, baz as b};"),
            vec![
                ("bar".to_string(), "crate::foo::bar".to_string()),
                ("b".to_string(), "crate::foo::baz".to_string()),
            ]
        );
        assert_eq!(
            parse_use("use std::io::Write as W;"),
            vec![("W".to_string(), "std::io::Write".to_string())]
        );
        assert_eq!(
            parse_use("use crate::x::*;"),
            vec![("*".to_string(), "crate::x".to_string())]
        );
        assert!(parse_use("use std::io;").len() == 1); // kept; filtered later
        assert!(parse_use("").is_empty());
    }

    #[test]
    fn type_normalization() {
        assert_eq!(normalize_rust_type("&mut Foo<T>"), "Foo");
        assert_eq!(normalize_rust_type("crate::bar::Baz"), "Baz");
        assert_eq!(normalize_rust_type("Option<String>"), "Option");
        assert_eq!(normalize_rust_type("u32"), "u32");
    }
}
