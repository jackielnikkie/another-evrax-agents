//! Graph schema: node labels, edge kinds, and row types.
//!
//! MVP subset of the codebase-memory-mcp taxonomy (28 labels / ~40 edge
//! types). Only what the 4 core queries need.

use std::fmt;

/// Node labels stored in the `nodes` table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeLabel {
    File,
    Module,
    Class,
    Struct,
    Enum,
    Trait,
    Interface,
    Type,
    Function,
    Method,
    Field,
    Variable,
    Constant,
    Macro,
}

impl NodeLabel {
    pub fn as_str(self) -> &'static str {
        match self {
            NodeLabel::File => "File",
            NodeLabel::Module => "Module",
            NodeLabel::Class => "Class",
            NodeLabel::Struct => "Struct",
            NodeLabel::Enum => "Enum",
            NodeLabel::Trait => "Trait",
            NodeLabel::Interface => "Interface",
            NodeLabel::Type => "Type",
            NodeLabel::Function => "Function",
            NodeLabel::Method => "Method",
            NodeLabel::Field => "Field",
            NodeLabel::Variable => "Variable",
            NodeLabel::Constant => "Constant",
            NodeLabel::Macro => "Macro",
        }
    }

    /// True for type-like containers (methods live inside these).
    pub fn is_type_like(self) -> bool {
        matches!(
            self,
            NodeLabel::Class
                | NodeLabel::Struct
                | NodeLabel::Enum
                | NodeLabel::Trait
                | NodeLabel::Interface
                | NodeLabel::Type
        )
    }

    pub fn is_callable(self) -> bool {
        matches!(self, NodeLabel::Function | NodeLabel::Method)
    }
}

impl fmt::Display for NodeLabel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Edge kinds stored in the `edges` table (MVP subset).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeKind {
    /// file/module -> symbol it declares
    Defines,
    /// type-like -> method it declares
    DefinesMethod,
    /// file -> file it imports
    Imports,
    /// function/method -> function/method it calls
    Calls,
    /// symbol -> symbol it references/uses
    Usage,
    /// class -> parent class
    Inherits,
    /// type -> trait/interface it implements
    Implements,
    /// file -> symbol physically inside it
    ContainsFile,
}

impl EdgeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EdgeKind::Defines => "DEFINES",
            EdgeKind::DefinesMethod => "DEFINES_METHOD",
            EdgeKind::Imports => "IMPORTS",
            EdgeKind::Calls => "CALLS",
            EdgeKind::Usage => "USAGE",
            EdgeKind::Inherits => "INHERITS",
            EdgeKind::Implements => "IMPLEMENTS",
            EdgeKind::ContainsFile => "CONTAINS_FILE",
        }
    }
}

impl fmt::Display for EdgeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A node row read back from the DB (used by query results).
#[derive(Debug, Clone)]
pub struct NodeRow {
    pub id: i64,
    pub label: String,
    pub name: String,
    pub qualified_name: String,
    pub file_path: String,
    pub start_line: i64,
    pub end_line: i64,
}
