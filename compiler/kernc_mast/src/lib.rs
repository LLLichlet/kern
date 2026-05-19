#![doc = include_str!("../README.md")]

//! Monomorphized AST passed from frontend lowering to backend lowering.
//!
//! MAST has no unresolved generics, nested modules, or trait dispatch syntax.
//! It keeps high-level constructs that are still useful for structured MIR/LLVM
//! generation, such as blocks, switches, aggregate init, SIMD intrinsics, and
//! explicit atomic operations.

pub mod expr;
pub mod item;
pub mod stmt;

pub use expr::*;
pub use item::*;
pub use stmt::*;
