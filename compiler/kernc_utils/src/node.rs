//! Stable AST node identifiers used as keys in compiler-side tables.

/// Stable node identifier used to index compiler-side AST tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub u32);

impl NodeId {
    pub fn to_usize(self) -> usize {
        self.0 as usize
    }
}
