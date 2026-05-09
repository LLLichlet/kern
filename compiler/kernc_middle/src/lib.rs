#![doc = include_str!("../README.md")]

use kernc_ty::TypeId;
use kernc_utils::{AtomicOrdering, FastHashMap, NodeId};

/// Shared semantic facts attached to source nodes after type checking.
#[derive(Clone, Default)]
pub struct NodeFacts {
    pub node_types: FastHashMap<NodeId, TypeId>,
    pub atomic_orderings: FastHashMap<NodeId, AtomicOrdering>,
    pub method_owner_tys: FastHashMap<NodeId, TypeId>,
    pub call_arg_expected_tys: FastHashMap<NodeId, TypeId>,
    pub binary_operator_lhs_trait_self_tys: FastHashMap<NodeId, TypeId>,
    pub binary_operator_rhs_trait_arg_tys: FastHashMap<NodeId, TypeId>,
    pub match_value_pattern_binary_exprs: FastHashMap<NodeId, NodeId>,
}
