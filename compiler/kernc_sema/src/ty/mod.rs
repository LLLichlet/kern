//! Semantic type helpers layered on top of `kernc_ty`.
//!
//! `kernc_ty` owns canonical type data.  This module adds sema-aware formatting,
//! layout computation, and generic/associated-type substitution that require
//! access to definitions, spans, diagnostics, or the active target machine.

mod format;
mod layout;
mod subst;

pub(crate) use format::TypeFormatter;
pub use layout::LayoutEngine;
pub use subst::{Substituter, substitute_associated_types};

pub use kernc_ty::*;
