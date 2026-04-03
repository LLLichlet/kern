use super::{Attribute, Decl, DocBlock};

#[derive(Debug, Clone, PartialEq)]
pub struct Module {
    pub path: String,
    pub docs: Option<DocBlock>,
    pub attributes: Vec<Attribute>,
    pub decls: Vec<Decl>,
}
