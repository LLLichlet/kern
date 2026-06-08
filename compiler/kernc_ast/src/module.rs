//! Parsed module container.
//!
//! A `Module` is the parser/loader boundary: it records the logical module
//! path, module-level docs and attributes, and the declarations parsed from the
//! corresponding source file or inline module body.

use super::{Attribute, Decl, DocBlock};

#[derive(Debug, Clone, PartialEq)]
pub struct Module {
    pub path: String,
    pub docs: Option<DocBlock>,
    pub attributes: Vec<Attribute>,
    pub decls: Vec<Decl>,
}
