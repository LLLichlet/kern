pub mod expr;
pub mod item;
pub mod stmt;

pub use expr::*;
pub use item::*;
pub use stmt::*;

/// 单态化 ID
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MonoId(pub u32);
