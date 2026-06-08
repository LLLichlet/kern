#![doc = include_str!("../README.md")]

//! Shared middle-end facts produced by semantic analysis and consumed downstream.
//!
//! This crate is deliberately small: it contains cross-stage data that does not
//! belong to the AST, semantic context, or final MIR, but still needs a stable
//! representation between compiler phases.

use kernc_ty::TypeId;
use kernc_utils::{AtomicOrdering, FastHashMap, NodeId};

/// Shared semantic facts attached to source nodes after type checking.
///
/// These maps are intentionally sparse.  A `NodeId` appears only when later
/// stages need extra semantic context that is not representable in the AST or
/// lowered expression itself, such as expected call argument types or the trait
/// receiver type chosen for an overloaded operator.
#[derive(Clone, Default)]
pub struct NodeFacts {
    pub node_types: FastHashMap<NodeId, TypeId>,
    pub atomic_orderings: FastHashMap<NodeId, AtomicOrdering>,
    pub method_owner_tys: FastHashMap<NodeId, TypeId>,
    pub call_arg_expected_tys: FastHashMap<NodeId, TypeId>,
    pub binary_operator_lhs_trait_self_tys: FastHashMap<NodeId, TypeId>,
    pub binary_operator_rhs_trait_arg_tys: FastHashMap<NodeId, TypeId>,
    pub match_value_pattern_bind_tys: FastHashMap<NodeId, TypeId>,
}
