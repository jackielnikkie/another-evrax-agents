//! Per-language tree-sitter node-type tables.
//!
//! Ported (as data, not code) from codebase-memory-mcp's
//! `internal/cbm/lang_specs.c` (MIT, DeusData). Instead of `.scm` query
//! files, each language is ~10 lines of table: tree-sitter node-type names
//! grouped by semantic category. Adding a language = one more `LangSpec`.

/// Semantic category of a tree-sitter node type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Function,
    Class,
    Field,
    Module,
    Call,
    Import,
    Variable,
}

pub struct LangSpec {
    pub name: &'static str,
    pub extensions: &'static [&'static str],
    /// tree-sitter node kind that is the file root (not turned into a node)
    pub root_node: &'static str,
    pub function_nodes: &'static [&'static str],
    pub class_nodes: &'static [&'static str],
    pub field_nodes: &'static [&'static str],
    pub module_nodes: &'static [&'static str],
    pub call_nodes: &'static [&'static str],
    pub import_nodes: &'static [&'static str],
    pub variable_nodes: &'static [&'static str],
    /// When true, class-like nodes without a `body` field are treated as type
    /// references, not definitions (C/C++ `struct S` specifiers).
    pub class_needs_body: bool,
}

impl LangSpec {
    pub fn category_of(&self, kind: &str) -> Option<Category> {
        if self.function_nodes.contains(&kind) {
            Some(Category::Function)
        } else if self.class_nodes.contains(&kind) {
            Some(Category::Class)
        } else if self.module_nodes.contains(&kind) {
            Some(Category::Module)
        } else if self.call_nodes.contains(&kind) {
            Some(Category::Call)
        } else if self.import_nodes.contains(&kind) {
            Some(Category::Import)
        } else if self.field_nodes.contains(&kind) {
            Some(Category::Field)
        } else if self.variable_nodes.contains(&kind) {
            Some(Category::Variable)
        } else {
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Language tables
// ---------------------------------------------------------------------------

static RUST: LangSpec = LangSpec {
    name: "rust",
    extensions: &["rs"],
    root_node: "source_file",
    function_nodes: &[
        "function_item",
        "function_signature_item",
        "closure_expression",
    ],
    class_nodes: &[
        "struct_item",
        "enum_item",
        "union_item",
        "trait_item",
        "type_item",
        "impl_item",
    ],
    field_nodes: &["field_declaration"],
    module_nodes: &["mod_item"],
    call_nodes: &["call_expression", "macro_invocation"],
    import_nodes: &["use_declaration", "extern_crate_declaration"],
    variable_nodes: &["static_item", "const_item", "let_declaration"],
    class_needs_body: false,
};

static PYTHON: LangSpec = LangSpec {
    name: "python",
    extensions: &["py"],
    root_node: "module",
    function_nodes: &["function_definition"],
    class_nodes: &["class_definition"],
    field_nodes: &[],
    module_nodes: &[],
    call_nodes: &["call"],
    import_nodes: &[
        "import_statement",
        "import_from_statement",
        "future_import_statement",
    ],
    variable_nodes: &["assignment"],
    class_needs_body: false,
};

static TYPESCRIPT: LangSpec = LangSpec {
    name: "typescript",
    extensions: &["ts", "tsx", "mts", "cts"],
    root_node: "program",
    function_nodes: &[
        "function_declaration",
        "generator_function_declaration",
        "function_expression",
        "arrow_function",
        "method_definition",
    ],
    class_nodes: &[
        "class_declaration",
        "class",
        "abstract_class_declaration",
        "enum_declaration",
        "interface_declaration",
        "type_alias_declaration",
    ],
    field_nodes: &["field_definition", "public_field_definition"],
    module_nodes: &["namespace_declaration", "module_declaration"],
    call_nodes: &["call_expression"],
    import_nodes: &["import_statement"],
    variable_nodes: &["variable_declarator"],
    class_needs_body: false,
};

static GO: LangSpec = LangSpec {
    name: "go",
    extensions: &["go"],
    root_node: "source_file",
    function_nodes: &["function_declaration", "method_declaration", "func_literal"],
    class_nodes: &["type_declaration", "type_spec"],
    field_nodes: &["field_declaration"],
    module_nodes: &[],
    call_nodes: &["call_expression"],
    import_nodes: &["import_declaration", "import_spec"],
    variable_nodes: &["var_spec"],
    class_needs_body: false,
};

static JAVASCRIPT: LangSpec = LangSpec {
    name: "javascript",
    extensions: &["js", "jsx", "mjs", "cjs"],
    root_node: "program",
    function_nodes: &[
        "function_declaration",
        "generator_function_declaration",
        "function_expression",
        "arrow_function",
        "method_definition",
        "generator_function",
    ],
    class_nodes: &["class_declaration", "class"],
    field_nodes: &["field_definition"],
    module_nodes: &[],
    call_nodes: &["call_expression"],
    import_nodes: &["import_statement"],
    variable_nodes: &["variable_declarator"],
    class_needs_body: false,
};

static JAVA: LangSpec = LangSpec {
    name: "java",
    extensions: &["java"],
    root_node: "program",
    function_nodes: &[
        "method_declaration",
        "constructor_declaration",
        "compact_constructor_declaration",
        "lambda_expression",
    ],
    class_nodes: &[
        "class_declaration",
        "interface_declaration",
        "enum_declaration",
        "record_declaration",
        "annotation_type_declaration",
    ],
    field_nodes: &["field_declaration"],
    module_nodes: &["module_declaration"],
    call_nodes: &[
        "method_invocation",
        "object_creation_expression",
        "explicit_constructor_invocation",
    ],
    import_nodes: &["import_declaration"],
    variable_nodes: &["variable_declarator"],
    class_needs_body: false,
};

static C: LangSpec = LangSpec {
    name: "c",
    extensions: &["c", "h"],
    root_node: "translation_unit",
    function_nodes: &["function_definition"],
    class_nodes: &["struct_specifier", "enum_specifier", "union_specifier"],
    field_nodes: &["field_declaration"],
    module_nodes: &[],
    call_nodes: &["call_expression"],
    import_nodes: &["preproc_include"],
    variable_nodes: &["init_declarator"],
    class_needs_body: true,
};

static CPP: LangSpec = LangSpec {
    name: "cpp",
    extensions: &["cpp", "cc", "cxx", "hpp", "hh", "hxx"],
    root_node: "translation_unit",
    function_nodes: &["function_definition", "lambda_expression"],
    class_nodes: &[
        "class_specifier",
        "struct_specifier",
        "enum_specifier",
        "union_specifier",
    ],
    field_nodes: &["field_declaration"],
    module_nodes: &["namespace_definition"],
    call_nodes: &["call_expression"],
    import_nodes: &["preproc_include", "import_declaration", "using_declaration"],
    variable_nodes: &["init_declarator"],
    class_needs_body: true,
};

static CSHARP: LangSpec = LangSpec {
    name: "csharp",
    extensions: &["cs"],
    root_node: "compilation_unit",
    function_nodes: &[
        "method_declaration",
        "constructor_declaration",
        "destructor_declaration",
        "local_function_statement",
        "lambda_expression",
        "anonymous_method_expression",
        "operator_declaration",
    ],
    class_nodes: &[
        "class_declaration",
        "struct_declaration",
        "interface_declaration",
        "enum_declaration",
        "record_declaration",
    ],
    field_nodes: &["field_declaration"],
    module_nodes: &["namespace_declaration", "file_scoped_namespace_declaration"],
    call_nodes: &["invocation_expression", "object_creation_expression"],
    import_nodes: &["using_directive"],
    variable_nodes: &["variable_declarator"],
    class_needs_body: false,
};

static PHP: LangSpec = LangSpec {
    name: "php",
    extensions: &["php"],
    root_node: "program",
    function_nodes: &[
        "function_definition",
        "method_declaration",
        "anonymous_function",
        "arrow_function",
    ],
    class_nodes: &[
        "class_declaration",
        "interface_declaration",
        "trait_declaration",
        "enum_declaration",
    ],
    field_nodes: &["property_declaration"],
    module_nodes: &["namespace_definition"],
    call_nodes: &[
        "function_call_expression",
        "member_call_expression",
        "nullsafe_member_call_expression",
        "scoped_call_expression",
        "object_creation_expression",
    ],
    import_nodes: &["namespace_use_declaration"],
    variable_nodes: &[],
    class_needs_body: false,
};

static RUBY: LangSpec = LangSpec {
    name: "ruby",
    extensions: &["rb"],
    root_node: "program",
    function_nodes: &["method", "singleton_method", "lambda"],
    class_nodes: &["class", "module", "singleton_class"],
    field_nodes: &[],
    module_nodes: &[],
    call_nodes: &["call"],
    import_nodes: &[],
    variable_nodes: &["assignment"],
    class_needs_body: false,
};

static BASH: LangSpec = LangSpec {
    name: "bash",
    extensions: &["sh", "bash"],
    root_node: "program",
    function_nodes: &["function_definition"],
    class_nodes: &[],
    field_nodes: &[],
    module_nodes: &[],
    call_nodes: &[],
    import_nodes: &[],
    variable_nodes: &["variable_assignment"],
    class_needs_body: false,
};

static LANGS: &[&LangSpec] = &[
    &RUST,
    &PYTHON,
    &TYPESCRIPT,
    &GO,
    &JAVASCRIPT,
    &JAVA,
    &C,
    &CPP,
    &CSHARP,
    &PHP,
    &RUBY,
    &BASH,
];

/// Find the spec handling a file extension (without the dot).
pub fn find_spec(extension: &str) -> Option<&'static LangSpec> {
    LANGS
        .iter()
        .find(|s| s.extensions.contains(&extension))
        .copied()
}

/// tree-sitter Language for a file extension.
pub fn language_for(extension: &str) -> Option<tree_sitter::Language> {
    match extension {
        "rs" => Some(tree_sitter_rust::LANGUAGE.into()),
        "py" => Some(tree_sitter_python::LANGUAGE.into()),
        "ts" | "mts" | "cts" => Some(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()),
        "tsx" => Some(tree_sitter_typescript::LANGUAGE_TSX.into()),
        "go" => Some(tree_sitter_go::LANGUAGE.into()),
        "js" | "jsx" | "mjs" | "cjs" => Some(tree_sitter_javascript::LANGUAGE.into()),
        "java" => Some(tree_sitter_java::LANGUAGE.into()),
        "c" | "h" => Some(tree_sitter_c::LANGUAGE.into()),
        "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx" => Some(tree_sitter_cpp::LANGUAGE.into()),
        "cs" => Some(tree_sitter_c_sharp::LANGUAGE.into()),
        "php" => Some(tree_sitter_php::LANGUAGE_PHP.into()),
        "rb" => Some(tree_sitter_ruby::LANGUAGE.into()),
        "sh" | "bash" => Some(tree_sitter_bash::LANGUAGE.into()),
        _ => None,
    }
}
