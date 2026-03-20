/// 节点 ID，用于在 AST 列表中索引
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub u32);

impl NodeId {
    pub fn to_usize(self) -> usize {
        self.0 as usize
    }
}
