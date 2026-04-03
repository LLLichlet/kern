pub mod expr;
pub mod item;
pub mod stmt;

pub use expr::*;
pub use item::*;
pub use stmt::*;

/// Monomorphized item identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MonoId(pub u32);
