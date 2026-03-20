use super::{Attribute, Decl};

#[derive(Debug, Clone, PartialEq)]
pub struct Module {
    pub path: String,
    pub attributes: Vec<Attribute>,
    pub decls: Vec<Decl>,
}
